#!/usr/bin/env bash
# apex c5 check: opt-in wake word sidecar.
# Verifies the sidecar lifecycle, token-only IPC privacy boundary, armed tray
# and overlay indicators, Privacy Center control, and wake-to-voice frontend path.
# Live microphone detection is intentionally outside this static/local gate.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
WAKE="$SRC/wake_word.rs"
AMBIENT="$SRC/ambient.rs"
COMMANDS="$SRC/commands.rs"
MODELS="$SRC/models.rs"
MAIN="$SRC/main.rs"
APP_TSX="$DESKTOP/src/App.tsx"
OVERLAY="$DESKTOP/src/Overlay.tsx"
TAURI_CLIENT="$DESKTOP/src/tauriClient.ts"
AMBIENT_CLIENT="$DESKTOP/src/ambientClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex c5 wake word sidecar check ---"

# 1. sidecar lifecycle is opt-in and externally configured.
test -f "$WAKE" || fail "wake_word.rs missing"
grep -q "pub struct WakeWordManager" "$WAKE" || fail "WakeWordManager missing"
grep -q "WAKE_WORD_ENABLED_KEY" "$WAKE" || fail "persisted enable key missing"
grep -q "WAKE_WORD_COMMAND_ENV" "$WAKE" || fail "detector command env missing"
grep -q "pub fn load_enabled" "$WAKE" || fail "opt-in loader missing"
grep -q "pub fn maybe_start_from_settings" "$WAKE" || fail "startup restore missing"
grep -q "pub fn set_enabled" "$WAKE" || fail "enable/disable lifecycle missing"
grep -q "child.kill" "$WAKE" || fail "disable path does not kill sidecar"
grep -q "child.wait" "$WAKE" || fail "disable path does not reap sidecar"
grep -q "maybe_start_from_settings" "$MAIN" || fail "startup hook not wired"
pass "sidecar lifecycle is opt-in, restarts from settings, and disables by killing/reaping"

# 2. privacy boundary: only wake tokens cross IPC; no raw microphone/audio pipe.
grep -q "Stdio::null" "$WAKE" || fail "detector stdin is not closed/null"
grep -q "stdout(Stdio::piped" "$WAKE" || fail "token stdout pipe missing"
grep -q "no_raw_audio_ipc: true" "$WAKE" || fail "status does not assert no raw audio ipc"
grep -q "is_wake_word_signal" "$WAKE" || fail "wake token parser missing"
grep -q "pcm:abcdef" "$WAKE" || fail "negative raw-audio token test missing"
grep -q "transcript hello" "$WAKE" || fail "negative transcript token test missing"
if grep -Eq "audio_base64|MediaRecorder|getUserMedia" "$WAKE"; then
  fail "wake_word.rs appears to handle raw audio directly"
fi
pass "pre-wake audio stays in detector process; Jeff accepts wake tokens only"

# 3. commands, models, dashboard, and startup are wired.
grep -q "pub struct WakeWordStatusDto" "$MODELS" || fail "WakeWordStatusDto missing"
grep -q "pub wake_word: WakeWordStatusDto" "$MODELS" || fail "Privacy Center dashboard lacks wake_word status"
grep -q "pub fn get_wake_word_status" "$COMMANDS" || fail "get_wake_word_status command missing"
grep -q "pub fn set_wake_word_enabled" "$COMMANDS" || fail "set_wake_word_enabled command missing"
grep -q "commands::get_wake_word_status" "$MAIN" || fail "get_wake_word_status not registered"
grep -q "commands::set_wake_word_enabled" "$MAIN" || fail "set_wake_word_enabled not registered"
grep -q "wake_word: state.wake_word.status" "$COMMANDS" || fail "dashboard does not report wake status"
pass "wake-word commands, DTOs, dashboard, and Tauri registration are present"

# 4. truthful armed indicator in ambient state and tray.
grep -q "wake_word_armed" "$AMBIENT" || fail "ambient armed state missing"
grep -q "update_wake_word_armed" "$AMBIENT" || fail "armed state updater missing"
grep -q "wake word armed" "$AMBIENT" || fail "tray tooltip armed text missing"
grep -q "ambient://state-changed" "$WAKE" "$AMBIENT" || fail "armed state event missing"
grep -q "wake_word_armed" "$AMBIENT_CLIENT" || fail "frontend ambient state lacks armed field"
grep -q "wake-word-armed" "$OVERLAY" || fail "overlay armed indicator missing"
pass "armed state propagates to tray and overlay"

# 5. detection opens the overlay and starts voice with an audible local ack.
grep -q "WAKE_WORD_DETECTED_EVENT" "$WAKE" || fail "wake detected event constant missing"
grep -q "wake_word://detected" "$WAKE" "$OVERLAY" || fail "wake detected event not shared with overlay"
grep -q "show_overlay_interactive" "$WAKE" || fail "wake detection does not open overlay"
grep -q "playWakeWordAckCue" "$OVERLAY" || fail "audible acknowledgement cue missing"
grep -q "openVoiceSessionFromWakeWord" "$OVERLAY" || fail "wake-to-voice opener missing"
grep -q "voiceConnRef.current" "$OVERLAY" || fail "wake path does not guard already-open voice session"
grep -q "startVoiceSession" "$OVERLAY" || fail "wake path cannot open voice session"
pass "wake token expands overlay, plays ack, and opens voice without toggling off live voice"

# 6. Privacy Center control reports configured/running/armed/pid/no-raw-audio state.
grep -q "WakeWordStatusDto" "$TAURI_CLIENT" || fail "frontend wake status type missing"
grep -q "getWakeWordStatus" "$TAURI_CLIENT" || fail "frontend getWakeWordStatus binding missing"
grep -q "setWakeWordEnabled" "$TAURI_CLIENT" || fail "frontend setWakeWordEnabled binding missing"
grep -q "privacy-surface-wake-word" "$APP_TSX" || fail "Privacy Center wake-word surface missing"
grep -q "privacy-toggle-wake-word" "$APP_TSX" || fail "Privacy Center wake-word toggle missing"
grep -q "wake-word-status" "$APP_TSX" || fail "wake-word status line missing"
grep -q "wake-word-privacy-guarantee" "$APP_TSX" || fail "privacy guarantee text missing"
pass "Privacy Center exposes explicit opt-in control and status"

# 7. behavioral checks: compile, focused tests, frontend, and adjacent gates.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

C5_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test c5_ --quiet 2>&1)
echo "$C5_TEST_OUT" | grep -q "test result: ok" || { echo "$C5_TEST_OUT"; fail "c5 tests failed"; }
echo "$C5_TEST_OUT" | grep -q "FAILED" && { echo "$C5_TEST_OUT"; fail "c5 tests failed"; }
pass "c5 opt-in, parser, and kill/reap tests pass"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend TypeScript check did not run"; }
pass "frontend TypeScript check passes"

bash "$ROOT_DIR/scripts/apex_a1_check.sh" >/dev/null 2>&1 || fail "apex a1 model-router/model-string gate regressed"
pass "apex a1 router + model-string gate still passes"

bash "$ROOT_DIR/scripts/apex_c4_check.sh" >/dev/null 2>&1 || fail "apex c4 realtime voice gate regressed"
pass "apex c4 realtime voice gate still passes"

echo "SKIP: live wake-word microphone detection requires the external detector"
echo "      configured by JEFF_WAKE_WORD_COMMAND and a real microphone. This gate"
echo "      verifies opt-in control, token-only IPC, lifecycle, UI, and wake-to-voice."
echo "--- apex c5 check passed ---"
