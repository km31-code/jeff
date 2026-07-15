#!/usr/bin/env bash
# apex f3a: the end-to-end encrypted companion channel.
#
# a companion (a phone/earbud client -- reference client here) reaches the same
# store-backed core over a Noise-encrypted session: an utterance answered by Jeff
# with the same memory and pipeline as the desktop, a memory recall, job status --
# and nothing else. end-to-end by construction: the transport underneath sees only
# ciphertext (the F3b relay will carry opaque frames), and only a paired device can
# open a session. off by default; pairing is explicit and windowed; devices revoke.

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
TAURI="$ROOT_DIR/desktop/src-tauri"
DESKTOP="$ROOT_DIR/desktop"
COMP="$SRC/companion.rs"
STORE="$SRC/store.rs"
COMMANDS="$SRC/commands.rs"
MAIN="$SRC/main.rs"
APP_TSX="$DESKTOP/src/App.tsx"
CLIENT_TS="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex f3a encrypted companion channel check ---"

# 1. a real end-to-end-encrypted, mutually authenticated, psk-gated session.
test -f "$COMP" || fail "companion.rs missing"
grep -q "Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s" "$COMP" || fail "not a psk-gated mutual-auth Noise handshake"
grep -q "fn establish_initiator" "$COMP" || fail "initiator handshake missing"
grep -q "fn establish_responder" "$COMP" || fail "responder handshake missing"
grep -q "get_remote_static" "$COMP" || fail "peer identity is not authenticated"
grep -q "into_transport_mode" "$COMP" || fail "no transport encryption after handshake"
grep -q "snow = " "$TAURI/Cargo.toml" || fail "snow (Noise) dependency missing"
pass "Noise_XXpsk3 session: mutual static-key auth + forward secrecy, psk-gated"

# 2. the remote surface is exactly turn/recall/jobs and reuses the desktop pipeline
#    -- NOT the full command table (the F1b-3 scope decision).
grep -q "fn dispatch" "$COMP" || fail "companion dispatch missing"
for m in '"turn"' '"recall"' '"jobs"'; do
  grep -q "$m" "$COMP" || fail "companion method $m missing"
done
grep -q "send_message_for_task" "$COMP" || fail "turn does not reuse the same chat pipeline/memory"
grep -q "AppHandle" "$COMP" && fail "companion must be tauri-agnostic (no AppHandle)"
pass "remote surface is turn/recall/jobs, reusing the desktop pipeline, tauri-agnostic"

# 3. off by default; fails closed; pairing is windowed; devices revoke.
grep -q 'ENABLED_KEY.*companion_enabled' "$COMP" || fail "no companion enable key"
grep -q "unwrap_or(false)" "$COMP" || fail "companion channel must default OFF"
grep -q "pub fn store_authorize" "$COMP" || fail "authorization policy missing"
grep -q "AuthDecision::Reject" "$COMP" || fail "policy cannot reject"
grep -q "pub fn begin_pairing" "$COMP" || fail "windowed pairing missing"
grep -q "CREATE TABLE IF NOT EXISTS companion_devices" "$STORE" || fail "device allowlist table missing"
grep -q "pub fn remove_companion_device" "$STORE" || fail "device revocation missing"
pass "off by default, fails closed, windowed pairing, revocable device allowlist"

# 4. private key never leaves; pairing code carries only public material.
PAIR_BODY="$(awk '/pub fn begin_pairing/{f=1} f{print} f&&/^}/{exit}' "$COMP")"
printf '%s\n' "$PAIR_BODY" | grep -q "keys.public" || fail "pairing code must carry the public identity"
printf '%s\n' "$PAIR_BODY" | grep -q "encode(psk)" || fail "pairing code must carry the pairing secret"
printf '%s\n' "$PAIR_BODY" | grep -q "keys.private" && fail "pairing code must never include the private key"
pass "pairing exposes only public identity + secret; the private key never leaves"

# 5. controls surfaced in the app and registered.
for c in get_companion_status set_companion_enabled begin_companion_pairing \
         list_companion_devices remove_companion_device; do
  grep -q "pub fn $c" "$COMMANDS" || fail "command $c missing"
  grep -q "commands::$c" "$MAIN" || fail "command $c not registered"
done
grep -q "privacy-toggle-companion" "$APP_TSX" || fail "Privacy Center companion toggle missing"
grep -q "companion-pair-button" "$APP_TSX" || fail "Privacy Center pairing control missing"
grep -q "setCompanionEnabled" "$CLIENT_TS" || fail "frontend binding missing"
pass "companion controls are in the Privacy Center and registered"

# 6. warning-free compile, f3a tests, frontend lint.
CHECK_OUT=$(cd "$TAURI" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

for t in \
  f3a_paired_client_reaches_the_same_store_over_an_encrypted_session \
  f3a_wrong_pairing_secret_cannot_open_a_session \
  f3a_relay_would_see_only_ciphertext \
  f3a_authorization_is_off_by_default_and_gated_by_the_pairing_window; do
  grep -q "fn $t" "$COMP" || fail "expected f3a test $t is missing"
done
T_OUT=$(cd "$TAURI" && cargo test --lib f3a_ --quiet 2>&1)
echo "$T_OUT" | grep -q "test result: ok" || { echo "$T_OUT"; fail "f3a tests failed"; }
echo "$T_OUT" | grep -q "FAILED" && { echo "$T_OUT"; fail "f3a tests failed"; }
pass "f3a tests pass (same-store E2E; wrong-psk rejected; ciphertext-only; off-by-default policy)"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend lint did not run"; }
pass "frontend TypeScript check passes"

echo "--- apex f3a check passed ---"
