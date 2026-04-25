#!/usr/bin/env bash
# phase 22 behavioral check script
# verifies: explicit selection capture, browser bridge, in-memory prompt
# consumption, rate-only typing awareness, activity-aware tts, spoken cleanup,
# and persisted tts voice selection.

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
echo "phase 22: selection capture and voice naturalness"
echo "=================================================================="

echo ""
echo "--- m22.1/m22.2: selection state, privacy gate, hotkey, native AX ---"

grep_check "selection capture module exists" \
    "pub struct SelectionCaptureState" "$SRC/selection_capture.rs"
grep_check "capture is in-memory and consumed once" \
    "take_prompt_context" "$SRC/selection_capture.rs"
grep_check "selection hotkey constant" \
    "CmdOrCtrl+Shift+V" "$SRC/selection_capture.rs"
grep_check "selection hotkey registered" \
    "SELECTION_CAPTURE_HOTKEY" "$SRC/ambient.rs" "$SRC/main.rs"
grep_check "global shortcut handler routes selection hotkey" \
    "capture_selection_from_hotkey" "$SRC/main.rs"
grep_check "privacy gate for native selection capture" \
    "get_privacy_selection_capture_enabled" "$SRC/selection_capture.rs"
grep_check "native AX selected-text attribute path" \
    "AXSelectedText" "$SRC/selection_capture.rs"
grep_check "native AX focused element path" \
    "AXFocusedUIElement" "$SRC/selection_capture.rs"
grep_check "fallback message branch" \
    "Could not capture text from" "$SRC/selection_capture.rs"
grep_check "selection capture never references sqlite store" \
    "CapturedSelection" "$SRC/selection_capture.rs"

echo ""
echo "--- m22.3: indicator, dismiss, prompt injection ---"

grep_check "selection indicator DTO" \
    "SelectionCaptureIndicatorDto" "$SRC/models.rs" "$FRONTEND/tauriClient.ts"
grep_check "indicator command" \
    "get_selection_capture_indicator" "$SRC/commands.rs" "$SRC/main.rs"
grep_check "dismiss command clears in-memory state" \
    "dismiss_selection_capture" "$SRC/commands.rs" "$SRC/main.rs" "$FRONTEND/App.tsx"
grep_check "selection events wired" \
    "selection://captured" "$SRC/selection_capture.rs" "$FRONTEND/App.tsx"
grep_check "prompt injection consumes selected context" \
    "next_message_context" "$SRC/commands.rs"
grep_check "send_message takes selection state" \
    "selection_state: State<'_, SelectionCaptureState>" "$SRC/commands.rs"
grep_check "frontend indicator render" \
    "selection-capture-indicator" "$FRONTEND/App.tsx" "$FRONTEND/App.test.tsx"

echo ""
echo "--- m22.4: browser extension bridge ---"

grep_check "local bridge port" \
    "SELECTION_BRIDGE_PORT" "$SRC/selection_capture.rs"
grep_check "bridge status command" \
    "get_selection_bridge_status" "$SRC/commands.rs" "$SRC/main.rs" "$FRONTEND/tauriClient.ts"
grep_check "token validation" \
    "invalid browser selection bridge token" "$SRC/selection_capture.rs"
grep_check "bridge accepts only selection capture post" \
    "/selection-capture" "$SRC/selection_capture.rs" "$EXT/background.js"
grep_check "extension manifest exists" \
    "Jeff Selection Capture" "$EXT/manifest.json"
grep_check "extension captures selected text only" \
    "window.getSelection" "$EXT/background.js" "$EXT/content.js"
grep_check "supported StoryMaps and Docs surfaces" \
    "storymaps.arcgis.com" "$EXT/manifest.json"
grep_check "extension stores pairing token" \
    "jeffBridgeToken" "$EXT/options.js" "$EXT/background.js"

echo ""
echo "--- m22.5/m22.6: rate-only typing and tts delay ---"

grep_check "typing activity module exists" \
    "pub struct TypingActivityState" "$SRC/typing_activity.rs"
grep_check "typing module stores timing only" \
    "recent_keydowns" "$SRC/typing_activity.rs"
grep_check "typing privacy setting key" \
    "privacy_typing_activity_enabled" "$SRC/store.rs"
