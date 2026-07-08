# Jeff Local Runtime

Apex A3 uses a llama.cpp-compatible `llama-server` sidecar for local Reflex
reasoning when a GGUF model is installed. The sidecar is chosen because it is
portable across Apple Silicon and other developer machines, exposes OpenAI-like
chat and embedding endpoints, and can be managed as a normal child process.

Models live under the app data directory:

- `models/reflex-instruct.gguf` for local Reflex reasoning.
- `models/embedding.gguf` for a sidecar embedding model when available.

The app also includes deterministic on-device fallbacks:

- Reflex classification falls back to a local rules classifier when no local
  sidecar/model is installed and no cloud fallback key is available.
- Embeddings fall back to a normalized local hash embedding model id
  `local-hash-embedding-v1`.

Model downloads must provide a SHA-256 digest. The downloader writes to a
temporary `.part` file, verifies the digest, then atomically moves the verified
model into `models/`. When an expected size is provided, Jeff checks available
disk with 256 MB headroom before downloading and aborts if the download exceeds
the expected size.

Set `JEFF_LOCAL_LLAMACPP_SERVER` to the `llama-server` executable path when it
is not installed in `/opt/homebrew/bin`, `/usr/local/bin`, `/usr/bin`, or the
process `PATH`. Optional `JEFF_LOCAL_RUNTIME_PORT` and
`JEFF_LOCAL_LLAMACPP_ARGS` override the default local port and server arguments.
