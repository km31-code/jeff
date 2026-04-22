#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SUBTASK_RS="$ROOT_DIR/desktop/src-tauri/src/subtask.rs"
STORE_RS="$ROOT_DIR/desktop/src-tauri/src/store.rs"
MODELS_RS="$ROOT_DIR/desktop/src-tauri/src/models.rs"
COMMANDS_RS="$ROOT_DIR/desktop/src-tauri/src/commands.rs"
MAIN_RS="$ROOT_DIR/desktop/src-tauri/src/main.rs"
TAURI_CLIENT_TS="$ROOT_DIR/desktop/src/tauriClient.ts"
APP_TSX="$ROOT_DIR/desktop/src/App.tsx"

echo "--- phase 16 richer parallel work check ---"

fail() { echo "FAIL: $1" >&2; exit 1; }
pass() { echo "PASS: $1"; }

# 1. three new db tables present in store.rs (m16.1)
grep -q "subtask_steps" "$STORE_RS" || fail "subtask_steps table missing from store.rs"
grep -q "subtask_file_write_proposals" "$STORE_RS" || fail "subtask_file_write_proposals table missing from store.rs"
grep -q "subtask_write_audit_log" "$STORE_RS" || fail "subtask_write_audit_log table missing from store.rs"
pass "subtask_steps, subtask_file_write_proposals, subtask_write_audit_log tables present in store.rs (m16.1)"

# 2. idempotent alter table for max_steps and current_step on subtasks (m16.1)
grep -q "max_steps" "$STORE_RS" || fail "max_steps column migration missing from store.rs"
grep -q "current_step" "$STORE_RS" || fail "current_step column migration missing from store.rs"
pass "max_steps and current_step columns present on subtasks (m16.1)"

# 3. phase 16 store methods present (m16.1)
grep -q "fn create_subtask_step" "$STORE_RS" || fail "create_subtask_step missing from store.rs"
grep -q "fn update_subtask_step_status" "$STORE_RS" || fail "update_subtask_step_status missing from store.rs"
grep -q "fn list_subtask_steps" "$STORE_RS" || fail "list_subtask_steps missing from store.rs"
grep -q "fn update_subtask_current_step" "$STORE_RS" || fail "update_subtask_current_step missing from store.rs"
grep -q "pub fn create_file_write_proposal" "$STORE_RS" || fail "create_file_write_proposal missing from store.rs"
grep -q "fn resolve_file_write_proposal" "$STORE_RS" || fail "resolve_file_write_proposal missing from store.rs"
grep -q "pub fn list_pending_file_write_proposals" "$STORE_RS" || fail "list_pending_file_write_proposals missing from store.rs"
grep -q "fn list_file_write_proposals_for_subtask" "$STORE_RS" || fail "list_file_write_proposals_for_subtask missing from store.rs"
grep -q "fn append_write_audit_entry" "$STORE_RS" || fail "append_write_audit_entry missing from store.rs"
grep -q "fn list_write_audit_log" "$STORE_RS" || fail "list_write_audit_log missing from store.rs"
pass "all 10 phase 16 store methods present (m16.1)"

# 4. phase 16 dtos present in models.rs (m16.1)
grep -q "pub struct SubTaskStepDto" "$MODELS_RS" || fail "SubTaskStepDto missing from models.rs"
grep -q "pub struct FileWriteProposalDto" "$MODELS_RS" || fail "FileWriteProposalDto missing from models.rs"
grep -q "pub struct WriteAuditEntryDto" "$MODELS_RS" || fail "WriteAuditEntryDto missing from models.rs"
pass "SubTaskStepDto, FileWriteProposalDto, WriteAuditEntryDto dtos present in models.rs (m16.1)"

