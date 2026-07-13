#!/usr/bin/env bash
# apex b1 check: semantic document model + the real on-device embedding
# substrate it depends on.
# Verifies the document model produces structural deltas with localized churn,
# raw paragraph text never leaves the module, the curated semantic embedding
# model is wired with a verified checksum, the local embedding provider is
# mode-aware, retrieval re-embedding is non-fatal, and the perception poll +
# snapshot are wired. No external API calls or model downloads are required.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
DOC_MODEL_RS="$SRC/document_model.rs"
LOCAL_RUNTIME_RS="$SRC/local_runtime.rs"
LOCAL_PROVIDER_RS="$SRC/providers/local.rs"
RETRIEVAL_RS="$SRC/retrieval.rs"
CONTEXT_OBSERVER_RS="$SRC/context_observer.rs"
AWARENESS_RS="$SRC/awareness_core.rs"
STATE_RS="$SRC/state.rs"
MAIN_RS="$SRC/main.rs"
COMMANDS_RS="$SRC/commands.rs"
MODELS_RS="$SRC/models.rs"
APP_TSX="$DESKTOP/src/App.tsx"
TAURI_CLIENT_TS="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex b1 semantic document model check ---"

# 1. document model core.
test -f "$DOC_MODEL_RS" || fail "document_model.rs missing"
grep -q "pub struct DocumentModel" "$DOC_MODEL_RS" || fail "DocumentModel missing"
grep -q "pub struct DocumentDelta" "$DOC_MODEL_RS" || fail "DocumentDelta missing"
grep -q "pub struct DocumentStateSummary" "$DOC_MODEL_RS" || fail "DocumentStateSummary missing"
grep -q "pub fn observe" "$DOC_MODEL_RS" || fail "observe() missing"
grep -q "churn_map" "$DOC_MODEL_RS" || fail "churn map missing"
grep -q "structure_changed" "$DOC_MODEL_RS" || fail "structure_changed missing"
grep -q "fn segment_paragraphs" "$DOC_MODEL_RS" || fail "paragraph segmentation missing"
pass "document model core types and paragraph segmentation present"

# 2. privacy: raw paragraph text stays inside the module.
if grep -q "TaskStore" "$DOC_MODEL_RS"; then
  fail "document_model.rs references the store; raw text must not be persisted"
fi
if grep -q "eprintln!" "$DOC_MODEL_RS"; then
  fail "document_model.rs logs; raw paragraph text must not reach logs"
fi
# the exported summary must be counts-only: no String fields in its struct body.
SUMMARY_BLOCK=$(awk '/pub struct DocumentStateSummary/{flag=1} flag{print} /^}/{if(flag) exit}' "$DOC_MODEL_RS")
if printf "%s" "$SUMMARY_BLOCK" | grep -q "String"; then
  fail "DocumentStateSummary carries a String field; it must be counts-only"
fi
grep -q "raw paragraph text" "$DOC_MODEL_RS" || fail "document model raw-text contract note missing"
pass "raw paragraph text is contained; exported summary is counts-only"

# 3. step 0: curated semantic embedding substrate with a verified checksum.
grep -q "LOCAL_SEMANTIC_EMBEDDING_MODEL_ID" "$LOCAL_RUNTIME_RS" || fail "semantic embedding model id missing"
grep -q "CURATED_EMBEDDING_MODEL_URL" "$LOCAL_RUNTIME_RS" || fail "curated embedding url missing"
grep -q "fn semantic_embedding_available" "$LOCAL_RUNTIME_RS" || fail "semantic capability probe missing"
grep -q "download_curated_embedding_model" "$LOCAL_RUNTIME_RS" || fail "curated download helper missing"
SHA=$(grep -oE "[0-9a-f]{64}" "$LOCAL_RUNTIME_RS" | head -1 || true)
[ -n "$SHA" ] || fail "curated embedding sha-256 not present or malformed"
pass "curated semantic embedding model wired with a 64-hex checksum ($SHA)"

# 4. mode-aware provider: model_id and embed_text gate on the same capability.
grep -q "semantic_embedding_available" "$LOCAL_PROVIDER_RS" || fail "provider does not gate on semantic capability"
grep -q "LOCAL_SEMANTIC_EMBEDDING_MODEL_ID" "$LOCAL_PROVIDER_RS" || fail "provider model_id does not report semantic id"
grep -q "mark_embedding_capability_stale" "$LOCAL_PROVIDER_RS" || fail "provider does not self-heal on sidecar failure"
pass "local embedding provider is mode-aware and self-consistent"

# 5. retrieval re-embed is non-fatal.
grep -q "retrieval_reembed_skipped" "$RETRIEVAL_RS" || fail "re-embed failure is not handled non-fatally"
pass "retrieval tolerates a transient re-embed failure"

# 6. perception + snapshot wiring; 80-char comparison retired.
grep -q "document_model: Arc<Mutex<crate::document_model::DocumentModel>>" "$STATE_RS" \
  || fail "document model not held in JeffState"
grep -q "mod document_model;" "${MAIN_RS%/*}/lib.rs" || fail "document_model module not registered"
grep -q "dm.observe(task_id" "${MAIN_RS%/*}/app_polls.rs" || fail "poll loop does not drive the document model"
grep -q "document_structure_changed" "$CONTEXT_OBSERVER_RS" || fail "structural signal field missing"
grep -q "structure changed" "$AWARENESS_RS" || fail "snapshot excerpt not enriched with structure signal"
if grep -q "chars().take(80)" "$CONTEXT_OBSERVER_RS"; then
  fail "first-80-char comparison still present in context_observer.rs"
fi
pass "document model wired into perception + snapshot; 80-char diff retired"

# 7. commands + frontend surface.
grep -q "download_curated_embedding_model" "$COMMANDS_RS" || fail "curated download command missing"
grep -q "commands::download_curated_embedding_model" "$MAIN_RS" || fail "curated download command not registered"
grep -q "semantic_embedding_available" "$MODELS_RS" || fail "status DTO lacks semantic availability"
grep -q "downloadCuratedEmbeddingModel" "$TAURI_CLIENT_TS" || fail "frontend curated download missing"
grep -q "embedding_mode" "$TAURI_CLIENT_TS" || fail "frontend DTO lacks embedding mode"
grep -q "embedding-mode-status" "$APP_TSX" || fail "Privacy Center embedding mode UI missing"
grep -q "local-model-download-semantic-embedding" "$APP_TSX" || fail "one-click semantic download UI missing"
pass "commands and Privacy Center embedding surface wired"

# 8. behavioral: b1 tests, clean compile, frontend checks.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

B1_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test b1_ --quiet 2>&1)
echo "$B1_TEST_OUT" | grep -q "test result: ok" || { echo "$B1_TEST_OUT"; fail "b1-focused tests failed"; }
echo "$B1_TEST_OUT" | grep -q "FAILED" && { echo "$B1_TEST_OUT"; fail "b1-focused tests failed"; }
pass "b1 churn/structure/perf/re-embed/catalog tests pass"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend TypeScript check did not run"; }
pass "frontend TypeScript check passes"

FRONTEND_TEST_OUT=$(cd "$DESKTOP" && npm test -- --run 2>&1)
echo "$FRONTEND_TEST_OUT" | grep -qE "Tests +[0-9]+ passed" || { echo "$FRONTEND_TEST_OUT"; fail "frontend tests did not run"; }
if echo "$FRONTEND_TEST_OUT" | grep -qE "[1-9][0-9]* failed"; then
  echo "$FRONTEND_TEST_OUT"
  fail "frontend tests failed"
fi
pass "frontend tests pass"

echo "--- apex b1 check passed ---"
