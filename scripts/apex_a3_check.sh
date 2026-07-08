#!/usr/bin/env bash
# apex a3 check: local Reflex runtime and local embeddings.
# Verifies the llama.cpp sidecar manager, local provider routing, no-key Reflex
# classifier fallback, embedding model versioning, UI command wiring, and
# focused local-runtime tests. No external API calls are required.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
LOCAL_RUNTIME_RS="$SRC/local_runtime.rs"
LOCAL_PROVIDER_RS="$SRC/providers/local.rs"
MODEL_ROUTER_RS="$SRC/model_router.rs"
RETRIEVAL_RS="$SRC/retrieval.rs"
STORE_RS="$SRC/store.rs"
COMMANDS_RS="$SRC/commands.rs"
MAIN_RS="$SRC/main.rs"
APP_TSX="$DESKTOP/src/App.tsx"
TAURI_CLIENT_TS="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex a3 local runtime check ---"

test -f "$LOCAL_RUNTIME_RS" || fail "local_runtime.rs missing"
test -f "$LOCAL_PROVIDER_RS" || fail "providers/local.rs missing"
test -f "$ROOT_DIR/docs/LOCAL_RUNTIME.md" || fail "local runtime design note missing"
grep -q "llama.cpp server" "$LOCAL_RUNTIME_RS" || fail "llama.cpp sidecar choice missing"
grep -q "JEFF_LOCAL_LLAMACPP_SERVER" "$LOCAL_RUNTIME_RS" || fail "sidecar executable override missing"
grep -q "health_check" "$LOCAL_RUNTIME_RS" || fail "local runtime health check missing"
grep -q "fn start" "$LOCAL_RUNTIME_RS" || fail "local runtime start missing"
grep -q "fn stop" "$LOCAL_RUNTIME_RS" || fail "local runtime stop missing"
grep -q "download_model" "$LOCAL_RUNTIME_RS" || fail "checksum download manager missing"
grep -q "Sha256" "$LOCAL_RUNTIME_RS" || fail "SHA-256 verification missing"
grep -q "available_disk_bytes" "$LOCAL_RUNTIME_RS" || fail "disk-space check missing"
pass "local runtime sidecar lifecycle and model manager are present"

grep -q "ProviderKind::Local" "$MODEL_ROUTER_RS" || fail "local provider kind missing"
grep -q "provider: ProviderKind::Local" "$MODEL_ROUTER_RS" || fail "Reflex default is not local"
grep -q "classify_intent_locally" "$MODEL_ROUTER_RS" || fail "router does not use local Reflex classifier"
grep -q "model_router_fallback tier=reflex reason=local_unavailable" "$MODEL_ROUTER_RS" || fail "local sidecar fallback log missing"
grep -q "a3_classify_without_api_keys_uses_local_reflex" "$MODEL_ROUTER_RS" || fail "no-key Reflex test missing"
grep -q "LocalReasoningProvider" "$LOCAL_PROVIDER_RS" || fail "LocalReasoningProvider missing"
grep -q "LocalEmbeddingProvider" "$LOCAL_PROVIDER_RS" || fail "LocalEmbeddingProvider missing"
grep -q "hash_embedding" "$LOCAL_PROVIDER_RS" || fail "local hash embedding fallback missing"
pass "router and local providers are wired"

grep -q "fn model_id" "$SRC/providers.rs" || fail "embedding provider model_id missing"
grep -q "embedding_model TEXT" "$STORE_RS" || fail "embedding model DB column missing"
grep -q "update_chunk_embedding" "$STORE_RS" || fail "chunk re-embedding update method missing"
grep -q "chunk.embedding_model != active_embedding_model" "$RETRIEVAL_RS" || fail "lazy re-embedding check missing"
grep -q "a3_retrieval_reembeds_stale_embedding_model_on_touch" "$RETRIEVAL_RS" || fail "stale embedding re-embed test missing"
grep -q "a3_local_embedding_smoke_returns_same_seed_top1" "$RETRIEVAL_RS" || fail "local retrieval smoke test missing"
pass "local embedding versioning and retrieval migration are present"

grep -q "get_local_runtime_status" "$COMMANDS_RS" || fail "local runtime status command missing"
grep -q "start_local_runtime" "$COMMANDS_RS" || fail "local runtime start command missing"
grep -q "download_local_model" "$COMMANDS_RS" || fail "local model download command missing"
grep -q "commands::get_local_runtime_status" "$MAIN_RS" || fail "local runtime commands not registered"
grep -q "LocalRuntimeStatusDto" "$TAURI_CLIENT_TS" || fail "frontend local runtime DTO missing"
grep -q "downloadLocalModel" "$TAURI_CLIENT_TS" || fail "frontend download command missing"
grep -q "privacy-surface-local-runtime" "$APP_TSX" || fail "Privacy Center local runtime UI missing"
grep -q "local-runtime-start" "$APP_TSX" || fail "local runtime start UI control missing"
pass "commands and Privacy Center local runtime UI are wired"

CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

A3_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test a3_ --quiet 2>&1)
echo "$A3_TEST_OUT" | grep -q "test result: ok" || { echo "$A3_TEST_OUT"; fail "a3-focused tests failed"; }
echo "$A3_TEST_OUT" | grep -q "FAILED" && { echo "$A3_TEST_OUT"; fail "a3-focused tests failed"; }
pass "a3-focused Rust tests pass"

FRONTEND_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_OUT"; fail "frontend TypeScript check did not run"; }
pass "frontend TypeScript check passes"

echo "--- apex a3 check passed ---"
