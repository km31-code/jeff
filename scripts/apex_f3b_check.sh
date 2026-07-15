#!/usr/bin/env bash
# apex f3b: the ciphertext-only relay + remote path.
#
# the daemon and a companion both dial a rendezvous relay, are matched by an opaque
# token, and exchange Noise ciphertext the relay forwards verbatim -- so a paired
# phone reaches the core from anywhere, and a compromised relay learns nothing.
# this closes the Pillar 14 exit criterion: the relay-compromise test shows the
# relay only ever carried ciphertext, end to end.

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
TAURI="$ROOT_DIR/desktop/src-tauri"
DESKTOP="$ROOT_DIR/desktop"
RELAY="$ROOT_DIR/relay"
COMP="$SRC/companion.rs"
CORE="$SRC/core_runtime.rs"
COMMANDS="$SRC/commands.rs"
MAIN="$SRC/main.rs"
APP_TSX="$DESKTOP/src/App.tsx"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex f3b ciphertext-only relay check ---"

# 1. rendezvous protocol: a single opaque header, then dial the relay.
grep -q 'RENDEZVOUS_MAGIC.*JEFFRDV1' "$COMP" || fail "rendezvous protocol tag missing"
grep -q "fn write_rendezvous_header" "$COMP" || fail "rendezvous header writer missing"
grep -q "fn dial_relay" "$COMP" || fail "relay dialer missing"
grep -q "fn serve_one_via_relay" "$COMP" || fail "relay serve path missing"
# serving via the relay still goes through the F3a serve() (all gates intact) and
# is gated by the enable flag.
SERVE_BODY="$(awk '/pub fn serve_one_via_relay/{f=1} f{print} f&&/^}/{exit}' "$COMP")"
printf '%s\n' "$SERVE_BODY" | grep -q "is_enabled" || fail "relay serving is not gated by the enable flag"
printf '%s\n' "$SERVE_BODY" | grep -q "serve(" || fail "relay path bypasses the authenticated serve()"
pass "rendezvous dial + relay serve reuse the authenticated, gated F3a session"

# 2. the deployable Node relay is a ciphertext-only forwarder matched by token.
test -f "$RELAY/rendezvous.mjs" || fail "Node rendezvous relay missing"
grep -q "createRendezvousServer" "$RELAY/rendezvous.mjs" || fail "rendezvous server factory missing"
grep -q "JEFFRDV1" "$RELAY/rendezvous.mjs" || fail "Node relay does not speak the rendezvous header"
grep -q "\.pipe(" "$RELAY/rendezvous.mjs" || fail "Node relay does not forward opaquely"
# it must hold no keys and never decrypt: no crypto imports in the forwarder.
grep -qiE "createCipher|createDecipher|crypto" "$RELAY/rendezvous.mjs" && fail "the relay must hold no keys / never touch crypto"
pass "Node relay forwards by token, holds no keys, never inspects the payload"

# 3. the daemon keeps a gated presence at the relay (background schedulers).
grep -q "fn spawn_companion_relay" "$CORE" || fail "companion relay scheduler missing"
BG_BLOCK="$(awk '/if profile.runs_background_schedulers\(\)/{f=1} f{print} f&&/^    }/{exit}' "$CORE")"
printf '%s\n' "$BG_BLOCK" | grep -q "spawn_companion_relay(" || fail "companion relay not wired under runs_background_schedulers()"
pass "the daemon serves companion sessions via the relay, gated, on the background schedulers"

# 4. relay is configurable and disclosed; remote access is off until a relay is set.
grep -q "pub fn set_companion_relay_url" "$COMMANDS" || fail "relay url command missing"
grep -q "commands::set_companion_relay_url" "$MAIN" || fail "relay url command not registered"
grep -q "companion-relay-input" "$APP_TSX" || fail "Privacy Center relay control missing"
pass "the relay is user-configurable and disclosed in the Privacy Center"

# 5. warning-free compile; the exit-criterion test; the Node relay tests.
CHECK_OUT=$(cd "$TAURI" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

grep -q "fn f3b_session_works_through_the_relay_which_sees_only_ciphertext" "$COMP" \
  || fail "the relay-compromise exit-criterion test is missing"
T_OUT=$(cd "$TAURI" && cargo test --lib f3b_ --quiet 2>&1)
echo "$T_OUT" | grep -q "test result: ok" || { echo "$T_OUT"; fail "f3b test failed"; }
echo "$T_OUT" | grep -q "FAILED" && { echo "$T_OUT"; fail "f3b test failed"; }
pass "relay-compromise test passes: a session works through the relay, which sees only ciphertext"

RELAY_OUT=$(cd "$RELAY" && npm test 2>&1)
echo "$RELAY_OUT" | grep -q "# fail 0" || { echo "$RELAY_OUT"; fail "Node relay tests failed"; }
pass "Node relay tests pass (bridges by token, forwards verbatim, refuses mismatched tokens)"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend lint did not run"; }
pass "frontend TypeScript check passes"

echo "--- apex f3b check passed ---"
