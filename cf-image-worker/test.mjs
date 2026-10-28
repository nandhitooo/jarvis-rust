import worker from "./worker.js";

const API_KEY = "test-secret-key";
const AUTH = `Bearer ${API_KEY}`;
const JPEG = new Uint8Array(1200);
JPEG.set([0xff, 0xd8, 0xff]);
JPEG[JPEG.length - 2] = 0xff;
JPEG[JPEG.length - 1] = 0xd9;
const FLUX_ID = "@cf/black-forest-labs/flux-1-schnell";
const SDXL_ID = "@cf/bytedance/stable-diffusion-xl-lightning";
const TEXT_MODEL = "@cf/meta/llama-3.1-8b-instruct";
const VISION_MODEL = "@cf/llava-hf/llava-1.5-7b-hf";
const encodedJpeg = btoa(String.fromCharCode(...JPEG));

function mockEnv(run, apiKey = API_KEY) {
  return { API_KEY: apiKey, AI: { run } };
}

function request(path, { method = "POST", headers = {}, body } = {}) {
  return new Request(`https://worker.test${path}`, {
    method,
    headers,
    ...(body === undefined ? {} : { body }),
  });
}

async function expectStatus(response, status) {
  if (response.status !== status) {
    throw new Error(`Expected status ${status}, got ${response.status}`);
  }
}

async function jsonBody(response) {
  return response.json();
}

async function runCase(name, test) {
  try {
    await test();
    console.log(`PASS ${name}`);
    return true;
  } catch (error) {
    console.error(`FAIL ${name}: ${error instanceof Error ? error.message : error}`);
    return false;
  }
}

