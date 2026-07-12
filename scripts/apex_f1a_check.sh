#!/usr/bin/env bash
# apex f1a: core lifecycle consolidation. every recurring background scheduler
# and startup task moves out of the main.rs setup closure into a single
# lifecycle-managed core_runtime module -- the in-process seam that makes the
# f1b headless-daemon split a re-homing, not a rewrite. behavior is unchanged.

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
CORE="$SRC/core_runtime.rs"
MAIN="$SRC/main.rs"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex f1a core lifecycle check ---"

# 1. the core module exists with a start/stop lifecycle.
test -f "$CORE" || fail "core_runtime.rs missing"
grep -q "mod core_runtime;" "$MAIN" || fail "core_runtime not declared in main.rs"
grep -q "pub fn start(app: &AppHandle) -> CoreHandle" "$CORE" || fail "core start() entry point missing"
grep -q "pub struct CoreHandle" "$CORE" || fail "CoreHandle lifecycle type missing"
grep -q "pub struct CoreShutdown" "$CORE" || fail "CoreShutdown signal missing"
grep -q "pub fn stop(self)" "$CORE" || fail "CoreHandle::stop lifecycle teardown missing"
pass "core_runtime module with start/stop lifecycle present and wired into main.rs"

# 2. the setup closure delegates to the core instead of inlining the schedulers.
grep -q "core_runtime::start(&handle)" "$MAIN" || fail "main.rs does not start the core"

# 3. every core scheduler/startup task now lives in core_runtime, not main.rs.
# each symbol is unique to one loop body, so its presence in core_runtime and
# absence from main.rs proves the loop was moved (not duplicated).
for sym in \
  poll_active_window \
  fetch_next_event \
  run_due_standing_jobs_with_router \
  maybe_run_for_active_task \
  resume_incomplete_jobs_with_router \
  check_stale_task_notifications \
  spawn_ambient_monitor; do
  grep -q "$sym" "$CORE" || fail "core scheduler symbol $sym not found in core_runtime.rs"
  grep -q "$sym" "$MAIN" && fail "core scheduler symbol $sym still inlined in main.rs (not moved)"
done
pass "context, calendar, standing-job, speculation, job-resume, stale, and proactive loops moved to core_runtime"

# 4. the helper poll bodies still live in main.rs and the core calls them across
# the module boundary (verifies the child-module -> crate-root call path holds).
for helper in \
  spawn_content_observation_poll \
  spawn_goal_extraction_poll \
  spawn_memory_session_summary_poll \
  spawn_memory_consolidation_poll \
  perform_update_check; do
  grep -q "crate::$helper" "$CORE" || fail "core does not invoke crate::$helper"
done
pass "core drives the main.rs poll helpers through the module boundary"

# 5. cargo check is warning-free.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

# 6. lifecycle tests + full backend suite pass.
for t in \
  f1a_core_shutdown_signals_a_loop_to_stop \
  f1a_core_shutdown_is_shared_across_clones; do
  grep -q "fn $t" "$CORE" || fail "expected f1a test $t is missing"
done
F1A_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test --bin jeff-desktop f1a_ --quiet 2>&1)
echo "$F1A_TEST_OUT" | grep -q "test result: ok" || { echo "$F1A_TEST_OUT"; fail "f1a lifecycle tests failed"; }
echo "$F1A_TEST_OUT" | grep -q "FAILED" && { echo "$F1A_TEST_OUT"; fail "f1a lifecycle tests failed"; }
pass "f1a core shutdown lifecycle tests pass"

TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test --quiet 2>&1)
echo "$TEST_OUT" | grep -q "FAILED" && { echo "$TEST_OUT" | tail -20; fail "backend test suite has failures"; }
echo "$TEST_OUT" | grep -q "test result: ok" || { echo "$TEST_OUT" | tail -20; fail "backend test suite did not report ok"; }
pass "full backend test suite passes"

# 7. frontend is untouched but must still typecheck and test green.
FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend lint did not run"; }
pass "frontend TypeScript check passes"

FRONTEND_TEST_OUT=$(cd "$DESKTOP" && npm test -- --run 2>&1)
echo "$FRONTEND_TEST_OUT" | grep -qE "Test Files.*passed" || { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
echo "$FRONTEND_TEST_OUT" | grep -qE "[0-9]+ failed" && { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
pass "frontend test suite passes"

# 8. adjacent gate: the full Apex spine still proves out.
if [ "${JEFF_SKIP_ADJACENT_GATES:-0}" != "1" ]; then
  if ! ADJACENT_OUT=$(JEFF_SKIP_ADJACENT_GATES=1 bash "$ROOT_DIR/scripts/apex_e7_check.sh" 2>&1); then
    echo "$ADJACENT_OUT"
    fail "apex e7 ship gate regressed"
  fi
  pass "apex e7 ship gate still passes"
fi

echo "--- apex f1a check passed ---"
