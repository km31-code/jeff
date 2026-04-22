#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
AMBIENT_RS="$ROOT_DIR/desktop/src-tauri/src/ambient.rs"
MAIN_RS="$ROOT_DIR/desktop/src-tauri/src/main.rs"
CARGO_TOML="$ROOT_DIR/desktop/src-tauri/Cargo.toml"
TAURI_CONF="$ROOT_DIR/desktop/src-tauri/tauri.conf.json"
CAPABILITIES="$ROOT_DIR/desktop/src-tauri/capabilities/default.json"
OVERLAY_TSX="$ROOT_DIR/desktop/src/Overlay.tsx"
AMBIENT_CLIENT="$ROOT_DIR/desktop/src/ambientClient.ts"
MAIN_TSX="$ROOT_DIR/desktop/src/main.tsx"
COMMANDS_RS="$ROOT_DIR/desktop/src-tauri/src/commands.rs"

echo "--- phase 11 ambient presence check ---"

fail() { echo "FAIL: $1" >&2; exit 1; }
pass() { echo "PASS: $1"; }

# 1. tray: TrayIconBuilder present in ambient.rs
grep -q "TrayIconBuilder" "$AMBIENT_RS" || fail "TrayIconBuilder not found in ambient.rs"
pass "tray icon builder present"

# 2. single-instance plugin registered in main.rs
grep -q "tauri_plugin_single_instance" "$MAIN_RS" || fail "single-instance plugin not registered in main.rs"
pass "single-instance plugin registered"

# 3. single-instance dep in Cargo.toml
grep -q "tauri-plugin-single-instance" "$CARGO_TOML" || fail "tauri-plugin-single-instance not in Cargo.toml"
pass "single-instance dep present"

# 4. global-shortcut plugin and pressed-only handler in main.rs
grep -q "tauri_plugin_global_shortcut" "$MAIN_RS" || fail "global-shortcut plugin not in main.rs"
grep -q "ShortcutState::Pressed" "$MAIN_RS" || fail "hotkey pressed-only handler not found in main.rs"
pass "global-shortcut plugin and pressed-only handler present"

# 5. hotkey constant is CmdOrCtrl+Shift+J
grep -q "CmdOrCtrl+Shift+J" "$AMBIENT_RS" || fail "expected hotkey CmdOrCtrl+Shift+J not found in ambient.rs"
pass "default hotkey is CmdOrCtrl+Shift+J"

# 6. overlay window: frameless and always-on-top
grep -q '\.decorations(false)' "$AMBIENT_RS" || fail "overlay window missing .decorations(false)"
grep -q '\.always_on_top(true)' "$AMBIENT_RS" || fail "overlay window missing .always_on_top(true)"
pass "overlay window is frameless and always-on-top"

# 7. focus preservation: show_overlay must NOT call set_focus
SHOW_OVERLAY_BLOCK=$(awk '/^pub fn show_overlay/,/^}/' "$AMBIENT_RS")
if echo "$SHOW_OVERLAY_BLOCK" | grep -q "set_focus("; then
  fail "show_overlay calls set_focus — this would steal focus from user app"
fi
pass "show_overlay does not steal focus"

# 8. overlay builder has focused(false)
grep -q '\.focused(false)' "$AMBIENT_RS" || fail "overlay window builder missing .focused(false)"
pass "overlay window has focused(false) on build"

# 9. close-to-tray: both windows use prevent_close
grep -q "prevent_close" "$AMBIENT_RS" || fail "CloseRequested handler missing prevent_close in ambient.rs"
grep -q "prevent_close" "$MAIN_RS" || fail "main window CloseRequested handler missing prevent_close in main.rs"
pass "close-to-tray implemented for both windows"

# 10. quit only via tray menu
grep -q '"tray:quit"' "$AMBIENT_RS" || fail "tray:quit menu item not found in ambient.rs"
pass "quit only reachable via tray menu"

# 11. notification plugin registered
grep -q "tauri_plugin_notification" "$MAIN_RS" || fail "notification plugin not in main.rs"
grep -q "tauri-plugin-notification" "$CARGO_TOML" || fail "tauri-plugin-notification not in Cargo.toml"
pass "notification plugin registered and in Cargo.toml"

# 12. notification quiet mode suppression
grep -q "is_quiet_mode" "$AMBIENT_RS" || fail "quiet mode suppression not found in ambient.rs"
pass "notification quiet mode suppression present"

# 13. notification permission probed in overlay
grep -q "requestPermission\|markNotificationPermission" "$OVERLAY_TSX" || fail "notification permission not probed in Overlay.tsx"
pass "notification permission probed in overlay"

# 13b. proactive nudge path dispatches native notification when backgrounded
grep -q "Jeff has a nudge" "$COMMANDS_RS" || fail "proactive nudge notification wiring missing in commands.rs"
pass "proactive nudge notification wiring present"

# 14. main window starts hidden
grep -q '"visible": false' "$TAURI_CONF" || fail "main window not set to visible:false in tauri.conf.json"
pass "main window hidden on startup"

# 14b. if overlay window is declared in tauri.conf.json, it must be always-on-top
if grep -q '"label": "overlay"' "$TAURI_CONF"; then
  grep -q '"alwaysOnTop": true' "$TAURI_CONF" || \
    fail "overlay window config in tauri.conf.json must set alwaysOnTop:true"
  pass "overlay window config has alwaysOnTop:true"
else
  pass "overlay window is runtime-created; no overlay entry in tauri.conf.json"
fi

# 15. overlay window label constant present
grep -q 'OVERLAY_WINDOW_LABEL' "$AMBIENT_RS" || fail "OVERLAY_WINDOW_LABEL constant not found in ambient.rs"
pass "overlay window label constant present"

# 16. collapsed and expanded resize logic present
grep -q "OVERLAY_COLLAPSED_HEIGHT" "$AMBIENT_RS" || fail "OVERLAY_COLLAPSED_HEIGHT constant not found"
grep -q "OVERLAY_EXPANDED_HEIGHT" "$AMBIENT_RS" || fail "OVERLAY_EXPANDED_HEIGHT constant not found"
grep -q "resize_overlay_for_mode" "$AMBIENT_RS" || fail "resize_overlay_for_mode function not found"
pass "collapsed and expanded overlay resize logic present"

# 17. frontend overlay entrypoint branches on isOverlayWindow
grep -q "isOverlayWindow" "$MAIN_TSX" || fail "isOverlayWindow not used in main.tsx"
pass "main.tsx branches on isOverlayWindow()"

# 18. Overlay.tsx has collapsed and expanded states
grep -q "overlay-collapsed" "$OVERLAY_TSX" || fail "collapsed state missing in Overlay.tsx"
grep -q "overlay-expanded" "$OVERLAY_TSX" || fail "expanded state missing in Overlay.tsx"
pass "overlay has collapsed and expanded states"

# 19. capabilities grant notification and shortcut permissions
grep -q "notification:allow-notify" "$CAPABILITIES" || fail "notification:allow-notify not in capabilities"
grep -q "global-shortcut:allow-register" "$CAPABILITIES" || fail "global-shortcut:allow-register not in capabilities"
pass "capabilities grant notification and shortcut permissions"

# 20. full build and test suite
cd "$ROOT_DIR/desktop"
npm run lint
npm run test
cargo build --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml ambient

echo ""
echo "phase 11 checks passed"