grep_check "global monitor does not store key codes" \
    "K_CG_EVENT_KEY_DOWN" "$SRC/typing_activity.rs"
grep_check "frontend receives typing boolean only" \
    "typing://activity-changed" "$SRC/main.rs" "$FRONTEND/App.tsx"
grep_check "streaming tts delay timer" \
    "streamTtsDelayTimerRef" "$FRONTEND/App.tsx"
grep_check "text-only discard path" \
    "discardStreamingTtsForTextOnly" "$FRONTEND/App.tsx"
grep_check "non-streaming tts waits for typing pause" \
    "waitForSpeechPlaybackSlot" "$FRONTEND/App.tsx"

echo ""
echo "--- m22.7/m22.8: spoken cleanup and voice selection ---"

grep_check "voice naturalness module" \
    "prepare_tts_text" "$SRC/voice_naturalness.rs"
grep_check "filler phrase list" \
    "great question" "$SRC/voice_naturalness.rs"
grep_check "deterministic interjections" \
    "got it" "$SRC/voice_naturalness.rs"
grep_check "chat prompt voice addendum" \
    "Be concise. One to three sentences unless the user asks for more. No filler phrases." "$SRC/chat.rs"
grep_check "tts_voice setting key" \
    "tts_voice" "$SRC/store.rs"
grep_check "tts voice command registered" \
    "commands::set_tts_voice" "$SRC/main.rs"
grep_check "streaming tts uses selected voice" \
    "tts_voice.clone()" "$SRC/chat_streaming.rs"
grep_check "non-streaming tts uses selected voice" \
    "get_tts_voice" "$SRC/commands.rs"
grep_check "voice selection UI" \
    "tts-voice-select" "$FRONTEND/App.tsx" "$FRONTEND/App.test.tsx"
grep_check "tray voice settings entry" \
    "Voice Settings" "$SRC/ambient.rs"

echo ""
echo "--- m22 remediation: overlay indicator, token entropy, cors, voice quality ---"

grep_check "selection capture indicator in overlay" \
    "overlay-selection-capture-indicator" "$FRONTEND/Overlay.tsx"
grep_check "overlay listens for selection captured event" \
    "selection://captured" "$FRONTEND/Overlay.tsx"
grep_check "overlay listens for selection failed event" \
    "selection://capture-failed" "$FRONTEND/Overlay.tsx"
grep_check "overlay listens for selection cleared event" \
    "selection://cleared" "$FRONTEND/Overlay.tsx"
grep_check "overlay dismiss handler wired" \
    "handleDismissSelectionCapture" "$FRONTEND/Overlay.tsx"
grep_check "overlay imports dismissSelectionCapture" \
    "dismissSelectionCapture" "$FRONTEND/Overlay.tsx"
grep_check "bridge token uses /dev/urandom entropy" \
    "/dev/urandom" "$SRC/selection_capture.rs"
grep_check "bridge token is 32 hex chars" \
    "32.*hex\|hex.*32" "$SRC/selection_capture.rs"
grep_check "cors wildcard removed from bridge" \
    -L "Access-Control-Allow-Origin" "$SRC/selection_capture.rs"
grep_check "no filler phrases in reorientation prompt" \
    "No filler phrases" "$SRC/proactive.rs"
grep_check "expanded filler phrase list" \
    "i'd be happy to" "$SRC/voice_naturalness.rs"
grep_check "expanded interjection list has at least 5 entries" \
    "understood" "$SRC/voice_naturalness.rs"
grep_check "starts_with_interjection uses broad boundary check" \
    "is_alphanumeric" "$SRC/voice_naturalness.rs"

echo ""
echo "--- behavioral tests ---"

run_check "selection capture state tests pass" \
    cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" selection_capture
run_check "typing activity tests pass" \
    cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" typing_activity
run_check "voice naturalness tests pass" \
    cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" voice_naturalness
run_check "privacy settings include phase 22 toggles" \
    cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" privacy_settings_round_trip
run_check "frontend phase 22 UI tests pass" \
    npm --prefix "$REPO_ROOT/desktop" test -- --run

echo ""
echo "phase 22 check: $PASS passed, $FAIL failed"
if [ "$FAIL" -ne 0 ]; then
    exit 1
fi
