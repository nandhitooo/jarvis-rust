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
