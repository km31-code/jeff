#!/usr/bin/env bash
# apex d4 check: trust ladder with hard caps, earned autonomy, and demotion.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
TRUST="$SRC/trust.rs"
ACTION_BUS="$SRC/action_bus.rs"
STORE="$SRC/store.rs"
MODELS="$SRC/models.rs"
COMMANDS="$SRC/commands.rs"
MAIN="$SRC/main.rs"
APP_TSX="$DESKTOP/src/App.tsx"
TAURI_CLIENT="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex d4 trust ladder check ---"

# 1. Trust table, DTO, and command surface.
test -f "$TRUST" || fail "trust.rs missing"
grep -q "CREATE TABLE IF NOT EXISTS trust_levels" "$STORE" || fail "trust_levels table missing"
grep -q "pub struct TrustLevelDto" "$MODELS" || fail "TrustLevelDto missing"
grep -q "pub trust_ladder: Vec<TrustLevelDto>" "$MODELS" || fail "Privacy dashboard lacks trust ladder"
grep -q "pub fn list_trust_ladder" "$COMMANDS" || fail "list_trust_ladder command missing"
grep -q "pub fn set_trust_level" "$COMMANDS" || fail "set_trust_level command missing"
grep -q "pub fn demote_trust_class" "$COMMANDS" || fail "demote_trust_class command missing"
grep -q "commands::list_trust_ladder" "$MAIN" || fail "list_trust_ladder not registered"
grep -q "commands::set_trust_level" "$MAIN" || fail "set_trust_level not registered"
grep -q "commands::demote_trust_class" "$MAIN" || fail "demote_trust_class not registered"
pass "trust table, DTO, commands, and dashboard field are wired"

# 2. Hard caps and explicit L3 boundary.
grep -q "HARD_CAP_ACTION_CLASSES" "$TRUST" || fail "compile-time hard cap list missing"
grep -q '"email.send"' "$TRUST" || fail "email.send hard cap missing"
grep -q '"file.delete"' "$TRUST" || fail "file.delete hard cap missing"
grep -q "HARD_CAP_TOOL_PREFIX" "$TRUST" || fail "tool.custom.* hard cap missing"
grep -q "assert_runtime_level_allowed" "$TRUST" || fail "runtime hard cap assertion missing"
grep -q "L3 trust can only be set by an explicit Privacy Center action" "$TRUST" || fail "L3 explicit-action guard missing"
grep -q "effective_level_for_action" "$TRUST" || fail "tamper-clamping effective level path missing"
pass "hard caps, tamper clamp, and explicit L3 guard are present"

# 3. Receipt outcome accounting and L2 bus path.
grep -q "TRUST_GRADUATION_STREAK: i64 = 10" "$TRUST" || fail "10-approval graduation threshold missing"
grep -q "record_receipt_outcome" "$TRUST" "$ACTION_BUS" || fail "receipt outcomes are not wired to trust ladder"
grep -q "demote_trust_class(store, &receipt.class)" "$TRUST" || fail "revert demotion path missing"
grep -q "execute_file_write_trusted" "$ACTION_BUS" || fail "L2 trusted bus execution path missing"
grep -q "requires approval at L1" "$ACTION_BUS" || fail "L1 approval floor missing from trusted bus path"
pass "approval streaks, revert demotion, and L2 bus path are implemented"

# 4. Frontend Privacy Center controls.
grep -q "interface TrustLevelDto" "$TAURI_CLIENT" || fail "frontend TrustLevelDto missing"
grep -q "setTrustLevel" "$TAURI_CLIENT" "$APP_TSX" || fail "frontend trust level setter missing"
grep -q "demoteTrustClass" "$TAURI_CLIENT" "$APP_TSX" || fail "frontend demote command missing"
grep -q "privacy-surface-trust-ladder" "$APP_TSX" || fail "Privacy Center trust ladder row missing"
grep -q "trust-graduation-offer" "$APP_TSX" || fail "graduation offer UI missing"
grep -q "trust-explicit-l3" "$APP_TSX" || fail "explicit L3 Privacy Center control missing"
grep -q "trust-demote" "$APP_TSX" || fail "demote button missing"
pass "Privacy Center trust ladder controls are present"

# 5. Compile/tests/adjacent gates.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

D4_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test d4_ --quiet 2>&1)
echo "$D4_TEST_OUT" | grep -q "test result: ok" || { echo "$D4_TEST_OUT"; fail "d4 tests failed"; }
echo "$D4_TEST_OUT" | grep -q "FAILED" && { echo "$D4_TEST_OUT"; fail "d4 tests failed"; }
pass "d4 trust tests pass"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend TypeScript check did not run"; }
pass "frontend TypeScript check passes"

bash "$ROOT_DIR/scripts/apex_d1_check.sh" >/dev/null 2>&1 || fail "apex d1 action bus gate regressed"
pass "apex d1 action bus gate still passes"

bash "$ROOT_DIR/scripts/apex_d2_check.sh" >/dev/null 2>&1 || fail "apex d2 Google Docs gate regressed"
pass "apex d2 Google Docs gate still passes"

bash "$ROOT_DIR/scripts/apex_d3_check.sh" >/dev/null 2>&1 || fail "apex d3 native docs gate regressed"
pass "apex d3 native docs gate still passes"

echo "--- apex d4 check passed ---"