const cases = [
  [
    "unauthorized requests",
    async () => {
      const run = async () => ({ image: encodedJpeg });
      await expectStatus(
        await worker.fetch(request("/", { body: JSON.stringify({ prompt: "car" }) }), mockEnv(run)),
        401,
      );
      await expectStatus(
        await worker.fetch(
          request("/", {
            method: "POST",
            headers: { Authorization: AUTH },
            body: JSON.stringify({ prompt: "car" }),
          }),
          mockEnv(run, ""),
        ),
        401,
      );
    },
  ],
  [
    "routes and health",
    async () => {
      await expectStatus(await worker.fetch(request("/", { method: "GET" }), mockEnv()), 405);
      const response = await worker.fetch(request("/health", { method: "GET" }), {});
      await expectStatus(response, 200);
      const body = await jsonBody(response);
      if (body.ok !== true || Object.keys(body).length !== 1) {
        throw new Error("Health response must be exactly {ok:true}");
      }
    },
  ],
  [
    "invalid request bodies",
    async () => {
      const env = mockEnv(async () => ({ image: encodedJpeg }));
      const withAuth = (body) =>
        request("/", { headers: { Authorization: AUTH }, body });
      await expectStatus(
        await worker.fetch(withAuth(JSON.stringify({ prompt: " " })), env),
        400,
      );
      await expectStatus(
        await worker.fetch(withAuth(JSON.stringify({ prompt: "x".repeat(501) })), env),
        400,
      );
      await expectStatus(await worker.fetch(withAuth("{"), env), 400);
    },
  ],
  [
    "Flux happy path and response headers",
    async () => {
      const env = mockEnv(async (model) => {
        if (model === TEXT_MODEL) {
          return {
            response: JSON.stringify({
              prompt: "A clearly visible driver in a blue racing suit",
              subjects: ["driver"],
            }),
          };
        }
        if (model === FLUX_ID) return { image: encodedJpeg };
        throw new Error(`Unexpected model: ${model}`);
      });
      const response = await worker.fetch(
        request("/", {
          headers: { Authorization: AUTH, "Content-Type": "application/json" },
          body: JSON.stringify({ prompt: "driver" }),
        }),
        env,
      );
      await expectStatus(response, 200);
      if (response.headers.get("Content-Type") !== "image/jpeg") {
        throw new Error("Expected image/jpeg");
      }
      for (const name of ["X-Model", "X-Final-Prompt", "X-Attempts"]) {
        if (!response.headers.get(name)) throw new Error(`Missing ${name}`);
      }
    },
  ],
  [
    "text model failure keeps original prompt",
    async () => {
      let fluxPrompt = "";
      const env = mockEnv(async (model, input) => {
        if (model === TEXT_MODEL) throw new Error("Planner unavailable");
        if (model === FLUX_ID) {
          fluxPrompt = input.prompt;
          return { image: encodedJpeg };
        }
        throw new Error(`Unexpected model: ${model}`);
      });
      const response = await worker.fetch(
        request("/", {
          headers: { Authorization: AUTH },
          body: JSON.stringify({ prompt: "original scene" }),
        }),
        env,
      );
      await expectStatus(response, 200);
      if (fluxPrompt !== "original scene") throw new Error("Did not use original prompt");
    },
  ],
  [
    "Flux failures fall back to SDXL",
    async () => {
      let fluxCalls = 0;
      const env = mockEnv(async (model) => {
        if (model === TEXT_MODEL) {
          return {
            response: JSON.stringify({ prompt: "improved scene", subjects: [] }),
          };
        }
        if (model === FLUX_ID) {
          fluxCalls += 1;
          throw new Error("Flux unavailable");
        }
        if (model === SDXL_ID) return new Response(JPEG);
        throw new Error(`Unexpected model: ${model}`);
      });
      const response = await worker.fetch(
        request("/", {
          headers: { Authorization: AUTH },
          body: JSON.stringify({ prompt: "original scene" }),
        }),
        env,
      );
      await expectStatus(response, 200);
      if (fluxCalls !== 2 || response.headers.get("X-Model") !== SDXL_ID) {
        throw new Error("SDXL fallback was not used");
      }
    },
  ],
  [
    "missing subject retry passes",
    async () => {
      let fluxCalls = 0;
      let visionCalls = 0;
      const env = mockEnv(async (model) => {
        if (model === TEXT_MODEL) {
          return { response: JSON.stringify({ prompt: "Driver", subjects: ["driver"] }) };
        }
        if (model === FLUX_ID) {
          fluxCalls += 1;
          return { image: encodedJpeg };
        }
        if (model === VISION_MODEL) {
          visionCalls += 1;
          return {
            description: visionCalls === 1 ? "MISSING: driver" : "MISSING: none",
          };
        }
        throw new Error(`Unexpected model: ${model}`);
      });
      const response = await worker.fetch(
        request("/", {
          headers: { Authorization: AUTH },
          body: JSON.stringify({ prompt: "driver", seed: 7 }),
        }),
        env,
      );
      await expectStatus(response, 200);
      if (fluxCalls !== 2 || response.headers.get("X-Verified") !== "retry-pass") {
        throw new Error("Expected two Flux calls and retry-pass verification");
      }
    },
  ],
  [
    "vision failure returns image as unknown",
    async () => {
      let fluxCalls = 0;
      const env = mockEnv(async (model) => {
        if (model === TEXT_MODEL) {
          return { response: JSON.stringify({ prompt: "Driver", subjects: ["driver"] }) };
        }
        if (model === FLUX_ID) {
          fluxCalls += 1;
          return { image: encodedJpeg };
        }
        if (model === VISION_MODEL) throw new Error("Vision unavailable");
        throw new Error(`Unexpected model: ${model}`);
      });
      const response = await worker.fetch(
        request("/", {
          headers: { Authorization: AUTH },
          body: JSON.stringify({ prompt: "driver" }),
        }),
        env,
      );
      await expectStatus(response, 200);
      if (fluxCalls !== 1 || response.headers.get("X-Verified") !== "unknown") {
        throw new Error("Expected one Flux call and unknown verification");
      }
    },
  ],
  [
    "verify false skips vision",
    async () => {
      let visionCalls = 0;
      const env = mockEnv(async (model) => {
        if (model === TEXT_MODEL) {
          return { response: JSON.stringify({ prompt: "Driver", subjects: ["driver"] }) };
        }
        if (model === FLUX_ID) return { image: encodedJpeg };
        if (model === VISION_MODEL) {
          visionCalls += 1;
          return { description: "MISSING: none" };
        }
        throw new Error(`Unexpected model: ${model}`);
      });
      const response = await worker.fetch(
        request("/", {
          headers: { Authorization: AUTH },
          body: JSON.stringify({ prompt: "driver", verify: false }),
        }),
        env,
      );
      await expectStatus(response, 200);
      if (visionCalls !== 0 || response.headers.get("X-Verified") !== "skipped") {
        throw new Error("Verification was not skipped");
      }
    },
  ],
  [
    "verification punctuation does not trigger a retry",
    async () => {
      let fluxCalls = 0;
      const env = mockEnv(async (model) => {
        if (model === TEXT_MODEL) {
          return {
            response: JSON.stringify({ prompt: "Driver", subjects: ["driver"] }),
          };
        }
        if (model === FLUX_ID) {
          fluxCalls += 1;
          return { image: encodedJpeg };
        }
        if (model === VISION_MODEL) return { description: "MISSING: none." };
        throw new Error(`Unexpected model: ${model}`);
      });
      const response = await worker.fetch(
        request("/", {
          headers: { Authorization: AUTH },
          body: JSON.stringify({ prompt: "driver" }),
        }),
        env,
      );
      await expectStatus(response, 200);
      if (fluxCalls !== 1 || response.headers.get("X-Verified") !== "pass") {
        throw new Error(
          `Trailing punctuation was read as a missing subject (fluxCalls=${fluxCalls}, verified=${response.headers.get("X-Verified")})`,
        );
      }
    },
  ],
  [
    "retry prompt leads with the missing subject",
    async () => {
      const fluxPrompts = [];
      let visionCalls = 0;
      const env = mockEnv(async (model, input) => {
        if (model === TEXT_MODEL) {
          return {
            response: JSON.stringify({
              prompt: "A driver in a blue car",
              subjects: ["driver"],
            }),
          };
        }
        if (model === FLUX_ID) {
          fluxPrompts.push(input.prompt);
          return { image: encodedJpeg };
        }
        if (model === VISION_MODEL) {
          visionCalls += 1;
          return {
            description: visionCalls === 1 ? "MISSING: driver" : "MISSING: none",
          };
        }
        throw new Error(`Unexpected model: ${model}`);
      });
      await worker.fetch(
        request("/", {
          headers: { Authorization: AUTH },
          body: JSON.stringify({ prompt: "driver" }),
        }),
        env,
      );
      if (fluxPrompts.length !== 2) {
        throw new Error(`Expected 2 Flux calls, got ${fluxPrompts.length}`);
      }
      if (!fluxPrompts[1].startsWith("driver clearly visible in frame, ")) {
        throw new Error(`Retry prompt did not lead with the subject: ${fluxPrompts[1]}`);
      }
    },
  ],
  [
    "plan without subjects still improves the prompt",
    async () => {
      let fluxPrompt = "";
      const env = mockEnv(async (model, input) => {
        if (model === TEXT_MODEL) {
          return { response: JSON.stringify({ prompt: "A red car at night" }) };
        }
        if (model === FLUX_ID) {
          fluxPrompt = input.prompt;
          return { image: encodedJpeg };
        }
        throw new Error(`Unexpected model: ${model}`);
      });
      const response = await worker.fetch(
        request("/", {
          headers: { Authorization: AUTH },
          body: JSON.stringify({ prompt: "car" }),
        }),
        env,
      );
      await expectStatus(response, 200);
      if (fluxPrompt !== "A red car at night") {
        throw new Error(`Planner prompt was discarded: ${fluxPrompt}`);
      }
      if (response.headers.get("X-Verified") !== "skipped") {
        throw new Error("Verification should be skipped without subjects");
      }
    },
  ],
  [
    "long plan prompt is trimmed at a word boundary",
    async () => {
      const words = Array.from({ length: 100 }, (_, index) => `word${index}`);
      let fluxPrompt = "";
      const env = mockEnv(async (model, input) => {
        if (model === TEXT_MODEL) {
          return {
            response: JSON.stringify({ prompt: words.join(" "), subjects: [] }),
          };
        }
        if (model === FLUX_ID) {
          fluxPrompt = input.prompt;
          return { image: encodedJpeg };
        }
        throw new Error(`Unexpected model: ${model}`);
      });
      await worker.fetch(
        request("/", {
          headers: { Authorization: AUTH },
          body: JSON.stringify({ prompt: "words" }),
        }),
        env,
      );
      const count = fluxPrompt.split(/\s+/).length;
      if (count !== 60 || !fluxPrompt.endsWith("word59")) {
        throw new Error(`Prompt was not trimmed cleanly (${count} words): ${fluxPrompt}`);
      }
    },
  ],
  [
    "oversized request bodies are rejected",
    async () => {
      const env = mockEnv(async () => ({ image: encodedJpeg }));
      const response = await worker.fetch(
        request("/", {
          headers: { Authorization: AUTH, "content-length": "5000" },
          body: JSON.stringify({ prompt: "x".repeat(5000) }),
        }),
        env,
      );
      await expectStatus(response, 413);
      const body = await jsonBody(response);
      if (body.error !== "Request body too large") {
        throw new Error(`Unexpected error body: ${JSON.stringify(body)}`);
      }
    },
  ],
  [
    "anime style reaches Flux",
    async () => {
      let fluxPrompt = "";
      const env = mockEnv(async (model, input) => {
        if (model === TEXT_MODEL) {
          return { response: JSON.stringify({ prompt: "A cat", subjects: [] }) };
        }
        if (model === FLUX_ID) {
          fluxPrompt = input.prompt;
          return { image: encodedJpeg };
        }
        throw new Error(`Unexpected model: ${model}`);
      });
      const response = await worker.fetch(
        request("/", {
          headers: { Authorization: AUTH },
          body: JSON.stringify({ prompt: "A cat", style: "anime" }),
        }),
        env,
      );
      await expectStatus(response, 200);
      if (!fluxPrompt.includes("anime style")) {
        throw new Error("Anime style keyword was not sent to Flux");
      }
    },
  ],
];

let failed = 0;
for (const [name, test] of cases) {
  if (!(await runCase(name, test))) failed += 1;
}
if (failed > 0) process.exitCode = 1;
