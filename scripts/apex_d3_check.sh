#!/usr/bin/env bash
# apex d3 check: native docs adapter, scripting-first with guided fallback.
# This is a deterministic local gate. Live Pages/Word execution remains macOS
# Apple Events/application gated.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
NATIVE="$SRC/native_docs.rs"
STORE="$SRC/store.rs"
MODELS="$SRC/models.rs"
COMMANDS="$SRC/commands.rs"
MAIN="$SRC/main.rs"
LIB="$SRC/lib.rs"
APP_TSX="$DESKTOP/src/App.tsx"
TAURI_CLIENT="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex d3 native docs adapter check ---"

# 1. Backend adapter seam, scripting dictionary paths, and default-off AX flag.
test -f "$NATIVE" || fail "native_docs.rs missing"
grep -q "NativeDocApp" "$NATIVE" || fail "NativeDocApp taxonomy missing"
grep -q "NativeDocsWriteRequest" "$NATIVE" || fail "NativeDocsWriteRequest missing"
grep -q "build_pages_script" "$NATIVE" || fail "Pages script builder missing"
grep -q "tell application \"Pages\"" "$NATIVE" || fail "Pages adapter is not AppleScript/ScriptingBridge-based"
grep -q "body text of front document" "$NATIVE" || fail "Pages adapter does not use Pages scripting dictionary body text"
grep -q "characters targetOffset thru targetEnd" "$NATIVE" || fail "Pages ranged replacement script missing"
grep -q "build_word_script" "$NATIVE" || fail "Word script builder missing"
grep -q "tell application \"Microsoft Word\"" "$NATIVE" || fail "Word adapter is not AppleScript/ScriptingBridge-based"
# Word uses an exact anchored range (create range + content of text object),
# not an ambiguous global find, so replacements land at the intended location.
grep -q "create range active document start" "$NATIVE" || fail "Word adapter does not use Word scripting dictionary ranged replacement"
grep -q "AX_BUFFER_WRITEBACK_ENABLED_KEY" "$NATIVE" || fail "AX fallback feature flag missing"
grep -q "unwrap_or(false)" "$NATIVE" || fail "AX fallback is not default off"
pass "native Pages/Word scripting adapters and default-off AX guard are present"

# 2. Guided fallback floor.
grep -q "FALLBACK_UNSUPPORTED_SURFACE" "$NATIVE" || fail "unsupported-app fallback reason missing"
grep -q "FALLBACK_ANCHOR_MISS" "$NATIVE" || fail "anchor-miss fallback reason missing"
grep -q "create_guided_receipt" "$NATIVE" || fail "guided fallback receipt path missing"
grep -q "create_guided_live_edit_receipt" "$STORE" "$NATIVE" || fail "guided live-edit card path missing"
grep -q "status IN ('pending_approval', 'fallback', 'failed')" "$STORE" || fail "guided cards are not surfaced as pending live edits"
grep -q "guided-apply-fallback" "$APP_TSX" || fail "guided fallback card missing from UI"
pass "unsupported apps and anchor drift route to guided apply receipts/cards"

# 3. Commands, models, and Privacy Center visibility.
grep -q "pub struct NativeDocsStatusDto" "$MODELS" || fail "NativeDocsStatusDto missing"
grep -q "pub native_docs: NativeDocsStatusDto" "$MODELS" || fail "Privacy dashboard lacks native_docs status"
grep -q "pub fn get_native_docs_status" "$COMMANDS" || fail "get_native_docs_status command missing"
grep -q "pub fn request_native_doc_write" "$COMMANDS" || fail "request_native_doc_write command missing"
grep -q "commands::get_native_docs_status" "$MAIN" || fail "get_native_docs_status not registered"
grep -q "commands::request_native_doc_write" "$MAIN" || fail "request_native_doc_write not registered"
grep -q "pub mod native_docs" "$LIB" || fail "native_docs not exposed to tests"
grep -q "requestNativeDocWrite" "$TAURI_CLIENT" || fail "frontend native-doc request binding missing"
grep -q "privacy-surface-native-docs" "$APP_TSX" || fail "Privacy Center native docs row missing"
grep -q "native-docs-automation-explainer" "$APP_TSX" || fail "automation explainer not shown in Privacy Center"
pass "native docs commands, models, bindings, and Privacy Center surface are wired"

# 4. Compile/tests/adjacent gates.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

D3_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test d3_ --quiet 2>&1)
echo "$D3_TEST_OUT" | grep -q "test result: ok" || { echo "$D3_TEST_OUT"; fail "d3 tests failed"; }
echo "$D3_TEST_OUT" | grep -q "FAILED" && { echo "$D3_TEST_OUT"; fail "d3 tests failed"; }
pass "d3 native-doc tests pass"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend TypeScript check did not run"; }
pass "frontend TypeScript check passes"

if [ "${JEFF_SKIP_ADJACENT_GATES:-0}" != "1" ]; then
  if ! ADJACENT_OUT=$(JEFF_SKIP_ADJACENT_GATES=1 bash "$ROOT_DIR/scripts/apex_d1_check.sh" 2>&1); then
    echo "$ADJACENT_OUT"
    fail "apex d1 action bus gate regressed"
  fi
  pass "apex d1 action bus gate still passes"

  if ! ADJACENT_OUT=$(JEFF_SKIP_ADJACENT_GATES=1 bash "$ROOT_DIR/scripts/apex_d2_check.sh" 2>&1); then
    echo "$ADJACENT_OUT"
    fail "apex d2 Google Docs gate regressed"
  fi
  pass "apex d2 Google Docs gate still passes"
fi

echo "SKIP: live Pages/Word execution requires macOS Apple Events permission,"
echo "      the target app installed, and an active document. Static/local checks"
echo "      cover scripting dictionary paths, revert script generation, receipts,"
echo "      guided fallback, default-off AX writeback, and UI surfacing."
echo "--- apex d3 check passed ---"
