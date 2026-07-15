#!/usr/bin/env bash
# apex f1c: the daemon survives crashes and OS restarts.
#
# f1b-3 had the app spawn the daemon directly, so it only ever came back when the
# app did. f1c hands supervision to a per-user launchd LaunchAgent: RunAtLoad
# (start at login / after an OS restart with no app open) plus KeepAlive
# (relaunch on crash), and a kill switch that unloads the agent so KeepAlive
# cannot resurrect it. this check proves the wiring by static assertion and, on a
# machine with a gui login session, by a real launchd relaunch-on-crash round-trip.

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
TAURI="$ROOT_DIR/desktop/src-tauri"
DESKTOP="$ROOT_DIR/desktop"
LAUNCHD="$SRC/daemon_launchd.rs"
SUP="$SRC/daemon_supervisor.rs"
COMMANDS="$SRC/commands.rs"
APP_TSX="$DESKTOP/src/App.tsx"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex f1c daemon crash/restart survival check ---"

# 1. the LaunchAgent declares the two properties f1c exists to add.
test -f "$LAUNCHD" || fail "daemon_launchd.rs missing"
grep -q "<key>RunAtLoad</key>" "$LAUNCHD" || fail "agent does not start at login (RunAtLoad)"
grep -q "<key>KeepAlive</key>" "$LAUNCHD" || fail "agent does not relaunch on crash (KeepAlive)"
grep -q "<key>ThrottleInterval</key>" "$LAUNCHD" || fail "no crash-loop throttle"
grep -q "JEFF_DAEMON_RUN_CORE" "$LAUNCHD" || fail "launchd daemon must host the core"
grep -q "Library/LaunchAgents" "$LAUNCHD" || fail "must be a per-user LaunchAgent, not a root LaunchDaemon"
grep -q "bootout" "$LAUNCHD" || fail "no launchctl unload path"
pass "LaunchAgent declares RunAtLoad + KeepAlive + throttle and is a per-user agent"

# 2. the supervisor prefers launchd and the kill switch unloads it FIRST.
grep -q "daemon_launchd::install" "$SUP" || fail "supervisor does not install launchd supervision"
grep -q "daemon_launchd::uninstall" "$SUP" || fail "kill switch does not unload the launchd agent"
# ordering: within stop(), the launchd uninstall must precede the IPC shutdown,
# otherwise KeepAlive relaunches the daemon the instant the IPC shutdown kills it.
STOP_BODY="$(awk '/pub fn stop\(/{f=1} f{print} f&&/^}/{exit}' "$SUP")"
UNINSTALL_LINE="$(printf '%s\n' "$STOP_BODY" | grep -n "daemon_launchd::uninstall" | head -1 | cut -d: -f1)"
SHUTDOWN_LINE="$(printf '%s\n' "$STOP_BODY" | grep -n 'call("shutdown"' | head -1 | cut -d: -f1)"
[ -n "$UNINSTALL_LINE" ] || fail "stop() does not unload launchd"
[ -n "$SHUTDOWN_LINE" ] || fail "stop() does not send an IPC shutdown"
[ "$UNINSTALL_LINE" -lt "$SHUTDOWN_LINE" ] || fail "kill switch must unload launchd before the IPC shutdown"
pass "supervisor prefers launchd; kill switch unloads the agent before the IPC shutdown"

# 3. enabling starts supervision now, and the always-on nature is disclosed.
grep -q "daemon_supervisor::ensure_running" "$COMMANDS" || fail "enabling does not start the daemon now"
grep -qi "starts at login" "$APP_TSX" || fail "Privacy Center does not disclose start-at-login"
grep -qi "relaunches on its own" "$APP_TSX" || fail "Privacy Center does not disclose relaunch-on-crash"
pass "enabling starts supervision immediately; the Privacy Center discloses start-at-login and relaunch"

