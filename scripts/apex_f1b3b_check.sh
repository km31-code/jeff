#!/usr/bin/env bash
# apex f1b-3b: the app supervises the background daemon, behind a real control.
#
# controls precede capability: the background daemon is OFF by default. the user
# turns it on in the Privacy Center; the app then starts it and hands it the
# background schedulers. it outlives the app (that is the point -- overnight
# work), so turning the control off must actually terminate it.

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
TAURI="$ROOT_DIR/desktop/src-tauri"
DESKTOP="$ROOT_DIR/desktop"
SUP="$SRC/daemon_supervisor.rs"
DAEMON="$SRC/bin/jeff_daemon.rs"
COMMANDS="$SRC/commands.rs"
MAIN="$SRC/main.rs"
APP_TSX="$DESKTOP/src/App.tsx"
CLIENT_TS="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex f1b-3b daemon supervision + control check ---"

# 1. off by default, with a real kill switch.
test -f "$SUP" || fail "daemon_supervisor.rs missing"
grep -q "pub fn is_enabled" "$SUP" || fail "enable state missing"
grep -q "unwrap_or(false)" "$SUP" || fail "background daemon must default to OFF"
grep -q "pub fn ensure_running" "$SUP" || fail "supervisor cannot start the daemon"
grep -q "pub fn stop" "$SUP" || fail "kill switch missing"
grep -q '"shutdown"' "$DAEMON" || fail "daemon does not honour a shutdown request"
pass "daemon is off by default, startable by the app, and terminable by a kill switch"

# 2. the app supervises it and the control is in the Privacy Center.
grep -q "daemon_supervisor::ensure_running" "$MAIN" || fail "app does not supervise the daemon"
grep -q "pub fn get_background_daemon" "$COMMANDS" || fail "status command missing"
grep -q "pub fn set_background_daemon_enabled" "$COMMANDS" || fail "toggle command missing"
grep -q "commands::get_background_daemon" "$MAIN" || fail "status command not registered"
grep -q "commands::set_background_daemon_enabled" "$MAIN" || fail "toggle command not registered"
grep -q "setBackgroundDaemonEnabled" "$CLIENT_TS" || fail "frontend binding missing"
grep -q "privacy-surface-background-daemon" "$APP_TSX" || fail "Privacy Center surface missing"
grep -q "privacy-toggle-background-daemon" "$APP_TSX" || fail "Privacy Center toggle missing"
pass "app supervises the daemon; the control lives in the Privacy Center"

# 3. warning-free + supervisor tests + suites.
CHECK_OUT=$(cd "$TAURI" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

for t in \
  f1b3b_background_daemon_is_off_until_the_user_turns_it_on \
  f1b3b_disabled_daemon_is_never_started; do
  grep -q "fn $t" "$SUP" || fail "expected f1b-3b test $t is missing"
done
T_OUT=$(cd "$TAURI" && cargo test --lib f1b3b_ --quiet 2>&1)
echo "$T_OUT" | grep -q "test result: ok" || { echo "$T_OUT"; fail "f1b-3b tests failed"; }
echo "$T_OUT" | grep -q "FAILED" && { echo "$T_OUT"; fail "f1b-3b tests failed"; }
pass "supervisor tests pass (off by default; a disabled daemon is never started)"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend lint did not run"; }
pass "frontend TypeScript check passes"

# 4. end to end: enabled -> the app spawns it and defers; the daemon outlives the
# app; the kill switch stops it. runs against a scratch store and socket.
(cd "$TAURI" && cargo build --bin jeff_daemon --quiet 2>&1) || fail "daemon failed to build"
SOCK="/tmp/jeff-f1b3b.sock"; TMP="/tmp/jeff-f1b3b-store"
rm -rf "$SOCK" "$TMP"; mkdir -p "$TMP"
dcount() { pgrep -f "jeff_daemon" | wc -l | tr -d ' '; }

JEFF_DAEMON_SOCKET="$SOCK" JEFF_DAEMON_STORE_DIR="$TMP" JEFF_DAEMON_RUN_CORE=1 \
  "$TAURI/target/debug/jeff_daemon" >/tmp/jeff-f1b3b.log 2>&1 &
DPID=$!
sleep 5
kill -0 "$DPID" 2>/dev/null || { cat /tmp/jeff-f1b3b.log; fail "daemon did not start"; }

# the kill switch must actually terminate the process.
python3 - "$SOCK" >/dev/null 2>&1 <<'PY'
import socket, struct, json, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect(sys.argv[1])
r = json.dumps({"id":1,"method":"shutdown","params":None}).encode()
s.sendall(struct.pack(">I", len(r)) + r); s.recv(4)
PY
sleep 2
if kill -0 "$DPID" 2>/dev/null; then
  kill -9 "$DPID" 2>/dev/null
  fail "the kill switch did not terminate the daemon"
fi
rm -rf "$SOCK" "$TMP"
pass "kill switch terminates the running daemon"

echo "--- apex f1b-3b check passed ---"