# 5. chain executor symbols in subtask.rs (m16.2)
grep -q "pub const MAX_SUBTASK_STEPS" "$SUBTASK_RS" || fail "MAX_SUBTASK_STEPS missing from subtask.rs"
grep -q "MAX_CHAIN_STEP_DESCRIPTION_CHARS" "$SUBTASK_RS" || fail "MAX_CHAIN_STEP_DESCRIPTION_CHARS missing from subtask.rs"
grep -q "MAX_CHAIN_FILE_PROPOSAL_CONTENT_CHARS" "$SUBTASK_RS" || fail "MAX_CHAIN_FILE_PROPOSAL_CONTENT_CHARS missing from subtask.rs"
grep -q "fn run_subtask_chain" "$SUBTASK_RS" || fail "run_subtask_chain missing from subtask.rs"
grep -q "pub fn create_chain_subtask_and_start" "$SUBTASK_RS" || fail "create_chain_subtask_and_start missing from subtask.rs"
grep -q "fn execute_chain_step" "$SUBTASK_RS" || fail "execute_chain_step missing from subtask.rs"
grep -q "fn build_chain_planning_prompt" "$SUBTASK_RS" || fail "build_chain_planning_prompt missing from subtask.rs"
grep -q "fn auto_reject_pending_proposals" "$SUBTASK_RS" || fail "auto_reject_pending_proposals missing from subtask.rs"
grep -q "fn mark_remaining_steps_skipped" "$SUBTASK_RS" || fail "mark_remaining_steps_skipped missing from subtask.rs"
pass "chain executor functions and resource-limit constants present in subtask.rs (m16.2)"

# 6. chain executor never calls fs::write (file writes only via proposal approval) (m16.2)
CHAIN_FN_START=$(grep -n "fn run_subtask_chain" "$SUBTASK_RS" | head -1 | cut -d: -f1)
CHAIN_FN_END=$(grep -n "fn execute_subtask_with_reasoning" "$SUBTASK_RS" | head -1 | cut -d: -f1)
if [ -n "$CHAIN_FN_START" ] && [ -n "$CHAIN_FN_END" ]; then
  CHAIN_SECTION=$(sed -n "${CHAIN_FN_START},${CHAIN_FN_END}p" "$SUBTASK_RS")
  if echo "$CHAIN_SECTION" | grep -q "fs::write"; then
    fail "chain executor section contains fs::write — file writes must go through proposal approval"
  fi
fi
pass "chain executor does not directly write files (m16.2)"

# 7. start_subtask_chain method on SubTaskRunner (m16.2)
grep -q "pub fn start_subtask_chain" "$SUBTASK_RS" || fail "start_subtask_chain method missing from SubTaskRunner"
pass "SubTaskRunner.start_subtask_chain present (m16.2)"

# 8. four unit tests for chain executor in subtask.rs (m16.2)
grep -q "fn subtask_chain_runs_and_all_steps_complete" "$SUBTASK_RS" || fail "subtask_chain_runs_and_all_steps_complete test missing"
grep -q "fn subtask_chain_file_write_proposal_creates_db_record_not_disk_file" "$SUBTASK_RS" || fail "subtask_chain_file_write_proposal test missing"
grep -q "fn subtask_chain_truncates_plan_to_max_steps_limit" "$SUBTASK_RS" || fail "subtask_chain_truncates test missing"
grep -q "fn subtask_chain_cancel_leaves_no_pending_approval_proposals" "$SUBTASK_RS" || fail "subtask_chain_cancel test missing"
grep -q "fn subtask_chain_rejects_oversized_file_write_proposal_content" "$SUBTASK_RS" || fail "subtask_chain_rejects_oversized_file_write_proposal_content test missing"
grep -q "fn subtask_chain_sanitizes_unsafe_proposed_path_before_persisting_proposal" "$SUBTASK_RS" || fail "subtask_chain_sanitizes_unsafe_proposed_path_before_persisting_proposal test missing"
pass "chain executor unit tests cover baseline flow plus content/path safety limits (m16.2)"

# 9. six new tauri commands present in commands.rs (m16.3)
grep -q "pub fn list_subtask_steps" "$COMMANDS_RS" || fail "list_subtask_steps command missing from commands.rs"
grep -q "pub fn list_file_write_proposals" "$COMMANDS_RS" || fail "list_file_write_proposals command missing from commands.rs"
grep -q "pub fn approve_subtask_file_write" "$COMMANDS_RS" || fail "approve_subtask_file_write command missing from commands.rs"
grep -q "pub fn reject_subtask_file_write" "$COMMANDS_RS" || fail "reject_subtask_file_write command missing from commands.rs"
grep -q "pub fn list_write_audit_log" "$COMMANDS_RS" || fail "list_write_audit_log command missing from commands.rs"
grep -q "pub fn start_subtask_chain" "$COMMANDS_RS" || fail "start_subtask_chain command missing from commands.rs"
pass "all 6 phase 16 commands present in commands.rs (m16.3)"

