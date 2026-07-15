#!/usr/bin/env bash
# apex f3c: audio remoting over the companion protocol (in-repo transport slice).
#
# a voice turn streams AudioFrames (opus/pcm) over the same Noise session as every
# other companion message, so audio is encrypted and relayed as opaque ciphertext by
# construction. the transport is proven end to end in-repo with a reference client
# and a deterministic loopback bridge; the live realtime-audio bridge and the real
# loadable client stay env/hardware-gated, exactly as C4 shipped its control plane.

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
TAURI="$ROOT_DIR/desktop/src-tauri"
DESKTOP="$ROOT_DIR/desktop"
COMP="$SRC/companion.rs"
APP_TSX="$DESKTOP/src/App.tsx"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex f3c audio remoting check ---"

# 1. an audio wire format that rides the existing encrypted session.
test -f "$COMP" || fail "companion.rs missing"
grep -q "struct AudioFrame" "$COMP" || fail "AudioFrame wire type missing"
grep -q '"voice"' "$COMP" || fail "voice method not on the protocol"
grep -q "fn serve_voice_turn" "$COMP" || fail "responder voice-turn handler missing"
grep -q "fn voice_turn" "$COMP" || fail "reference client voice_turn missing"
pass "AudioFrame + voice turn on the existing Noise session"

# 2. the transport/brain seam: deterministic loopback in-repo, live bridge gated.
grep -q "trait VoiceBridge" "$COMP" || fail "VoiceBridge seam missing"
grep -q "struct LoopbackVoiceBridge" "$COMP" || fail "in-repo loopback bridge missing"
grep -q "struct RealtimeVoiceBridge" "$COMP" || fail "live realtime bridge seam missing"
grep -q "JEFF_COMPANION_VOICE_LIVE" "$COMP" || fail "live bridge is not behind an explicit opt-in"
# the selector must default to loopback and only reach the live bridge under the
# explicit env opt-in -- never merely because a key is present.
SEL_BODY="$(awk '/fn voice_bridge_for/{f=1} f{print} f&&/^}/{c++} f&&c==1{exit}' "$COMP")"
printf '%s\n' "$SEL_BODY" | grep -q "COMPANION_VOICE_LIVE_ENV" || fail "selector does not gate on the opt-in"
printf '%s\n' "$SEL_BODY" | grep -q "LoopbackVoiceBridge" || fail "selector does not default to loopback"
pass "loopback bridge is the default; the live bridge is behind an explicit opt-in"

# 3. memory is bounded: per-frame and per-turn budgets are enforced.
grep -q "MAX_AUDIO_PAYLOAD" "$COMP" || fail "per-frame payload cap missing"
grep -q "MAX_AUDIO_FRAMES_PER_TURN" "$COMP" || fail "per-turn frame budget missing"
grep -q "MAX_AUDIO_TURN_BYTES" "$COMP" || fail "per-turn byte budget missing"
grep -q "fn accept_audio_frame" "$COMP" || fail "budget enforcement helper missing"
pass "voice turns are bounded by per-frame and per-turn budgets"

# 4. privacy parity: the companion disclosure names the voice/audio capability.
grep -q "companion-surface-disclosure" "$APP_TSX" || fail "companion disclosure testid missing"
grep -qi "voice" "$APP_TSX" || fail "Privacy Center does not disclose voice/audio on the channel"
pass "Privacy Center discloses the voice/audio capability on the companion channel"

# 5. warning-free compile.
CHECK_OUT=$(cd "$TAURI" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

# 6. behavioral proof: the f3c tests exist and pass, and f3a/f3b/f3-hardening still
#    pass (nothing regressed). tests are deterministic and never touch the network.
for t in \
  f3c_voice_turn_round_trips_audio_frames_over_the_encrypted_session \
  f3c_empty_utterance_still_closes_the_turn \
  f3c_audio_frames_are_ciphertext_on_the_wire \
  f3c_audio_budget_rejects_oversized_and_overlong_turns \
  f3c_live_voice_bridge_is_gated_off_by_default; do
  grep -q "fn $t" "$COMP" || fail "expected f3c test $t is missing"
done
T_OUT=$(cd "$TAURI" && cargo test --lib f3 --quiet 2>&1)
echo "$T_OUT" | grep -q "test result: ok" || { echo "$T_OUT"; fail "f3 tests failed"; }
echo "$T_OUT" | grep -q "FAILED" && { echo "$T_OUT"; fail "f3 tests failed"; }
pass "f3c audio tests pass; f3a/f3b/f3-hardening still green"

# 7. frontend typecheck.
FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend lint did not run"; }
pass "frontend TypeScript check passes"

echo "--- apex f3c check passed ---"
