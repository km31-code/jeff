#!/usr/bin/env bash
# apex c4 check: realtime voice sessions.
# Verifies the realtime session adapter (ephemeral mint), the VoiceSession
# abstraction + context assembly + tool routing + transcript persistence, the
# pipeline fallback, and the frontend WebRTC glue + controls. The live socket,
# audio, and latency (done-when 1-2) are env-gated: they need a key, a
# microphone, and reference hardware, exercised outside this check.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
REALTIME="$SRC/providers/realtime.rs"
VOICE_SESSION="$SRC/voice_session.rs"
COMMANDS="$SRC/commands.rs"
MAIN="$SRC/main.rs"
TAURI_CLIENT="$DESKTOP/src/tauriClient.ts"
VOICE_RT="$DESKTOP/src/voiceRealtime.ts"
OVERLAY="$DESKTOP/src/Overlay.tsx"
APP_TSX="$DESKTOP/src/App.tsx"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex c4 realtime voice sessions check ---"

# 1. realtime adapter: ephemeral mint + pure request/response.
test -f "$REALTIME" || fail "providers/realtime.rs missing"
grep -q "pub const REALTIME_MODEL" "$REALTIME" || fail "realtime model constant missing"
grep -q "pub fn build_session_request" "$REALTIME" || fail "session request builder missing"
grep -q "pub fn parse_session_response" "$REALTIME" || fail "session response parser missing"
grep -q "pub fn mint_realtime_session" "$REALTIME" || fail "ephemeral mint missing"
grep -q "route_request" "$REALTIME" || fail "tool surface missing from session request"
grep -q "pub mod realtime;" "$SRC/providers.rs" || fail "realtime provider not registered"
pass "realtime adapter: model, mint, and pure request/response present"

# 2. VoiceSession abstraction + context + tool routing + persistence + fallback.
test -f "$VOICE_SESSION" || fail "voice_session.rs missing"
grep -q "pub trait VoiceSession" "$VOICE_SESSION" || fail "VoiceSession trait missing"
grep -q "pub fn build_session_instructions" "$VOICE_SESSION" || fail "session context assembly missing"
grep -q "pub fn route_voice_tool_call" "$VOICE_SESSION" || fail "tool-call routing missing"
grep -q "RouteAsText" "$VOICE_SESSION" || fail "spoken-request-as-text mapping missing"
grep -q "pub fn persist_voice_turn" "$VOICE_SESSION" || fail "transcript persistence missing"
grep -q "append_chat_message" "$VOICE_SESSION" || fail "voice turns not persisted as chat messages"
grep -q "pub fn should_use_realtime" "$VOICE_SESSION" || fail "fallback decision missing"
pass "VoiceSession trait, context assembly, tool routing, persistence, fallback present"

# 3. commands: config, mint-with-context, transcript, tool-call.
grep -q "pub fn start_voice_session" "$COMMANDS" || fail "start_voice_session command missing"
grep -q "build_session_instructions" "$COMMANDS" || fail "start does not assemble character+context"
grep -q "should_use_realtime" "$COMMANDS" || fail "start does not gate on enabled+key"
grep -q "pub fn persist_voice_transcript" "$COMMANDS" || fail "persist_voice_transcript command missing"
grep -q "pub fn handle_voice_tool_call" "$COMMANDS" || fail "handle_voice_tool_call command missing"
grep -q "pub fn set_voice_config" "$COMMANDS" || fail "voice config command missing"
grep -q "mod voice_session;" "$MAIN" || fail "voice_session module not registered"
grep -q "commands::start_voice_session" "$MAIN" || fail "voice commands not registered"
pass "voice commands assemble context, mint, persist transcripts, and route tool-calls"

# 4. model-string gate holds (realtime model lives in providers/ only).
LEAK=$(grep -rn "gpt-4o-realtime" "$SRC" --include="*.rs" | grep -v "providers/" || true)
if [ -n "$LEAK" ]; then
  echo "$LEAK"
  fail "realtime model string leaked outside providers/"
fi
pass "realtime model string stays inside providers/ (a1 gate)"

# 5. frontend: bindings, WebRTC glue, controls.
grep -q "startVoiceSession" "$TAURI_CLIENT" || fail "frontend start binding missing"
grep -q "persistVoiceTranscript" "$TAURI_CLIENT" || fail "frontend transcript binding missing"
grep -q "handleVoiceToolCall" "$TAURI_CLIENT" || fail "frontend tool-call binding missing"
test -f "$VOICE_RT" || fail "voiceRealtime.ts (WebRTC glue) missing"
grep -q "connectRealtimeVoice" "$VOICE_RT" || fail "WebRTC connect helper missing"
grep -q "realtimeVoiceSupported" "$VOICE_RT" || fail "capability guard missing"
grep -q "RTCPeerConnection" "$VOICE_RT" || fail "WebRTC peer connection missing"
grep -q "voice-session-control" "$OVERLAY" || fail "overlay voice control missing"
grep -q "Shift" "$OVERLAY" || fail "push-to-talk hotkey missing"
grep -q "connectRealtimeVoice" "$OVERLAY" || fail "overlay does not open the realtime connection"
grep -q "privacy-toggle-voice" "$APP_TSX" || fail "voice enable toggle missing from Privacy Center"
pass "frontend bindings, capability-guarded WebRTC glue, overlay control, and toggle present"

# 6. behavioral: clean compile, c4 tests, frontend, a1 gate.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

C4_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test c4_ --quiet 2>&1)
echo "$C4_TEST_OUT" | grep -q "test result: ok" || { echo "$C4_TEST_OUT"; fail "c4 tests failed"; }
echo "$C4_TEST_OUT" | grep -q "FAILED" && { echo "$C4_TEST_OUT"; fail "c4 tests failed"; }
pass "c4 request/response, context, tool-routing, persistence, and fallback tests pass"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend TypeScript check did not run"; }
pass "frontend TypeScript check passes"

bash "$ROOT_DIR/scripts/apex_a1_check.sh" >/dev/null 2>&1 || fail "apex a1 model-router/model-string gate regressed"
pass "apex a1 router + model-string gate still passes"

echo "SKIP: live realtime session, audio round-trip latency, and the 3-minute"
echo "      zero-typing session are env-gated (need a key, a microphone, and"
echo "      reference hardware). The mint, context, routing, persistence, and"
echo "      fallback are covered above."
echo "--- apex c4 check passed ---"