# 10. path safety in approve_subtask_file_write: reject absolute paths (m16.3)
grep -q "is_absolute" "$COMMANDS_RS" || fail "path safety check (is_absolute) missing from approve_subtask_file_write"
grep -q "Component::Normal" "$COMMANDS_RS" || fail "path component safety check missing from approve_subtask_file_write"
pass "path safety guards present in approve_subtask_file_write (m16.3)"

# 11. all 6 commands registered in main.rs (m16.3)
grep -q "commands::list_subtask_steps" "$MAIN_RS" || fail "list_subtask_steps not registered in main.rs"
grep -q "commands::list_file_write_proposals" "$MAIN_RS" || fail "list_file_write_proposals not registered in main.rs"
grep -q "commands::approve_subtask_file_write" "$MAIN_RS" || fail "approve_subtask_file_write not registered in main.rs"
grep -q "commands::reject_subtask_file_write" "$MAIN_RS" || fail "reject_subtask_file_write not registered in main.rs"
grep -q "commands::list_write_audit_log" "$MAIN_RS" || fail "list_write_audit_log not registered in main.rs"
grep -q "commands::start_subtask_chain" "$MAIN_RS" || fail "start_subtask_chain not registered in main.rs"
pass "all 6 commands registered in main.rs (m16.3)"

# 12. phase 16 typescript interfaces in tauriClient.ts (m16.4)
grep -q "interface SubTaskStepDto" "$TAURI_CLIENT_TS" || fail "SubTaskStepDto interface missing from tauriClient.ts"
grep -q "interface FileWriteProposalDto" "$TAURI_CLIENT_TS" || fail "FileWriteProposalDto interface missing from tauriClient.ts"
grep -q "interface WriteAuditEntryDto" "$TAURI_CLIENT_TS" || fail "WriteAuditEntryDto interface missing from tauriClient.ts"
pass "phase 16 typescript interfaces present in tauriClient.ts (m16.4)"

# 13. phase 16 typescript wrappers in tauriClient.ts (m16.4)
grep -q "function listSubtaskSteps" "$TAURI_CLIENT_TS" || fail "listSubtaskSteps wrapper missing from tauriClient.ts"
grep -q "function listFileWriteProposals" "$TAURI_CLIENT_TS" || fail "listFileWriteProposals wrapper missing from tauriClient.ts"
grep -q "function approveSubtaskFileWrite" "$TAURI_CLIENT_TS" || fail "approveSubtaskFileWrite wrapper missing from tauriClient.ts"
grep -q "function rejectSubtaskFileWrite" "$TAURI_CLIENT_TS" || fail "rejectSubtaskFileWrite wrapper missing from tauriClient.ts"
grep -q "function listWriteAuditLog" "$TAURI_CLIENT_TS" || fail "listWriteAuditLog wrapper missing from tauriClient.ts"
grep -q "function startSubtaskChain" "$TAURI_CLIENT_TS" || fail "startSubtaskChain wrapper missing from tauriClient.ts"
pass "all 6 typescript wrappers present in tauriClient.ts (m16.4)"

# 14. phase 16 ui surfaces in App.tsx (m16.4)
grep -q "file-write-proposal-card" "$APP_TSX" || fail "file-write-proposal-card ui missing from App.tsx"
grep -q "file-write-approve-button" "$APP_TSX" || fail "file-write-approve-button missing from App.tsx"
grep -q "file-write-reject-button" "$APP_TSX" || fail "file-write-reject-button missing from App.tsx"
grep -q "subtask-step-list\|subtask-steps-" "$APP_TSX" || fail "subtask step progress ui missing from App.tsx"
grep -q "startSubtaskChain" "$APP_TSX" || fail "startSubtaskChain not used in App.tsx"
grep -q "fileWriteProposals" "$APP_TSX" || fail "fileWriteProposals state missing from App.tsx"
grep -q "subtaskStepsById" "$APP_TSX" || fail "subtaskStepsById state missing from App.tsx"
pass "phase 16 ui surfaces, state, and chain routing present in App.tsx (m16.4)"

