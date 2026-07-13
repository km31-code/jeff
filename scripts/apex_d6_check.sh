#!/usr/bin/env bash
# apex d6 check: steering, checkpoints/resume, fifo concurrency, standing jobs.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
RUNTIME="$SRC/agent_runtime.rs"
STORE="$SRC/store.rs"
COMMANDS="$SRC/commands.rs"
MAIN="$SRC/main.rs"
APP_TSX="$DESKTOP/src/App.tsx"
TAURI_CLIENT="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex d6 steering/checkpoints/standing jobs check ---"

# 1. Durable steering + checkpoint + standing-job schema and runtime seams.
test -f "$RUNTIME" || fail "agent_runtime.rs missing"
grep -q "CREATE TABLE IF NOT EXISTS job_steering" "$STORE" || fail "job_steering table missing"
grep -q "CREATE TABLE IF NOT EXISTS job_checkpoints" "$STORE" || fail "job_checkpoints table missing"
grep -q "CREATE TABLE IF NOT EXISTS standing_jobs" "$STORE" || fail "standing_jobs table missing"
grep -q "pub fn enqueue_job_steering" "$RUNTIME" || fail "steering enqueue missing"
grep -q "fn apply_pending_steering_at_boundary" "$RUNTIME" || fail "steering boundary application missing"
grep -q "fn create_job_checkpoint" "$RUNTIME" || fail "checkpoint persistence missing"
grep -q "pub fn resume_incomplete_jobs" "$RUNTIME" || fail "checkpoint resume missing"
grep -q "resumed_from_checkpoint" "$RUNTIME" || fail "resume status-stream event missing"
grep -q "pub fn cancel_job_preserving_checkpoints" "$RUNTIME" || fail "cancel-preserving-checkpoints missing"
grep -q "MAX_RUNNING_JOBS" "$RUNTIME" || fail "concurrency cap missing"
grep -q "fn promote_next_queued_job" "$RUNTIME" || fail "fifo queue promotion missing"
grep -q "pub fn create_standing_job" "$RUNTIME" || fail "standing job creation missing"
grep -q "pub fn run_due_standing_jobs" "$RUNTIME" || fail "standing job runner missing"
grep -q "pub fn set_standing_job_enabled" "$RUNTIME" || fail "standing job disable control missing"
grep -q "record_standing_job_run_receipt" "$RUNTIME" || fail "standing job receipt missing"
grep -q "STANDING_JOB_CRITICAL_EVENT_TYPE" "$RUNTIME" || fail "standing-job-critical crisis hook missing"
pass "steering, checkpoint/resume, fifo, and standing-job runtime seams are present"

# 2. main.rs runs the scheduler and startup-resume automatically, not just as
# manually-invokable commands (commands.rs references crate::agent_runtime::;
# these bare agent_runtime:: calls prove the background wiring in main.rs).
# apex f1a moved the startup-resume and standing-job scheduler out of the
# main.rs setup closure into core_runtime; main.rs starts the core.
CORE_RUNTIME="$SRC/core_runtime.rs"
grep -q "agent_runtime::resume_incomplete_jobs" "$CORE_RUNTIME" || fail "startup job resume not wired into core_runtime"
grep -q "agent_runtime::run_due_standing_jobs" "$CORE_RUNTIME" || fail "standing-job scheduler not wired into core_runtime"
grep -q "apex d6: standing-job scheduler" "$CORE_RUNTIME" || fail "standing-job scheduler task marker missing"
grep -q "apex d6: resume" "$CORE_RUNTIME" || fail "startup resume task marker missing"
pass "standing-job scheduler and startup resume run automatically in main.rs"

# 3. Commands and disable control wired end to end.
grep -q "pub fn send_job_steering" "$COMMANDS" || fail "send_job_steering command missing"
grep -q "pub fn cancel_agent_job" "$COMMANDS" || fail "cancel_agent_job command missing"
grep -q "pub fn resume_agent_jobs" "$COMMANDS" || fail "resume_agent_jobs command missing"
grep -q "pub fn create_standing_job" "$COMMANDS" || fail "create_standing_job command missing"
grep -q "pub fn set_standing_job_enabled" "$COMMANDS" || fail "set_standing_job_enabled command missing"
grep -q "commands::set_standing_job_enabled" "$MAIN" || fail "set_standing_job_enabled not registered"
grep -q "commands::run_due_standing_jobs" "$MAIN" || fail "run_due_standing_jobs not registered"
grep -q "setStandingJobEnabled" "$TAURI_CLIENT" || fail "frontend setStandingJobEnabled binding missing"
grep -q "sendJobSteering" "$TAURI_CLIENT" || fail "frontend sendJobSteering binding missing"
grep -q "standing-jobs-panel" "$APP_TSX" || fail "standing jobs panel missing"
grep -q "Next run:" "$APP_TSX" || fail "standing jobs next-run time not surfaced"
grep -q "standing-job-toggle" "$APP_TSX" || fail "standing job enable/disable control missing"
grep -q "job-steering-input" "$APP_TSX" || fail "job steering input missing"
pass "commands, scheduler control, and disable/steering UI are wired"

# 4. Behavioral: d6 runtime tests exercise steering-at-boundary, resume,
# fifo queueing, cancel-preserves-checkpoints, standing-run-with-receipt, and
# disable-stops-it.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

# the named behavioral tests must exist in the runtime source...
for t in \
  d6_steering_is_applied_at_step_boundary_and_checkpointed \
  d6_resume_continues_after_last_completed_checkpoint \
  d6_fourth_running_job_enters_fifo_queue \
  d6_cancel_preserves_checkpoints_and_partial_work \
  d6_standing_job_runs_through_job_model_with_receipt_and_crisis_hook \
  d6_disabling_standing_job_stops_it_from_running; do
  grep -q "fn $t" "$RUNTIME" || fail "expected d6 test $t is missing from agent_runtime.rs"
done
# ...and pass when run (quiet mode prints a pass/fail summary, not names).
D6_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test d6_ --quiet 2>&1)
echo "$D6_TEST_OUT" | grep -q "test result: ok" || { echo "$D6_TEST_OUT"; fail "d6 tests failed"; }
echo "$D6_TEST_OUT" | grep -q "FAILED" && { echo "$D6_TEST_OUT"; fail "d6 tests failed"; }
D6_PASSED=$(echo "$D6_TEST_OUT" | grep -oE "[0-9]+ passed" | awk '{s+=$1} END{print s+0}')
[ "$D6_PASSED" -ge 6 ] || { echo "$D6_TEST_OUT"; fail "expected >=6 d6 tests to run, saw $D6_PASSED"; }
pass "d6 steering/resume/fifo/cancel/standing/disable tests pass ($D6_PASSED passed)"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend TypeScript check did not run"; }
pass "frontend TypeScript check passes"

FRONTEND_TEST_OUT=$(cd "$DESKTOP" && npm test -- --run 2>&1)
echo "$FRONTEND_TEST_OUT" | grep -qE "Test Files.*passed" || { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
echo "$FRONTEND_TEST_OUT" | grep -qE "[0-9]+ failed" && { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
pass "frontend tests pass"

if [ "${JEFF_SKIP_ADJACENT_GATES:-0}" != "1" ]; then
  if ! ADJACENT_OUT=$(JEFF_SKIP_ADJACENT_GATES=1 bash "$ROOT_DIR/scripts/apex_d5_check.sh" 2>&1); then
    echo "$ADJACENT_OUT"
    fail "apex d5 agent runtime gate regressed"
  fi
  pass "apex d5 agent runtime gate still passes"
fi

echo "--- apex d6 check passed ---"
