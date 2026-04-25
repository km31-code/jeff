#!/usr/bin/env bash
# phase 23 behavioral check script
# verifies: personalization (user profile signals, "jeff remembers" ui),
# workload awareness (cross-task view, stale notifications, collision detection),
# calendar context (eventkit), and live app actions (browser extension write path,
# preview/approval flow, anchor validation, fallback path).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$REPO_ROOT/desktop/src-tauri/src"
FRONTEND="$REPO_ROOT/desktop/src"
EXT="$REPO_ROOT/browser-extension/selection-capture"

PASS=0
FAIL=0

check() {
    local desc="$1"
    local result="$2"
    if [ "$result" = "ok" ]; then
        echo "  [pass] $desc"
        PASS=$((PASS + 1))
    else
        echo "  [fail] $desc"
        FAIL=$((FAIL + 1))
    fi
}

grep_check() {
    local desc="$1"
    shift
    if grep -r "$@" > /dev/null 2>&1; then
        check "$desc" "ok"
    else
        check "$desc" "fail"
    fi
}

run_check() {
    local desc="$1"
    shift
    if "$@" > /dev/null 2>&1; then
        check "$desc" "ok"
    else
        check "$desc" "fail"
    fi
}

echo ""
echo "phase 23: personalization, workload, calendar, live app actions"
echo "=================================================================="

echo ""
echo "--- m23.1/m23.2: personalization — user profile signals ---"

grep_check "user_model module exists" \
    "pub fn build_profile_injection" "$SRC/user_model.rs"
grep_check "get_profile_value helper" \
    "pub fn get_profile_value" "$SRC/user_model.rs"
grep_check "set_profile_value helper" \
    "pub fn set_profile_value" "$SRC/user_model.rs"
grep_check "clear_all_profile helper" \
    "pub fn clear_all_profile" "$SRC/user_model.rs"
grep_check "user_profile table in db migration" \
    "CREATE TABLE IF NOT EXISTS user_profile" "$SRC/store.rs"
grep_check "profile injection in chat.rs" \
    "build_profile_injection" "$SRC/chat.rs"
grep_check "privacy gate guards chat injection" \
    "privacy_user_profile_memory_enabled" "$SRC/chat.rs"
grep_check "profile injection in proactive.rs" \
    "build_profile_injection" "$SRC/proactive.rs"
grep_check "privacy gate guards proactive injection" \
    "privacy_user_profile_memory_enabled" "$SRC/proactive.rs"
grep_check "revision accepted signal" \
    "record_revision_accepted\|record_revision_rewrite" "$SRC/user_model.rs"
grep_check "subtask accepted signal" \
    "record_subtask_accepted" "$SRC/user_model.rs"
grep_check "subtask rejected signal" \
    "record_subtask_rejected" "$SRC/user_model.rs"
grep_check "focus hour signal" \
    "record_focus_hour" "$SRC/user_model.rs"
grep_check "trigger dismissed signal" \
    "record_trigger_dismissed" "$SRC/user_model.rs"
grep_check "quality rubric write" \
    "add_quality_rubric\|rubric_" "$SRC/user_model.rs"
grep_check "get_readable_signals human labels" \
    "get_readable_signals" "$SRC/user_model.rs"
grep_check "get_user_profile_signals command registered" \
    "get_user_profile_signals" "$SRC/commands.rs" "$SRC/main.rs"
grep_check "add_quality_rubric command registered" \
    "add_quality_rubric" "$SRC/commands.rs" "$SRC/main.rs"
grep_check "delete_quality_rubric command registered" \
    "delete_quality_rubric" "$SRC/commands.rs" "$SRC/main.rs"
grep_check "delete_user_profile_signal command registered" \
    "delete_user_profile_signal" "$SRC/commands.rs" "$SRC/main.rs"
grep_check "signal writes wired in commands.rs on revision accept" \
    "record_revision" "$SRC/commands.rs"
grep_check "signal writes wired in commands.rs on subtask accept" \
    "record_subtask_accepted" "$SRC/commands.rs"
grep_check "signal writes wired in commands.rs on trigger dismiss" \
    "record_trigger_dismissed" "$SRC/commands.rs"
grep_check "signal writes wired in commands.rs on task focus" \
    "record_focus_hour" "$SRC/commands.rs"

echo ""
echo "--- m23.2: jeff remembers ui ---"

grep_check "jeff remembers panel in App.tsx" \
    "Jeff remembers" "$FRONTEND/App.tsx"
