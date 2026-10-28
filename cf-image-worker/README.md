# Image generation Worker

Paste `worker.js` into the Cloudflare dashboard for `image-api` under **Edit code**.
Keep the `API_KEY` secret and bind the Workers AI binding as `AI`.
The model IDs are constants at the top of `worker.js`.

Send an authenticated JSON request:

```sh
curl -i https://image-api.<your-subdomain>.workers.dev/ \
  -H 'Authorization: Bearer <API_KEY>' \
  -H 'Content-Type: application/json' \
  -d '{"prompt":"A racing driver seated in a blue car","style":"cinematic"}'
```

The response contains the raw image bytes. `-i` displays response headers, including `X-Model`,
`X-Final-Prompt`, `X-Attempts`, and `X-Verified`.

## Request body

`prompt` is required; everything else is optional.

| Field     | Type    | Default | Meaning |
| --------- | ------- | ------- | ------- |
| `prompt`  | string  | —       | Image description, 1–500 characters. Required and must not be blank. |
| `style`   | string  | none    | Appends a style phrase to the prompt. Accepted: `photo`, `cinematic`, `anime`, `illustration`, `3d`. Any other value is ignored. |
| `seed`    | integer | random  | Fixes the image for a given prompt so results are repeatable. A verification retry reuses `seed + 1` (or a fresh random seed when none was given), so the retry is a different image instead of a copy of the first. |
| `enhance` | boolean | `true`  | `true` runs the text model first to rewrite your text into a denser image brief plus a list of subjects to verify. `false` sends your prompt to the image model unchanged. |
| `verify`  | boolean | `true`  | `true` asks the vision model whether every planned subject is visible and retries once when one is missing. `false` returns the first image without checking it. |

The whole request is capped at 55 seconds. If the planner or the verifier runs out of budget
the worker skips it and still returns an image: a failed planner keeps your original prompt, and
a failed verifier reports `X-Verified: unknown`.

```sh
curl -i https://image-api.<your-subdomain>.workers.dev/ \
  -H 'Authorization: Bearer <API_KEY>' \
  -H 'Content-Type: application/json' \
  -d '{"prompt":"A red fox in fresh snow","style":"photo","seed":42,"enhance":false}'
```

## Response headers

| Header           | Meaning |
| ---------------- | ------- |
| `Content-Type`   | `image/jpeg` or `image/png`, decided from the image's magic bytes. |
| `X-Model`        | Model that produced the image: `@cf/black-forest-labs/flux-1-schnell`, or `@cf/bytedance/stable-diffusion-xl-lightning` when Flux failed. |
| `X-Final-Prompt` | The prompt actually sent to the image model, percent-encoded (URI-encoded) because header values are ASCII-only. Truncated to 300 encoded characters, and only cut on a complete `%XX` escape, so percent-decoding always yields valid text. |
| `X-Attempts`     | Number of image generation attempts made, including failed model calls and the verification retry. `1` means the first attempt worked. |
| `X-Verified`     | How subject verification ended: `pass` (every subject visible), `skipped` (verification not run), `unknown` (vision model failed or answered unusably), `retry-pass` (visible only after regenerating), `missing` (still missing; the best image found is returned anyway). |

`X-Verified` is `skipped` when `verify` is `false`, when the planner found no subjects to check,
or when less than 25 seconds of the budget remain.

## Status codes

| Status | Body                     | When |
| ------ | ------------------------ | ---- |
| `200`  | image bytes              | Success. |
| `400`  | `{"error": "..."}`       | Invalid JSON, blank prompt, prompt over 500 characters, or a wrong field type. |
| `401`  | `{"error": "Unauthorized"}` | Missing or wrong `Authorization: Bearer <API_KEY>`. |
| `405`  | `{"error": "Method not allowed"}` | Anything other than `GET /health` or `POST /`. |
| `413`  | `{"error": "Request body too large"}` | Declared body size above 4096 bytes. |
| `502`  | `{"error": "Image generation failed"}` | Every model attempt failed or the budget ran out before the first image. |

`GET /health` needs no auth and returns exactly `{"ok":true}` — useful for uptime checks.

## Tests

`node test.mjs` runs the worker against mocked Workers AI responses; it needs no network and no
Cloudflare account.
