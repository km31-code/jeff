#!/usr/bin/env bash
# apex b6 check: browser perception for Google Docs.
# Verifies the opt-in Chrome extension reader, token-gated content-observation
# bridge, Privacy Center mirror, and document-model parity with AX text. No
# external API calls required.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
EXT="$ROOT_DIR/browser-extension/selection-capture"
SELECTION_RS="$SRC/selection_capture.rs"
CONTEXT_RS="$SRC/context_observer.rs"
DOCUMENT_RS="$SRC/document_model.rs"
MODELS_RS="$SRC/models.rs"
COMMANDS_RS="$SRC/commands.rs"
STATE_RS="$SRC/state.rs"
APP_TSX="$DESKTOP/src/App.tsx"
TAURI_CLIENT_TS="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex b6 browser perception check ---"

# 1. Extension: disabled-by-default, per-site Google Docs reader.
test -f "$EXT/content.js" || fail "extension content.js missing"
test -f "$EXT/background.js" || fail "extension background.js missing"
test -f "$EXT/popup.html" || fail "extension popup.html missing"
test -f "$EXT/popup.js" || fail "extension popup.js missing"
grep -Fq '"https://docs.google.com/*"' "$EXT/manifest.json" || fail "docs.google.com not allowlisted"
grep -q '"default_popup": "popup.html"' "$EXT/manifest.json" || fail "extension action popup missing"
grep -q "JEFF_CONTENT_OBSERVATION_POLL_MS = 10_000" "$EXT/content.js" \
  || fail "10-second content-observation poll missing"
grep -q "jeffContentObservationSites" "$EXT/content.js" || fail "per-site storage missing in content script"
grep -q "document.visibilityState" "$EXT/content.js" || fail "active-tab visibility guard missing"
grep -q "/content-observation" "$EXT/content.js" || fail "content-observation POST missing"
grep -q "origin: window.location.origin" "$EXT/content.js" || fail "origin provenance missing"
grep -q "title: document.title" "$EXT/content.js" || fail "title provenance missing"
grep -q "captured_at" "$EXT/content.js" || fail "captured_at provenance missing"
grep -q "setJeffContentObservationSiteEnabled(false)" "$EXT/content.js" \
  || fail "backend privacy rejection does not stop extension polling"
grep -q "JEFF_SET_SITE_OBSERVATION_ENABLED" "$EXT/background.js" \
  || fail "background per-site toggle message missing"
grep -q "JEFF_CAPTURE_ACTIVE_SELECTION" "$EXT/popup.js" \
  || fail "popup no longer preserves manual selection capture"
pass "extension has opt-in active Google Docs reader, provenance, and per-site toggle"

# 2. Bridge route and backend privacy/raw-text boundary.
grep -q "BrowserContentObservationRequestDto" "$MODELS_RS" || fail "browser content observation DTO missing"
grep -q "BrowserContentObservationProvenanceDto" "$MODELS_RS" || fail "browser provenance DTO missing"
grep -q '"POST", "/content-observation"' "$SELECTION_RS" || fail "content-observation route missing"
grep -q "invalid browser content-observation bridge token" "$SELECTION_RS" \
  || fail "bridge token validation missing"
grep -q "get_content_observation_enabled(task_id)" "$SELECTION_RS" \
  || fail "Privacy Center content-observation gate missing"
grep -q "BROWSER_CONTENT_OBSERVATION_ALLOWED_ORIGINS" "$SELECTION_RS" \
  || fail "backend allowlist missing"
grep -q "observe_browser_document_model" "$SELECTION_RS" || fail "browser path does not update document model"
grep -q "apply_browser_content_observation_state" "$SELECTION_RS" \
  || fail "browser path does not update content-observation state"
grep -q "source_origin" "$CONTEXT_RS" || fail "browser provenance not retained in state"
grep -q "content_observation_source_origin" "$MODELS_RS" || fail "dashboard source origin missing"
grep -q "content_observation_source_origin" "$TAURI_CLIENT_TS" || fail "frontend source origin type missing"
grep -q "extension's per-site" "$APP_TSX" || fail "Privacy Center copy does not mention extension toggle"
grep -q "context_observer or the browser bridge" "$STATE_RS" || fail "state raw-text boundary note missing"
grep -q "raw paragraph text" "$DOCUMENT_RS" || fail "document model raw-text contract note missing"
pass "backend bridge is token-gated, privacy-gated, allowlisted, and raw-text bounded"

# 3. Behavioral gates.
grep -q "b6_content_observation_origin_is_allowlisted" "$SELECTION_RS" \
  || fail "allowlist test missing"
grep -q "b6_browser_docs_observation_matches_ax_document_model_churn" "$SELECTION_RS" \
  || fail "AX/browser document-model parity test missing"

CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

B6_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test b6_ --quiet 2>&1)
echo "$B6_TEST_OUT" | grep -q "test result: ok" || { echo "$B6_TEST_OUT"; fail "b6 tests failed"; }
echo "$B6_TEST_OUT" | grep -q "FAILED" && { echo "$B6_TEST_OUT"; fail "b6 tests failed"; }
pass "B6 origin, truncation, and AX/browser parity tests pass"

PHASE31_OUT=$(cd "$ROOT_DIR" && ./scripts/phase31_check.sh 2>&1)
echo "$PHASE31_OUT" | grep -q "phase 31 checks" || { echo "$PHASE31_OUT"; fail "phase31 check did not run"; }
echo "$PHASE31_OUT" | grep -q "FAIL" && { echo "$PHASE31_OUT"; fail "phase31 raw-text audit failed"; }
pass "phase31 raw-text audit passes"

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

echo "--- apex b6 check passed ---"