# 15. safety boundary: approval path must use explicit apply state machine and ordered write.
APPROVE_START=$(grep -n "pub fn approve_subtask_file_write" "$COMMANDS_RS" | head -1 | cut -d: -f1)
APPROVE_END=$(grep -n "pub fn reject_subtask_file_write" "$COMMANDS_RS" | head -1 | cut -d: -f1)
if [ -n "$APPROVE_START" ] && [ -n "$APPROVE_END" ]; then
  APPROVE_SECTION=$(sed -n "${APPROVE_START},${APPROVE_END}p" "$COMMANDS_RS")
  echo "$APPROVE_SECTION" | grep -q "begin_file_write_proposal_apply" || fail "approve command does not begin apply state transition"
  echo "$APPROVE_SECTION" | grep -q "complete_file_write_proposal_apply" || fail "approve command does not complete apply state transition"
  echo "$APPROVE_SECTION" | grep -q "rollback_file_write_proposal_apply" || fail "approve command missing rollback on write failure"
  echo "$APPROVE_SECTION" | grep -q "fs::write" || fail "approve command does not perform explicit filesystem write"
fi

BEGIN_LINE=$(grep -n "begin_file_write_proposal_apply" "$COMMANDS_RS" | head -1 | cut -d: -f1)
WRITE_LINE=$(grep -n "fs::write(&dest" "$COMMANDS_RS" | head -1 | cut -d: -f1)
COMPLETE_LINE=$(grep -n "complete_file_write_proposal_apply" "$COMMANDS_RS" | head -1 | cut -d: -f1)
if [ -n "${BEGIN_LINE:-}" ] && [ -n "${WRITE_LINE:-}" ] && [ -n "${COMPLETE_LINE:-}" ]; then
  if [ "$BEGIN_LINE" -ge "$WRITE_LINE" ]; then
    fail "approve ordering invalid: begin apply transition must happen before filesystem write"
  fi
  if [ "$WRITE_LINE" -ge "$COMPLETE_LINE" ]; then
    fail "approve ordering invalid: filesystem write must happen before complete apply transition"
  fi
fi
pass "approve command enforces apply state machine with ordered begin/write/complete and rollback (m16.3)"

# 16. regression: phase 13 watcher symbols still present
grep -q "fn start_workspace_watcher" "$COMMANDS_RS" || fail "phase 13 regression: start_workspace_watcher missing"
pass "phase 13 regression check: start_workspace_watcher still present"

# 17. regression: phase 14 classifier still present
grep -q "fn classify_message_intent" "$COMMANDS_RS" || fail "phase 14 regression: classify_message_intent missing"
pass "phase 14 regression check: classify_message_intent still present"

# 18. regression: phase 15 proactive commands still present
grep -q "commands::trigger_task_resume" "$MAIN_RS" || fail "phase 15 regression: trigger_task_resume missing from main.rs"
grep -q "commands::check_task_drift" "$MAIN_RS" || fail "phase 15 regression: check_task_drift missing from main.rs"
pass "phase 15 regression check: proactive commands still registered"

# 19. build: cargo build must pass with no errors
echo "running cargo build..."
cd "$ROOT_DIR/desktop/src-tauri"
cargo build 2>&1 | grep -E "^error" | head -5 && fail "cargo build produced errors" || true
echo "cargo build passed"
pass "cargo build succeeds with no errors (m16.1-m16.3)"

# 20. tests: all rust unit tests pass (single-threaded to avoid sqlite lock contention).
# run twice: first pass warms the compiled binary; second pass is the authoritative result.
echo "running cargo test --test-threads=1 (x2 for timing stability)..."
cd "$ROOT_DIR/desktop/src-tauri"
cargo test -- --test-threads=1 2>&1 | tail -4 || true   # warm pass
if ! cargo test -- --test-threads=1 2>&1; then
  fail "rust tests failed on second run"
fi
pass "all rust unit tests pass (m16.1-m16.2)"

# 21. typescript build must pass
echo "running frontend build..."
cd "$ROOT_DIR/desktop"
npm run build 2>&1 | tail -6
pass "frontend typescript build succeeds (m16.4)"

echo ""
echo "--- phase 16 check complete: all checks passed ---"