grep_check "jeff remembers open state" \
    "jeffRemembersOpen" "$FRONTEND/App.tsx"
grep_check "clear all button in jeff remembers panel" \
    "Clear all" "$FRONTEND/App.tsx"
grep_check "rubric input field" \
    "rubric-input\|rubricInput" "$FRONTEND/App.tsx"
grep_check "rubric add button" \
    "rubric-add" "$FRONTEND/App.tsx"
grep_check "per-signal delete wired" \
    "deleteUserProfileSignal\|delete_user_profile_signal" "$FRONTEND/App.tsx"
grep_check "typed wrappers in tauriClient.ts" \
    "getUserProfileSignals\|get_user_profile_signals" "$FRONTEND/tauriClient.ts"

echo ""
echo "--- m23.3: workload awareness ---"

grep_check "workload module exists" \
    "pub fn compute_workload_summary" "$SRC/workload.rs"
grep_check "active vs stale classification" \
    "active_tasks\|stale_tasks" "$SRC/workload.rs"
grep_check "stale notification throttle key format" \
    "stale_notify_" "$SRC/workload.rs"
grep_check "stale notification checks quiet mode" \
    "quiet_mode\|quiet" "$SRC/workload.rs"
grep_check "check_stale_task_notifications callable" \
    "pub fn check_stale_task_notifications" "$SRC/workload.rs"
grep_check "get_workload_summary command registered" \
    "get_workload_summary" "$SRC/commands.rs" "$SRC/main.rs"
grep_check "switch_active_task_from_companion command registered" \
    "switch_active_task_from_companion" "$SRC/commands.rs" "$SRC/main.rs"
grep_check "stale notification called at startup" \
    "check_stale_task_notifications" "$SRC/main.rs"
grep_check "cross-task collision detection in commands.rs" \
    "subtask://collision-detected" "$SRC/commands.rs"
grep_check "collision check uses cosine_similarity" \
    "cosine_similarity" "$SRC/commands.rs"
grep_check "collision detection queries recent subtasks" \
    "get_recent_cross_task_subtasks" "$SRC/store.rs" "$SRC/commands.rs"
grep_check "collision threshold 0.8" \
    "0.8" "$SRC/commands.rs"
grep_check "workload section in App.tsx" \
    "workloadSummary\|workload-summary" "$FRONTEND/App.tsx"
grep_check "workload open state" \
    "workloadOpen" "$FRONTEND/App.tsx"
grep_check "switch task from companion wired in App.tsx" \
    "switchActiveTaskFromCompanion" "$FRONTEND/App.tsx"
grep_check "collision notice state in App.tsx" \
    "collisionNotice" "$FRONTEND/App.tsx"
grep_check "collision event subscribed in App.tsx" \
    "subtask://collision-detected" "$FRONTEND/App.tsx"

echo ""
echo "--- m23.4: calendar context (eventkit) ---"

grep_check "calendar module exists" \
    "pub fn fetch_next_event\|fn fetch_next_event" "$SRC/calendar.rs"
grep_check "macos conditional compilation" \
    "target_os.*macos\|cfg.*macos" "$SRC/calendar.rs"
grep_check "eventkit permission request" \
    "request_calendar_permission\|EKEventStore\|requestAccessToEntityType" "$SRC/calendar.rs"
grep_check "get_calendar_permission_status returns string" \
    "get_calendar_permission_status\|not_determined\|granted\|denied" "$SRC/calendar.rs"
grep_check "calendar state managed" \
    "CalendarState" "$SRC/state.rs" "$SRC/main.rs"
grep_check "calendar poll task spawned in main.rs" \
    "calendar.*poll\|fetch_next_event" "$SRC/main.rs"
grep_check "calendar poll respects privacy gate" \
    "privacy_calendar_context_enabled" "$SRC/main.rs"
grep_check "calendar event emitted to frontend" \
    "calendar://event-updated" "$SRC/main.rs"
grep_check "request_calendar_permission command registered" \
    "request_calendar_permission" "$SRC/commands.rs" "$SRC/main.rs"
grep_check "get_calendar_permission_status command registered" \
    "get_calendar_permission_status" "$SRC/commands.rs" "$SRC/main.rs"
grep_check "get_calendar_next_event command registered" \
    "get_calendar_next_event" "$SRC/commands.rs" "$SRC/main.rs"
grep_check "calendar event in companion header" \
    "calendarEvent\|calendar_context_enabled" "$FRONTEND/App.tsx"
grep_check "calendar header shows minutes until" \
    "minutes_until" "$FRONTEND/App.tsx"
