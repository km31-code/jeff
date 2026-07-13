#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
FRONTEND_FILE="$ROOT_DIR/desktop/src/tauriClient.ts"
BACKEND_FILE="$ROOT_DIR/desktop/src-tauri/src/commands.rs"

required_commands=(
  "create_task"
  "list_tasks"
  "get_active_task"
  "set_active_task"
  "get_task_workspace"
  "get_task_summary"
  "list_open_resources"
  "import_artifact"
  "list_artifacts"
  "retrieve_context"
  "build_context_pack"
  "list_messages"
  "send_message"
  "cancel_interaction"
  "transcribe_audio"
  "synthesize_speech"
  "get_coworking_status"
  "set_proactive_mode"
  "set_user_typing"
  "set_user_speaking"
  "set_assistant_speaking"
  "evaluate_proactive_nudge"
  "get_artifact_content"
  "propose_artifact_revision"
  "list_pending_revisions"
  "list_task_pending_revisions"
  "apply_revision"
  "reject_revision"
  "list_artifact_versions"
  "revert_artifact_to_version"
  "create_subtask"
  "list_subtasks"
  "cancel_subtask"
  "accept_subtask_result"
  "reject_subtask_result"
  "suggest_subtask"
  "refine_subtask"
  "convert_subtask_to_revision"
  "evaluate_next_suggestions"
  "list_suggestions"
  "accept_suggestion"
  "dismiss_suggestion"
  "explain_suggestion"
  "get_session_mode_state"
  "list_recent_events"
  "get_active_artifact_selection"
  "set_active_artifact_selection"
)

for command in "${required_commands[@]}"; do
  # grep -F (fixed strings), not rg: rg is not always a real binary in a bash
  # subshell (it can be a shell function), and these patterns are literal.
  if ! grep -qF "\"$command\"" "$FRONTEND_FILE"; then
    echo "ERROR: frontend invoke contract missing command '$command'" >&2
    exit 1
  fi

  if ! grep -qF "pub fn $command" "$BACKEND_FILE"; then
    echo "ERROR: backend command handler missing '$command'" >&2
    exit 1
  fi
done

echo "IPC contract check passed for Phase 10 companion + integrated coworking/revision/subtask/flow commands"
