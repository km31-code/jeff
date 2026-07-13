#!/usr/bin/env bash
# apex d8 check: speculation scheduler (read-only speculative jobs, cache,
# invalidation, budget, Privacy Center surface).

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
SPEC="$SRC/speculation.rs"
RUNTIME="$SRC/agent_runtime.rs"
STORE="$SRC/store.rs"
COMMANDS="$SRC/commands.rs"
MAIN="$SRC/main.rs"
MODELS="$SRC/models.rs"
APP_TSX="$DESKTOP/src/App.tsx"
TAURI_CLIENT="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex d8 speculation scheduler check ---"

# 1. Module, tables, and cache/serve/invalidation seams.
test -f "$SPEC" || fail "speculation.rs missing"
grep -q "CREATE TABLE IF NOT EXISTS speculation_cache" "$STORE" || fail "speculation_cache table missing"
grep -q "CREATE TABLE IF NOT EXISTS speculation_events" "$STORE" || fail "speculation_events table missing"
grep -q "pub fn normalized_signature" "$SPEC" || fail "request signature normalization missing"
grep -q "pub fn run_speculation_cycle" "$SPEC" || fail "speculation cycle missing"
grep -q "pub fn serve_speculation" "$SPEC" || fail "cache serving missing"
grep -q "pub fn invalidate_for_task" "$SPEC" || fail "document-delta invalidation missing"
grep -q "pub fn invalidate_stale" "$SPEC" || fail "ttl invalidation missing"
grep -q "already ran this while you were working" "$SPEC" || grep -q "precomputed: true" "$SPEC" || fail "precomputed marker missing"
pass "speculation module, tables, cache, serving, and invalidation are present"

# 2. Read-only invariant enforced at scheduler + runtime (Part IV).
grep -q "pub fn speculative_tool_registry" "$RUNTIME" || fail "speculative read-only tool registry missing"
grep -q "pub fn guard_speculative_action" "$RUNTIME" || fail "runtime mutation guard missing"
grep -q '"read_only": speculative' "$RUNTIME" || fail "speculative plans do not mark read_only"
grep -q "guard_speculative_action" "$SPEC" || fail "speculation cycle does not assert read-only invariant"
pass "speculative jobs are read-only at the scheduler and runtime boundaries"

# 3. Budget cap + daily cap gate scheduling.
grep -q "within_speculation_budget" "$SPEC" || fail "speculation sub-budget gate missing"
grep -q "SPECULATION_DAILY_PREDICTION_CAP" "$SPEC" || fail "daily prediction cap missing"
grep -q "SPECULATION_BUDGET_KEY" "$SPEC" || fail "speculation does not draw from its named sub-budget"
pass "speculation is gated by its sub-budget and a hard daily cap"

# 4. Scheduler wiring + document-delta invalidation in main.rs (automatic, not
# just manual commands; commands.rs uses crate::speculation::).
# apex f1a moved the speculation scheduler out of main.rs into core_runtime.
grep -q "speculation::maybe_run_for_active_task" "$SRC/core_runtime.rs" || fail "speculation scheduler not wired into core_runtime"
grep -q "speculation::invalidate_for_task" "${MAIN%/*}/app_polls.rs" || fail "document-delta invalidation not wired into main.rs"
pass "speculation scheduler and document-delta invalidation run automatically in main.rs"

# 5. Commands + Privacy Center surface (toggle, spend, hit-rate, cached list, discard).
grep -q "pub fn get_speculation_status" "$COMMANDS" || fail "get_speculation_status command missing"
grep -q "pub fn set_speculation_enabled" "$COMMANDS" || fail "set_speculation_enabled command missing"
grep -q "pub fn list_speculation_cache" "$COMMANDS" || fail "list_speculation_cache command missing"
grep -q "pub fn discard_speculation_cache_entry" "$COMMANDS" || fail "discard command missing"
grep -q "commands::set_speculation_enabled" "$MAIN" || fail "set_speculation_enabled not registered"
grep -q "commands::discard_speculation_cache_entry" "$MAIN" || fail "discard not registered"
grep -q "pub speculation: SpeculationStatusDto" "$MODELS" || fail "dashboard lacks speculation status"
grep -q "setSpeculationEnabled" "$TAURI_CLIENT" || fail "frontend setSpeculationEnabled binding missing"
grep -q "discardSpeculationCacheEntry" "$TAURI_CLIENT" || fail "frontend discard binding missing"
grep -q "privacy-surface-speculation" "$APP_TSX" || fail "Privacy Center speculation surface missing"
grep -q "privacy-toggle-speculation" "$APP_TSX" || fail "speculation toggle missing"
grep -q "speculation-discard" "$APP_TSX" || fail "speculation discard control missing"
pass "speculation commands and Privacy Center surface (toggle/spend/hit-rate/discard) are wired"

# 6. Behavioral: read-only enforcement, cache-hit serving, delta invalidation,
# ttl, budget/daily cap all covered by d8 tests.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

for t in \
  d8_speculative_job_is_read_only_and_rejects_mutations \
  d8_cache_hit_serves_and_marks_precomputed \
  d8_document_delta_invalidates_cache_entry \
  d8_stale_entries_invalidated_after_ttl \
  d8_budget_and_daily_cap_gate_scheduling; do
  grep -qR "fn $t" "$SRC" || fail "expected d8 test $t is missing"
done
D8_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test d8_ --quiet 2>&1)
echo "$D8_TEST_OUT" | grep -q "test result: ok" || { echo "$D8_TEST_OUT"; fail "d8 tests failed"; }
echo "$D8_TEST_OUT" | grep -q "FAILED" && { echo "$D8_TEST_OUT"; fail "d8 tests failed"; }
D8_PASSED=$(echo "$D8_TEST_OUT" | grep -oE "[0-9]+ passed" | awk '{s+=$1} END{print s+0}')
[ "$D8_PASSED" -ge 9 ] || { echo "$D8_TEST_OUT"; fail "expected >=9 d8 tests to run, saw $D8_PASSED"; }
pass "d8 read-only/serve/invalidate/ttl/budget tests pass ($D8_PASSED passed)"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend TypeScript check did not run"; }
pass "frontend TypeScript check passes"

FRONTEND_TEST_OUT=$(cd "$DESKTOP" && npm test -- --run 2>&1)
echo "$FRONTEND_TEST_OUT" | grep -qE "Test Files.*passed" || { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
echo "$FRONTEND_TEST_OUT" | grep -qE "[0-9]+ failed" && { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
pass "frontend tests pass"

if [ "${JEFF_SKIP_ADJACENT_GATES:-0}" != "1" ]; then
  if ! ADJACENT_OUT=$(JEFF_SKIP_ADJACENT_GATES=1 bash "$ROOT_DIR/scripts/apex_d7_check.sh" 2>&1); then
    echo "$ADJACENT_OUT"
    fail "apex d7 agent eval gate regressed"
  fi
  pass "apex d7 agent eval gate still passes"
fi

echo "--- apex d8 check passed ---"
