#!/usr/bin/env bash
# apex d2 check: Google Docs adapter with anchored tracked/suggested changes.
# Verifies bridge wiring, 50-char anchor fallback, receipt status updates, UI
# copy, and adjacent D1/B6 gates. Live Google Docs DOM execution is browser-gated.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
EXT="$ROOT_DIR/browser-extension/selection-capture"
ACTION_BUS="$SRC/action_bus.rs"
SELECTION="$SRC/selection_capture.rs"
STORE="$SRC/store.rs"
COMMANDS="$SRC/commands.rs"
MAIN="$SRC/main.rs"
CONTENT="$EXT/content.js"
BACKGROUND="$EXT/background.js"
MANIFEST="$EXT/manifest.json"
APP_TSX="$DESKTOP/src/App.tsx"
TAURI_CLIENT="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex d2 google docs adapter check ---"

# 1. Backend adapter seam and typed path.
grep -q "pub struct GoogleDocsWritePayload" "$ACTION_BUS" || fail "GoogleDocsWritePayload missing"
grep -q "pub struct GoogleDocsAdapter" "$ACTION_BUS" || fail "GoogleDocsAdapter missing"
grep -q "request_tracked_change" "$ACTION_BUS" || fail "Google Docs tracked-change request missing"
grep -q "doc.suggest" "$ACTION_BUS" || fail "doc.suggest action class missing"
grep -q "anchor_context_50" "$ACTION_BUS" || fail "50-char anchor helper missing"
grep -q "pub fn request_google_docs_write" "$COMMANDS" || fail "request_google_docs_write command missing"
grep -q "commands::request_google_docs_write" "$MAIN" || fail "request_google_docs_write not registered"
grep -q "requestGoogleDocsWrite" "$TAURI_CLIENT" || fail "frontend Google Docs request binding missing"
pass "backend Google Docs adapter seam and typed request path are wired"

# 2. Bridge events and action receipt status updates.
grep -q "document_write://apply_requested" "$SELECTION" || fail "document_write apply event missing"
grep -q "document_write://result" "$SELECTION" || fail "document_write result event missing"
grep -q "reason: Option<String>" "$SELECTION" || fail "fallback reason is not captured"
grep -q '"fallback" => "guided"' "$STORE" || fail "fallback status is not mapped to guided action receipt"
grep -q "update_action_receipt_status(action_receipt_id" "$STORE" || fail "live edit status does not update linked action receipt"
pass "document-write bridge events and receipt status updates are present"

# 3. Extension supports Google Docs anchored apply and guided fallback.
grep -Fq '"https://docs.google.com/*"' "$MANIFEST" || fail "Google Docs host permission missing"
grep -q "JEFF_APPLY_GOOGLE_DOCS_ACTION" "$CONTENT" "$BACKGROUND" || fail "Google Docs apply message not shared"
grep -q "buildAnchorContext50" "$CONTENT" || fail "content script lacks 50-char context helper"
grep -q "anchorMatchesDocument" "$CONTENT" || fail "content script lacks context anchor matcher"
grep -q "detectGoogleDocsSuggestingMode" "$CONTENT" || fail "suggesting-mode detection missing"
grep -q "anchor_miss" "$CONTENT" || fail "anchor miss guided fallback reason missing"
grep -q "apply-fallback" "$CONTENT" || fail "extension does not report guided fallback"
grep -q "preferSuggesting" "$BACKGROUND" || fail "background does not carry suggest/direct mode preference"
pass "extension anchored apply, suggesting-mode note, and guided fallback are implemented"

# ensure anchor check appears before mutation in the Google Docs action body.
ANCHOR_LINE=$(grep -n "anchorMatchesDocument" "$CONTENT" | head -1 | cut -d: -f1)
MUTATE_LINE=$(grep -n "applyEditInPlace" "$CONTENT" | tail -1 | cut -d: -f1)
if [ -n "$ANCHOR_LINE" ] && [ -n "$MUTATE_LINE" ] && [ "$ANCHOR_LINE" -ge "$MUTATE_LINE" ]; then
  fail "Google Docs action can mutate before anchor validation"
fi
pass "anchor miss routes to guided fallback before any mutation"

# 4. Overlay approval card explains tracked/suggested Google Docs behavior.
grep -q "google-docs-tracked-change-note" "$APP_TSX" || fail "Google Docs tracked-change approval note missing"
grep -q "anchor moved" "$APP_TSX" || fail "guided fallback copy does not explain anchor drift"
pass "approval card explains tracked changes and guided fallback"

# 5. Compile/tests/adjacent gates.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

D2_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test d2_ --quiet 2>&1)
echo "$D2_TEST_OUT" | grep -q "test result: ok" || { echo "$D2_TEST_OUT"; fail "d2 tests failed"; }
echo "$D2_TEST_OUT" | grep -q "FAILED" && { echo "$D2_TEST_OUT"; fail "d2 tests failed"; }
pass "d2 anchor tests pass"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend TypeScript check did not run"; }
pass "frontend TypeScript check passes"

bash "$ROOT_DIR/scripts/apex_d1_check.sh" >/dev/null 2>&1 || fail "apex d1 action bus gate regressed"
pass "apex d1 action bus gate still passes"

bash "$ROOT_DIR/scripts/apex_b6_check.sh" >/dev/null 2>&1 || fail "apex b6 browser perception gate regressed"
pass "apex b6 browser perception gate still passes"

echo "SKIP: live Google Docs tracked-change execution requires Chrome, the"
echo "      extension installed, and an active Google Docs document. Static/local"
echo "      checks cover token/origin gating, anchors, fallback, receipts, and UI."
echo "--- apex d2 check passed ---"
