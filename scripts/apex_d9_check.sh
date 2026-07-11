#!/usr/bin/env bash
# apex d9 check: self-extension (gap -> propose -> approve -> run/kill, rails).

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
SELF="$SRC/self_extend.rs"
STORE="$SRC/store.rs"
COMMANDS="$SRC/commands.rs"
MAIN="$SRC/main.rs"
TRUST="$SRC/trust.rs"
APP_TSX="$DESKTOP/src/App.tsx"
TAURI_CLIENT="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex d9 self-extension check ---"

# 1. Module, tables, lifecycle.
test -f "$SELF" || fail "self_extend.rs missing"
grep -q "CREATE TABLE IF NOT EXISTS capability_gaps" "$STORE" || fail "capability_gaps table missing"
grep -q "CREATE TABLE IF NOT EXISTS custom_tools" "$STORE" || fail "custom_tools table missing"
grep -q "pub fn record_capability_gap" "$SELF" || fail "gap detection missing"
grep -q "pub fn propose_tool_for_gap" "$SELF" || fail "tool proposal missing"
grep -q "pub fn approve_custom_tool" "$SELF" || fail "approve/install missing"
grep -q "pub fn kill_custom_tool" "$SELF" || fail "kill switch missing"
grep -q "pub fn run_custom_tool" "$SELF" || fail "tool run path missing"
grep -q "MIN_GAP_OCCURRENCES" "$SELF" || fail "recurring-gap threshold missing"
pass "self_extend module, tables, and gap->propose->approve->run/kill lifecycle present"

# 2. Safety rails (Part IV): L1 cap, sandbox guard, allowlist, kill->guided.
grep -q "HARD_CAP_TOOL_PREFIX" "$TRUST" || fail "tool.custom.* L1 hard cap missing in trust.rs"
grep -q "pub fn script_is_sandbox_safe" "$SELF" || fail "text_script static sandbox guard missing"
grep -q "pub fn run_text_script_code" "$SELF" || fail "confined subprocess runner missing"
grep -q "env_clear()" "$SELF" || fail "text_script subprocess does not strip env"
grep -q "pub fn tool_may_target" "$SELF" || fail "applescript allowlist rail missing"
grep -q "RUN_STATUS_GUIDED" "$SELF" || fail "guided fallback status missing"
pass "L1 cap, static sandbox guard + confined subprocess, allowlist, and guided fallback are enforced"

# 3. Gap recording wired at a real rejection point + commands registered.
grep -q "self_extend::record_capability_gap" "$COMMANDS" || fail "capability gap recording not wired into a rejection path"
for cmd in list_capability_gaps list_custom_tools propose_custom_tool approve_custom_tool kill_custom_tool run_custom_tool; do
  grep -q "pub fn $cmd" "$COMMANDS" || fail "$cmd command missing"
  grep -q "commands::$cmd" "$MAIN" || fail "$cmd not registered"
done
pass "gap recording is wired and self-extension commands are registered"

# 4. Privacy Center surface (gaps, tools, approve/kill).
grep -q "listCapabilityGaps" "$TAURI_CLIENT" || fail "frontend listCapabilityGaps binding missing"
grep -q "killCustomTool" "$TAURI_CLIENT" || fail "frontend killCustomTool binding missing"
grep -q "privacy-surface-self-extend" "$APP_TSX" || fail "Privacy Center self-extension surface missing"
grep -q "capability-gap-propose" "$APP_TSX" || fail "propose-tool control missing"
grep -q "custom-tool-approve" "$APP_TSX" || fail "approve control missing"
grep -q "custom-tool-kill" "$APP_TSX" || fail "kill-switch control missing"
pass "Privacy Center self-extension surface (gaps, propose, approve, kill) is wired"

# 5. Behavioral: lifecycle + rails covered by d9 tests.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

for t in \
  d9_recurring_gap_proposes_stages_and_dry_runs \
  d9_approve_then_run_produces_applied_receipt \
  d9_kill_switch_degrades_to_guided_fallback \
  d9_tool_custom_hard_capped_at_l1 \
  d9_sandbox_guard_refuses_network_and_escape \
  d9_applescript_allowlist_is_enforced; do
  grep -q "fn $t" "$SELF" || fail "expected d9 test $t is missing"
done
D9_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test d9_ --quiet 2>&1)
echo "$D9_TEST_OUT" | grep -q "test result: ok" || { echo "$D9_TEST_OUT"; fail "d9 tests failed"; }
echo "$D9_TEST_OUT" | grep -q "FAILED" && { echo "$D9_TEST_OUT"; fail "d9 tests failed"; }
D9_PASSED=$(echo "$D9_TEST_OUT" | grep -oE "[0-9]+ passed" | awk '{s+=$1} END{print s+0}')
[ "$D9_PASSED" -ge 7 ] || { echo "$D9_TEST_OUT"; fail "expected >=7 d9 tests to run, saw $D9_PASSED"; }
pass "d9 lifecycle + safety-rail tests pass ($D9_PASSED passed)"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend TypeScript check did not run"; }
pass "frontend TypeScript check passes"

FRONTEND_TEST_OUT=$(cd "$DESKTOP" && npm test -- --run 2>&1)
echo "$FRONTEND_TEST_OUT" | grep -qE "Test Files.*passed" || { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
echo "$FRONTEND_TEST_OUT" | grep -qE "[0-9]+ failed" && { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
pass "frontend tests pass"

bash "$ROOT_DIR/scripts/apex_d8_check.sh" >/dev/null 2>&1 || fail "apex d8 speculation gate regressed"
pass "apex d8 speculation gate still passes"

echo "--- apex d9 check passed ---"
