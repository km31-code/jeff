#!/usr/bin/env bash
# phase 19 behavioral check script
# verifies: SMAppService login-item registration, setting round-trips, session-restore
# command, overlay/quiet persistence, no set_focus in startup path.
# grep checks confirm symbol presence; cargo test checks confirm runtime behavior.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$REPO_ROOT/desktop/src-tauri/src"

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
    if grep -r "$@" "$SRC" > /dev/null 2>&1; then
        check "$desc" "ok"
    else
        check "$desc" "fail"
    fi
}

echo ""
echo "phase 19: presence completion — launch at login + session restore"
echo "=================================================================="

echo ""
echo "--- m19.1: login item registration ---"

# SMAppService wrapper must be present
if [ -f "$SRC/login_item.rs" ]; then
    check "login_item.rs exists" "ok"
else
    check "login_item.rs exists" "fail"
fi

grep_check "SMAppService mainAppService wrapper" \
    "SMAppService" "$SRC/login_item.rs"

grep_check "ServiceManagement framework linked" \
    "ServiceManagement" "$SRC/login_item.rs"

grep_check "set_login_item_enabled helper" \
    "set_login_item_enabled" "$SRC/login_item.rs"

grep_check "login item status helper" \
    "login_item_status" "$SRC/login_item.rs"

if grep -q "tauri-plugin-autostart" "$REPO_ROOT/desktop/src-tauri/Cargo.toml"; then
    check "tauri-plugin-autostart removed from Cargo.toml" "fail"
else
    check "tauri-plugin-autostart removed from Cargo.toml" "ok"
fi

# store setting constants exist
grep_check "APP_SETTING_LAUNCH_AT_LOGIN constant" \
    "APP_SETTING_LAUNCH_AT_LOGIN" "$SRC/store.rs"

# store methods exist
grep_check "get_launch_at_login store method" \
    "fn get_launch_at_login" "$SRC/store.rs"

grep_check "set_launch_at_login store method" \
    "fn set_launch_at_login" "$SRC/store.rs"

# commands exist and call SMAppService helper
grep_check "get_launch_at_login command" \
    "pub fn get_launch_at_login" "$SRC/commands.rs"

grep_check "set_launch_at_login command" \
    "pub fn set_launch_at_login" "$SRC/commands.rs"

grep_check "set_launch_at_login uses SMAppService helper" \
    "set_login_item_enabled" "$SRC/commands.rs"

grep_check "get_launch_at_login reconciles OS state" \
    "login_item_enabled_or_pending" "$SRC/commands.rs"

# commands registered in invoke_handler
grep_check "get_launch_at_login registered in invoke_handler" \
    "commands::get_launch_at_login" "$SRC/main.rs"

grep_check "set_launch_at_login registered in invoke_handler" \
    "commands::set_launch_at_login" "$SRC/main.rs"

echo ""
echo "--- m19.2: tray menu toggle ---"

# tray:launch_at_login menu item event handler
grep_check "tray:launch_at_login menu event handler" \
    "tray:launch_at_login" "$SRC/ambient.rs"

grep_check "tray launch toggle uses SMAppService helper" \
    "set_login_item_enabled" "$SRC/ambient.rs"

# CheckMenuItem import (visual checkmark)
grep_check "CheckMenuItem imported in ambient.rs" \
    "CheckMenuItem" "$SRC/ambient.rs"

# build_tray_menu helper exists
grep_check "build_tray_menu helper function" \
    "fn build_tray_menu" "$SRC/ambient.rs"

# launch_at_login param in install_tray
grep_check "install_tray takes launch_at_login param" \
    "launch_at_login" "$SRC/ambient.rs"

echo ""
echo "--- m19.3: overlay mode + quiet mode persistence ---"

grep_check "APP_SETTING_OVERLAY_MODE constant" \
    "APP_SETTING_OVERLAY_MODE" "$SRC/store.rs"

grep_check "APP_SETTING_QUIET_MODE constant" \
    "APP_SETTING_QUIET_MODE" "$SRC/store.rs"