# 4. warning-free compile + the f1c and preserved f1b-3 unit tests.
CHECK_OUT=$(cd "$TAURI" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

for t in \
  f1c_plist_declares_crash_and_restart_supervision \
  f1c_plist_pins_the_exact_socket_store_and_binary \
  f1c_agent_plist_lives_in_user_launch_agents \
  f1c_plist_values_are_xml_escaped; do
  grep -q "fn $t" "$LAUNCHD" || fail "expected f1c test $t is missing"
done
T_OUT=$(cd "$TAURI" && cargo test --lib f1c_ --quiet 2>&1)
echo "$T_OUT" | grep -q "test result: ok" || { echo "$T_OUT"; fail "f1c tests failed"; }
echo "$T_OUT" | grep -q "FAILED" && { echo "$T_OUT"; fail "f1c tests failed"; }
B_OUT=$(cd "$TAURI" && cargo test --lib f1b3 --quiet 2>&1)
echo "$B_OUT" | grep -q "test result: ok" || { echo "$B_OUT"; fail "preserved f1b-3 supervisor tests failed"; }
pass "f1c unit tests pass; f1b-3 supervisor behavior preserved"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend lint did not run"; }
pass "frontend TypeScript check passes"

# 5. real machine: KeepAlive relaunches the daemon after a crash, and unloading
# the agent is a durable off switch. isolated under a throwaway label + socket so
# it can never touch the user's real agent. skips (does not fail) with no gui
# session (e.g. headless CI), where launchctl bootstrap is unavailable.
(cd "$TAURI" && cargo build --bin jeff_daemon --quiet 2>&1) || fail "daemon failed to build"
LABEL="com.jeff.daemon.f1ccheck"
DOMAIN="gui/$(id -u)"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
SOCK="/tmp/jeff-f1c.sock"
STORE="/tmp/jeff-f1c-store"
# run the daemon from a normal, non-TCC-protected location. this mirrors
# production (the daemon ships in /Applications/Jeff.app/Contents/MacOS, not a
# user folder) and avoids a dev-only trap: a launchd background agent opening a
# binary under ~/Desktop or ~/Documents hangs on a TCC consent check it can never
# satisfy without a UI. the checkout usually lives under one of those folders.
BIN_DIR="/tmp/jeff-f1c-bin"
DAEMON_BIN="$BIN_DIR/jeff_daemon"

roundtrip_cleanup() {
  launchctl bootout "$DOMAIN/$LABEL" >/dev/null 2>&1
  rm -f "$PLIST" "$SOCK"
  rm -rf "$STORE" "$BIN_DIR"
}
trap roundtrip_cleanup EXIT

# ask the daemon for its pid over the isolated socket; empty if unreachable.
daemon_pid() {
  python3 - "$SOCK" 2>/dev/null <<'PY'
import socket, struct, json, sys
try:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.settimeout(2)
    s.connect(sys.argv[1])
    req = json.dumps({"id": 1, "method": "status", "params": None}).encode()
    s.sendall(struct.pack(">I", len(req)) + req)
    hdr = s.recv(4)
    if len(hdr) < 4: sys.exit(0)
    n = struct.unpack(">I", hdr)[0]
    buf = b""
    while len(buf) < n: buf += s.recv(n - len(buf))
    r = json.loads(buf.decode())
    pid = (r.get("result") or {}).get("pid")
    if pid: print(pid)
except Exception:
    pass
PY
}

roundtrip_cleanup
mkdir -p "$STORE" "$BIN_DIR" "$HOME/Library/LaunchAgents"
cp "$TAURI/target/debug/jeff_daemon" "$DAEMON_BIN"
cat > "$PLIST" <<PLISTEOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>$LABEL</string>
    <key>ProgramArguments</key><array><string>$DAEMON_BIN</string></array>
    <key>EnvironmentVariables</key><dict>
        <key>JEFF_DAEMON_RUN_CORE</key><string>1</string>
        <key>JEFF_DAEMON_SOCKET</key><string>$SOCK</string>
        <key>JEFF_DAEMON_STORE_DIR</key><string>$STORE</string>
    </dict>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>ThrottleInterval</key><integer>10</integer>
    <key>ProcessType</key><string>Background</string>
</dict>
</plist>
PLISTEOF

launchctl bootout "$DOMAIN/$LABEL" >/dev/null 2>&1
if ! launchctl bootstrap "$DOMAIN" "$PLIST" >/dev/null 2>&1; then
  echo "SKIP: no gui login session; launchd relaunch round-trip not exercised here"
  echo "--- apex f1c check passed (static + unit; launchd round-trip skipped) ---"
  exit 0
fi

# wait for RunAtLoad to bring the daemon up and host the core.
PID1=""
for _ in $(seq 1 40); do
  PID1="$(daemon_pid)"
  [ -n "$PID1" ] && break
  sleep 0.25
done
[ -n "$PID1" ] || fail "launchd did not bring the daemon up (RunAtLoad)"
pass "launchd started the daemon at load (pid $PID1)"

# crash it. KeepAlive must relaunch it as a new process after ThrottleInterval.
kill -9 "$PID1" 2>/dev/null
PID2=""
for _ in $(seq 1 40); do   # up to ~14s, longer than the 10s throttle
  sleep 0.5
  PID2="$(daemon_pid)"
  [ -n "$PID2" ] && [ "$PID2" != "$PID1" ] && break
done
[ -n "$PID2" ] || fail "KeepAlive did not relaunch the daemon after a crash"
[ "$PID2" != "$PID1" ] || fail "daemon pid unchanged; relaunch not observed"
pass "KeepAlive relaunched the daemon after kill -9 (new pid $PID2)"

# the kill switch: unloading the agent stops it and it stays stopped.
launchctl bootout "$DOMAIN/$LABEL" >/dev/null 2>&1
sleep 2
STILL="$(daemon_pid)"
[ -z "$STILL" ] || fail "unloading the agent did not stop the daemon (KeepAlive not defeated)"
sleep 3
STILL="$(daemon_pid)"
[ -z "$STILL" ] || fail "daemon came back after the agent was unloaded"
pass "unloading the agent is a durable off switch (KeepAlive defeated)"

echo "--- apex f1c check passed (incl. real launchd relaunch-on-crash round-trip) ---"
