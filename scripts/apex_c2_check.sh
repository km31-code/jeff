#!/usr/bin/env bash
# apex c2 check: interruption ledger and focus depth.
# Verifies the ledger table + reaction capture, the focus-depth score and
# natural-boundary detector, the ledger summary + learned-hold in stage 2, the
# retired 300s guard and trigger_weight down-weighting, and the weekly
# self-audit surface. No external API calls required.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
STORE="$SRC/store.rs"
AWARE="$SRC/awareness_core.rs"
SYNTH="$SRC/synthesis.rs"
COMMANDS="$SRC/commands.rs"
MAIN="$SRC/main.rs"
APP_TSX="$DESKTOP/src/App.tsx"
TAURI_CLIENT="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex c2 interruption ledger and focus depth check ---"

# 1. ledger table + methods.
grep -q "CREATE TABLE IF NOT EXISTS interruption_ledger" "$STORE" || fail "interruption_ledger table missing"
grep -q "reaction TEXT" "$STORE" || fail "ledger reaction column missing"
grep -q "focus_score REAL" "$STORE" || fail "ledger focus_score column missing"
grep -q "pub fn record_interruption" "$STORE" || fail "record_interruption missing"
grep -q "pub fn record_interruption_reaction_within" "$STORE" || fail "reaction recorder missing"
grep -q "pub fn list_recent_interruptions" "$STORE" || fail "ledger reader missing"
grep -q "pub fn interruption_audit" "$STORE" || fail "self-audit query missing"
pass "interruption_ledger table, reaction capture, and audit query present"

# 2. focus depth + natural boundary.
grep -q "pub focus_score: f32" "$AWARE" || fail "snapshot focus_score field missing"
grep -q "pub fn compute_focus_score" "$AWARE" || fail "focus score computation missing"
grep -q "pub fn is_at_natural_boundary" "$AWARE" || fail "natural-boundary detector missing"
pass "focus-depth score and natural-boundary detector present"

# 3. retire the 300s guard and trigger_weight down-weighting on the live path.
grep -q "interim 300s delivery guard is retired" "$AWARE" || fail "300s guard not retired in stage 1"
grep -q "_last_delivered_at" "$AWARE" || fail "stage 1 still consumes the delivery gap"
if grep -q "DOWNWEIGHTED_RETURN_IDLE_THRESHOLD_SECONDS" "$AWARE"; then
  fail "trigger_weight down-weighting constant not retired"
fi
grep -q "trigger_weight down-weighting is retired" "$AWARE" || fail "trigger_weight retirement not documented"
pass "300s guard and trigger_weight down-weighting retired from the live path"

# 4. ledger drives stage 2: summary injected + learned hold + reaction capture.
grep -q "list_recent_interruptions" "$SYNTH" || fail "stage 2 does not read the ledger"
grep -q "fn build_ledger_summary" "$SYNTH" || fail "ledger summary builder missing"
grep -q "fn reason_band_is_ignored" "$SYNTH" || fail "learned-hold (ignored pattern) missing"
grep -q "fn fallback_verdict" "$SYNTH" || fail "deterministic hold/speak verdict missing"
grep -q "record_interruption(" "$SYNTH" || fail "deliveries are not recorded into the ledger"
grep -q "pub fn record_interruption_reaction_for_reply" "$SYNTH" || fail "reaction capture entry missing"
grep -q "record_interruption_reaction_for_reply" "$SRC/chat.rs" || fail "chat does not capture reactions"
grep -q "record_interruption_reaction_for_reply" "$SRC/chat_streaming.rs" \
  || fail "streaming chat does not capture reactions"
pass "ledger summary + learned hold in stage 2; reactions captured on reply"

# 5. weekly self-audit surface.
grep -q "pub fn get_interruption_audit" "$COMMANDS" || fail "self-audit command missing"
grep -q "commands::get_interruption_audit" "$MAIN" || fail "self-audit command not registered"
grep -q "InterruptionAuditDto" "$TAURI_CLIENT" || fail "frontend audit DTO missing"
grep -q "interruption-self-audit" "$APP_TSX" || fail "Privacy Center self-audit surface missing"
pass "weekly self-audit command and Privacy Center surface wired"

# 6. behavioral: clean compile, c2 tests, frontend, phase27/28.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

C2_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test c2_ --quiet 2>&1)
echo "$C2_TEST_OUT" | grep -q "test result: ok" || { echo "$C2_TEST_OUT"; fail "c2 tests failed"; }
echo "$C2_TEST_OUT" | grep -q "FAILED" && { echo "$C2_TEST_OUT"; fail "c2 tests failed"; }
pass "c2 focus-score, boundary, learned-hold, and reaction tests pass"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend TypeScript check did not run"; }
pass "frontend TypeScript check passes"

bash "$ROOT_DIR/scripts/phase27_check.sh" >/dev/null 2>&1 || fail "phase27 regressed"
bash "$ROOT_DIR/scripts/phase28_check.sh" >/dev/null 2>&1 || fail "phase28 regressed"
pass "phase27 and phase28 checks still pass"

echo "--- apex c2 check passed ---"