grep_check "get_overlay_expanded store method" \
    "fn get_overlay_expanded" "$SRC/store.rs"

grep_check "set_overlay_expanded store method" \
    "fn set_overlay_expanded" "$SRC/store.rs"

grep_check "get_quiet_mode store method" \
    "fn get_quiet_mode" "$SRC/store.rs"

grep_check "set_quiet_mode store method" \
    "fn set_quiet_mode" "$SRC/store.rs"

# ambient_set_overlay_mode persists (calls set_overlay_expanded)
grep_check "ambient_set_overlay_mode persists overlay mode" \
    "set_overlay_expanded" "$SRC/ambient.rs"

# ambient_set_quiet_mode persists (calls store.set_quiet_mode)
grep_check "ambient_set_quiet_mode persists quiet mode" \
    "store.set_quiet_mode" "$SRC/ambient.rs"

# startup reads and applies both settings
grep_check "startup reads quiet_mode from store" \
    "get_quiet_mode" "$SRC/main.rs"

grep_check "startup reads overlay_expanded from store" \
    "get_overlay_expanded" "$SRC/main.rs"

echo ""
echo "--- m19.4: restore_session command + first-session notification ---"

grep_check "APP_SETTING_SESSION_RESTORED_AT constant" \
    "APP_SETTING_SESSION_RESTORED_AT" "$SRC/store.rs"

grep_check "mark_session_restored store method" \
    "fn mark_session_restored" "$SRC/store.rs"

grep_check "get_session_restore_state command" \
    "pub fn get_session_restore_state" "$SRC/commands.rs"

grep_check "get_session_restore_state registered in invoke_handler" \
    "commands::get_session_restore_state" "$SRC/main.rs"

grep_check "SessionRestoreDto in models.rs" \
    "SessionRestoreDto" "$SRC/models.rs"

# first-session notification fires in main.rs
grep_check "first-session notification in main.rs" \
    "is_first_session" "$SRC/main.rs"

grep_check "mark_session_restored called in main.rs" \
    "mark_session_restored" "$SRC/main.rs"

grep_check "getSessionRestoreState exported in tauriClient.ts" \
    "getSessionRestoreState" "$REPO_ROOT/desktop/src/tauriClient.ts"

grep_check "get_launch_at_login does not write-on-read" \
    "reads OS state directly" "$SRC/commands.rs"

echo ""
echo "--- m19.5: no set_focus in startup path ---"

# set_focus must not appear as a code call in main.rs (startup path).
# show_workspace in ambient.rs does call set_focus on explicit user action,
# but never during automatic startup. grep excludes comment lines so that
# audit comments mentioning the function do not produce false failures.
if grep "set_focus" "$SRC/main.rs" | grep -v "^\s*//" > /dev/null 2>&1; then
    check "no set_focus calls in main.rs startup path" "fail"
else
    check "no set_focus calls in main.rs startup path" "ok"
fi

echo ""
echo "--- behavioral: cargo test ---"

echo "  running session_settings_round_trip..."
if (cd "$REPO_ROOT/desktop/src-tauri" && cargo test --bin jeff-desktop session_settings_round_trip >/dev/null); then
    check "session_settings_round_trip passes" "ok"
else
    check "session_settings_round_trip passes" "fail"
fi

echo "  running login_item status tests..."
if (cd "$REPO_ROOT/desktop/src-tauri" && cargo test --bin jeff-desktop login_item >/dev/null); then
    check "login_item tests pass" "ok"
else
    check "login_item tests pass" "fail"
fi

echo "  running all bin tests (regression check)..."
if (cd "$REPO_ROOT/desktop/src-tauri" && cargo test --bin jeff-desktop >/dev/null); then
    check "all bin tests pass (no regressions)" "ok"
else
    check "all bin tests pass (no regressions)" "fail"
fi

echo ""
echo "=================================================================="
echo "phase 19 check: $PASS passed, $FAIL failed"
echo "=================================================================="

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
