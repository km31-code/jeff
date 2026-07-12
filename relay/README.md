# Jeff bundled inference relay

This is the opt-in non-local component used by bundled inference. It issues
24-hour installation-scoped tokens, enforces per-token and enrollment quotas,
pins clients to a server-selected model, and keeps the upstream provider key on
the relay. The desktop app stores only the scoped token in the OS keychain.

Required environment variables:

- `RELAY_TOKEN_SECRET`: at least 32 random characters; never ship it in the app.
- `OPENAI_API_KEY`: server-side upstream credential.
- `JEFF_BUNDLED_RELAY_URL`: baked into the desktop build or set at runtime.

Optional variables are `PORT`, `RELAY_OPENAI_MODEL`, and `RELAY_USAGE_LOG`.
Terminate TLS at the deployment platform; the desktop rejects non-HTTPS relay
URLs except loopback addresses. Run `npm test` before deployment.
