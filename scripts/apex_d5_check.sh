#!/usr/bin/env bash
# apex d5 check: durable agent runtime core.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
RUNTIME="$SRC/agent_runtime.rs"
STORE="$SRC/store.rs"
MODELS="$SRC/models.rs"
COMMANDS="$SRC/commands.rs"
MAIN="$SRC/main.rs"
SUBTASK="$SRC/subtask.rs"
APP_TSX="$DESKTOP/src/App.tsx"
TAURI_CLIENT="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex d5 agent runtime check ---"

# 1. Runtime tables and DTO surface.
test -f "$RUNTIME" || fail "agent_runtime.rs missing"
grep -q "CREATE TABLE IF NOT EXISTS jobs" "$STORE" || fail "jobs table missing"
grep -q "CREATE TABLE IF NOT EXISTS job_steps" "$STORE" || fail "job_steps table missing"
grep -q "CREATE TABLE IF NOT EXISTS job_artifacts" "$STORE" || fail "job_artifacts table missing"
grep -q "CREATE TABLE IF NOT EXISTS job_events" "$STORE" || fail "job_events table missing"
grep -q "pub struct AgentJobDto" "$MODELS" || fail "AgentJobDto missing"
grep -q "pub struct AgentJobDetailDto" "$MODELS" || fail "AgentJobDetailDto missing"
pass "durable job tables and DTOs are present"

# 2. Executor loop, tool registry, budget, verification, and honest blocked delivery.
grep -q 'JOB_PHASES.*plan' "$RUNTIME" || fail "plan/act/observe/revise/verify/deliver phases missing"
grep -q "TOOL_LOCAL_RETRIEVAL" "$RUNTIME" || fail "local retrieval tool missing"
grep -q "TOOL_DOCUMENT_MODEL_READ" "$RUNTIME" || fail "document model read tool missing"
grep -q "TOOL_SNAPSHOT_READ" "$RUNTIME" || fail "snapshot read tool missing"
grep -q "TOOL_FILE_PROPOSAL_BUS" "$RUNTIME" || fail "file proposal bus tool missing"
grep -q "TOOL_ACTION_PROPOSAL_BUS" "$RUNTIME" || fail "action proposal bus tool missing"
grep -q "ROUTER_TOOL_CALL_PASSTHROUGH" "$RUNTIME" || fail "router tool-call passthrough seam missing"
grep -q "JobBudget" "$RUNTIME" || fail "job budget struct missing"
grep -q "finish_budget_exhausted" "$RUNTIME" || fail "budget exhaustion handler missing"
grep -q "fresh-context deterministic verification" "$RUNTIME" || fail "mandatory verification transcript missing"
grep -q "couldn't verify" "$RUNTIME" || fail "honest unverifiable-task wording missing"
grep -q "capability_request" "$RUNTIME" || fail "structured capability request missing"
grep -q "assessment_first" "$RUNTIME" || fail "assessment-first delivery event missing"
pass "executor loop, tools, budget, verification, and deliverable contract are implemented"

# 3. Commands and frontend routing.
grep -q "pub fn create_agent_job" "$COMMANDS" || fail "create_agent_job command missing"
grep -q "pub fn list_agent_jobs" "$COMMANDS" || fail "list_agent_jobs command missing"
grep -q "pub fn get_agent_job_detail" "$COMMANDS" || fail "get_agent_job_detail command missing"
grep -q "commands::create_agent_job" "$MAIN" || fail "create_agent_job not registered"
grep -q "interface AgentJobDto" "$TAURI_CLIENT" || fail "frontend AgentJobDto missing"
grep -q "createAgentJob" "$TAURI_CLIENT" "$APP_TSX" || fail "frontend createAgentJob binding/routing missing"
grep -q "agent-jobs-list" "$APP_TSX" || fail "workload jobs list missing"
grep -q "agent-job-detail" "$APP_TSX" || fail "job detail view missing"
grep -q "agent-job-verification" "$APP_TSX" || fail "verification transcript UI missing"
grep -q "agent-job-capability-request" "$APP_TSX" || fail "capability request UI missing"
grep -q "SUBTASK_CHAIN_RETIRED_BY_D5" "$SUBTASK" || fail "subtask chain retirement seam missing"
if grep -q "startSubtaskChain(" "$APP_TSX"; then
  fail "App still routes new delegated work through startSubtaskChain"
fi
pass "commands, UI, and subtask intent retirement are wired"

# 4. Compile/tests/adjacent gates.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

D5_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test d5_ --quiet 2>&1)
echo "$D5_TEST_OUT" | grep -q "test result: ok" || { echo "$D5_TEST_OUT"; fail "d5 tests failed"; }
echo "$D5_TEST_OUT" | grep -q "FAILED" && { echo "$D5_TEST_OUT"; fail "d5 tests failed"; }
pass "d5 agent runtime tests pass"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend TypeScript check did not run"; }
pass "frontend TypeScript check passes"

bash "$ROOT_DIR/scripts/apex_d4_check.sh" >/dev/null 2>&1 || fail "apex d4 trust ladder gate regressed"
pass "apex d4 trust ladder gate still passes"

echo "--- apex d5 check passed ---"
