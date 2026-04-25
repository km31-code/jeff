#!/usr/bin/env bash
# phase 21 behavioral check script
# verifies: privacy center commands, persistent toggles, enforcement guards,
# data clearing, audit views, and frontend rendering.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$REPO_ROOT/desktop/src-tauri/src"
FRONTEND="$REPO_ROOT/desktop/src"

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
echo "phase 21: privacy and trust control center"
echo "=================================================================="

echo ""
echo "--- m21.1: privacy settings and enforcement ---"

for key in \
    "privacy_workspace_watcher_enabled" \
    "privacy_clipboard_capture_enabled" \
    "privacy_active_window_context_enabled" \
    "privacy_proactive_triggers_enabled" \
    "privacy_user_profile_memory_enabled" \
    "privacy_calendar_context_enabled"; do
    grep_check "app_settings key: $key" "$key" "$SRC/store.rs"
done

grep_check "workspace restore respects privacy setting" \
    "get_privacy_workspace_watcher_enabled" "$SRC/commands.rs"
grep_check "clipboard poll respects global privacy setting" \
    "get_privacy_clipboard_capture_enabled" "$SRC/commands.rs"
grep_check "active-window context command respects privacy setting" \
    "get_privacy_active_window_context_enabled" "$SRC/commands.rs"
grep_check "active-window poll loop checks privacy setting" \
    "active_context_allowed" "$SRC/main.rs"
grep_check "proactive commands respect privacy setting" \
    "privacy_proactive_triggers_disabled" "$SRC/commands.rs"

echo ""
echo "--- m21.2: privacy dashboard commands and UI ---"

grep_check "PrivacyCenterDashboardDto model" \
    "struct PrivacyCenterDashboardDto" "$SRC/models.rs"
grep_check "get_privacy_center_dashboard command" \
    "pub fn get_privacy_center_dashboard" "$SRC/commands.rs"
grep_check "set_privacy_surface_enabled command" \
    "pub fn set_privacy_surface_enabled" "$SRC/commands.rs"
grep_check "privacy commands registered" \
    "commands::get_privacy_center_dashboard" "$SRC/main.rs"
grep_check "privacy surface setter registered" \
    "commands::set_privacy_surface_enabled" "$SRC/main.rs"
grep_check "tray What Jeff Knows menu item" \
    "What Jeff Knows" "$SRC/ambient.rs"
grep_check "privacy center open event" \
    "privacy://open" "$SRC/ambient.rs" "$FRONTEND/App.tsx"
grep_check "Privacy Center panel renders" \
    "privacy-center-panel" "$FRONTEND/App.tsx"
grep_check "workspace watcher toggle renders" \
    "privacy-toggle-workspace-watcher" "$FRONTEND/App.tsx"
grep_check "clipboard capture toggle renders" \
    "privacy-toggle-clipboard-capture" "$FRONTEND/App.tsx"
grep_check "active window context toggle renders" \
    "privacy-toggle-active-window-context" "$FRONTEND/App.tsx"
grep_check "proactive triggers toggle renders" \
    "privacy-toggle-proactive-triggers" "$FRONTEND/App.tsx"
grep_check "profile memory toggle renders" \
    "privacy-toggle-user-profile-memory" "$FRONTEND/App.tsx"
grep_check "calendar context toggle renders" \
    "privacy-toggle-calendar-context" "$FRONTEND/App.tsx"

echo ""
echo "--- m21.3: audit view ---"

grep_check "ProactiveAuditEntryDto model" \
    "struct ProactiveAuditEntryDto" "$SRC/models.rs"
grep_check "proactive audit store query" \
    "list_proactive_trigger_audit_log" "$SRC/store.rs"
grep_check "proactive audit command" \
    "pub fn list_proactive_trigger_audit_log" "$SRC/commands.rs"
grep_check "proactive audit command registered" \
    "commands::list_proactive_trigger_audit_log" "$SRC/main.rs"
grep_check "write audit shown in Privacy Center" \
    "privacy-write-audit-list" "$FRONTEND/App.tsx"
grep_check "proactive audit shown in Privacy Center" \
    "privacy-proactive-audit-list" "$FRONTEND/App.tsx"

echo ""
echo "--- m21.4/m21.5: data controls ---"

grep_check "DataClearResultDto model" \
    "struct DataClearResultDto" "$SRC/models.rs"
grep_check "clear active task data store method" \
    "pub fn clear_task_data" "$SRC/store.rs"
grep_check "clear all data store method" \
    "pub fn clear_all_data" "$SRC/store.rs"
grep_check "clear active task command" \
    "pub fn clear_active_task_data" "$SRC/commands.rs"
grep_check "clear all data command" \
    "pub fn clear_all_jeff_data" "$SRC/commands.rs"
grep_check "clear active task command registered" \
    "commands::clear_active_task_data" "$SRC/main.rs"
grep_check "clear all command registered" \
    "commands::clear_all_jeff_data" "$SRC/main.rs"
grep_check "clear all deletes keychain entry" \
    "delete_openai_api_key" "$SRC/commands.rs"
grep_check "clear all disables login item" \
    "set_login_item_enabled(false)" "$SRC/commands.rs"
grep_check "clear active task requests subtask cancellation" \
    "request_cancel" "$SRC/commands.rs"
grep_check "clear all confirmation UI" \
    "CLEAR JEFF" "$FRONTEND/App.tsx"
grep_check "clear active task data UI" \
    "privacy-clear-active-task-data" "$FRONTEND/App.tsx"

echo ""
echo "--- m21 remediation: proactive triggers does not set quiet mode ---"

grep_check "proactive_triggers toggle does not write quiet_mode" \
    -v "set_quiet_mode" "$SRC/commands.rs"

# verify: quiet_mode writes only appear in set_proactive_mode and clear_all paths,
# not in the privacy-surface toggle for proactive_triggers.
# the remediation removes quiet_mode side effects from set_privacy_surface_enabled.
grep_check "proactive triggers arm documents quiet mode separation" \
    "do NOT touch quiet_mode" "$SRC/commands.rs"

echo ""
echo "--- behavioral tests ---"

run_check "privacy_settings_round_trip passes" \
    cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" privacy_settings_round_trip
run_check "clear_task_data_keeps_task_and_removes_task_content passes" \
    cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" clear_task_data_keeps_task_and_removes_task_content
run_check "clear_all_data_resets_database_and_workspace_root passes" \
    cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" clear_all_data_resets_database_and_workspace_root
run_check "frontend privacy center tests pass" \
    npm --prefix "$REPO_ROOT/desktop" test -- --run

echo ""
echo "phase 21 check: $PASS passed, $FAIL failed"
if [ "$FAIL" -ne 0 ]; then
    exit 1
fi