grep_check "calendar context in reorientation prompt" \
    "calendar_event\|CalendarEvent\|minutes_until\|meeting in" "$SRC/proactive.rs"

echo ""
echo "--- m23.5: live app actions ---"

grep_check "live_edit_receipts table in db migration" \
    "CREATE TABLE IF NOT EXISTS live_edit_receipts" "$SRC/store.rs"
grep_check "live edit receipt crud in store.rs" \
    "create_live_edit_receipt\|update_live_edit_status" "$SRC/store.rs"
grep_check "apply-edit route in selection_capture.rs" \
    "/apply-edit" "$SRC/selection_capture.rs"
grep_check "apply-fallback route in selection_capture.rs" \
    "/apply-fallback" "$SRC/selection_capture.rs"
grep_check "pending-approval poll route" \
    "/pending-approval" "$SRC/selection_capture.rs"
grep_check "live_action apply_requested event emitted" \
    "live_action://apply_requested" "$SRC/selection_capture.rs"
grep_check "live_action approved event constant" \
    "EVENT_LIVE_ACTION_APPROVED\|live_action://approved" "$SRC/selection_capture.rs"
grep_check "live_action fallback event constant" \
    "EVENT_LIVE_ACTION_FALLBACK\|live_action://fallback_triggered" "$SRC/selection_capture.rs"
grep_check "approve_live_edit command registered" \
    "approve_live_edit" "$SRC/commands.rs" "$SRC/main.rs"
grep_check "reject_live_edit command registered" \
    "reject_live_edit" "$SRC/commands.rs" "$SRC/main.rs"
grep_check "list_live_edit_receipts command registered" \
    "list_live_edit_receipts" "$SRC/commands.rs" "$SRC/main.rs"
grep_check "get_pending_live_edits command registered" \
    "get_pending_live_edits" "$SRC/commands.rs" "$SRC/main.rs"
grep_check "anchor validation in content.js" \
    "normalizeText\|anchorMismatch\|anchor.*no longer matches" "$EXT/content.js"
grep_check "apply edit in place in content.js" \
    "applyEditInPlace" "$EXT/content.js"
grep_check "apply edit message handler in content.js" \
    "JEFF_APPLY_EDIT" "$EXT/content.js"
grep_check "apply-fallback fetch on anchor mismatch in content.js" \
    "apply-fallback" "$EXT/content.js"
grep_check "approval poll loop in background.js" \
    "pollForLiveEditApproval" "$EXT/background.js"
grep_check "apply edit dispatch from background.js" \
    "JEFF_APPLY_EDIT" "$EXT/background.js"
grep_check "live edit proposal handler in background.js" \
    "handleLiveEditProposal\|JEFF_PROPOSE_LIVE_EDIT" "$EXT/background.js"
grep_check "docs.google.com in extension manifest" \
    "docs.google.com" "$EXT/manifest.json"
grep_check "live edit preview card in App.tsx" \
    "live-edit-preview-card" "$FRONTEND/App.tsx"
grep_check "approve button in live edit preview" \
    "approve.*live.*edit\|approveLiveEdit" "$FRONTEND/App.tsx"
grep_check "reject button in live edit preview" \
    "reject.*live.*edit\|rejectLiveEdit" "$FRONTEND/App.tsx"
grep_check "guided-apply fallback render in App.tsx" \
    "guided-apply-fallback" "$FRONTEND/App.tsx"
grep_check "before_text and after_text rendered in preview" \
    "before_text\|after_text" "$FRONTEND/App.tsx"
grep_check "live edit event subscribed in App.tsx" \
    "live_action://apply_requested" "$FRONTEND/App.tsx"
grep_check "live edit approved event subscribed in App.tsx" \
    "live_action://approved" "$FRONTEND/App.tsx"
grep_check "live edit fallback event subscribed in App.tsx" \
    "live_action://fallback_triggered" "$FRONTEND/App.tsx"

echo ""
echo "--- behavioral tests ---"

run_check "user_model unit tests pass" \
    cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" user_model
run_check "workload unit tests pass" \
    cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" workload
run_check "frontend tests pass" \
    npm --prefix "$REPO_ROOT/desktop" test -- --run

echo ""
echo "--- regression guard ---"

run_check "phase22_check.sh still passes" \
    bash "$REPO_ROOT/scripts/phase22_check.sh"

echo ""
echo "phase 23 check: $PASS passed, $FAIL failed"
if [ "$FAIL" -ne 0 ]; then
    exit 1
fi
