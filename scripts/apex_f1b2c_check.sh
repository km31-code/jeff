#!/usr/bin/env bash
# apex f1b-2c: the headless CoreHost. DaemonHost implements the core's I/O seam
# without tauri (owns the world model, delivers events over the IPC stream), so
# jeff_daemon can run core_runtime's schedulers with no AppHandle and no webview.
# proven end to end: the daemon boots the core headless against a temp store and
# survives a SIGKILL of the app.

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
TAURI="$ROOT_DIR/desktop/src-tauri"
HOST="$SRC/daemon_host.rs"
DAEMON="$SRC/bin/jeff_daemon.rs"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex f1b-2c headless core host check ---"

# 1. a non-tauri CoreHost exists and owns the world model directly.
test -f "$HOST" || fail "daemon_host.rs missing"
grep -q "pub mod daemon_host;" "$SRC/lib.rs" || fail "daemon_host not exported from the lib"
grep -q "pub struct DaemonHost" "$HOST" || fail "DaemonHost missing"
grep -q "impl CoreHost for DaemonHost" "$HOST" || fail "DaemonHost does not implement the CoreHost seam"
grep -q "state: JeffState" "$HOST" || fail "DaemonHost does not own the world model"
grep -q "sink: EventSink" "$HOST" || fail "DaemonHost does not deliver events over the IPC stream"
# no AppHandle in the headless host's code (the module note may mention it).
grep -vE "^\s*//" "$HOST" | grep -q "AppHandle" && fail "DaemonHost still references an AppHandle"
pass "DaemonHost implements CoreHost with no AppHandle (owns state, emits over IPC)"

# 2. the world model and the pure crisis detectors are driven headless.
grep -q "update_with_context" "$HOST" || fail "daemon does not run the awareness update"
grep -q "crisis_core::detect_meeting_imminent" "$HOST" || fail "daemon does not detect meeting crises"
grep -q "crisis_core::detect_deadline_collision" "$HOST" || fail "daemon does not detect deadline crises"
pass "daemon runs the world model and the deterministic crisis detectors"

# 3. the daemon binary can host the core, opt-in so it cannot split-brain the app.
grep -q "core_runtime::start" "$DAEMON" || fail "daemon does not start the core"
grep -q "DaemonHost::new" "$DAEMON" || fail "daemon does not build a DaemonHost"
grep -q "JEFF_DAEMON_RUN_CORE" "$DAEMON" || fail "core hosting is not opt-in (split-brain risk)"
grep -q "JEFF_DAEMON_STORE_DIR" "$DAEMON" || fail "store dir override missing"
pass "jeff_daemon hosts the core behind an explicit opt-in, with a store override"

# 4. warning-free build + the headless host tests.
CHECK_OUT=$(cd "$TAURI" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

for t in \
  f1b2c_daemon_host_implements_the_core_seam_without_tauri \
  f1b2c_daemon_host_relays_crises_over_ipc_instead_of_a_webview; do
  grep -q "fn $t" "$HOST" || fail "expected f1b-2c test $t is missing"
done
T_OUT=$(cd "$TAURI" && cargo test --lib f1b2c_ --quiet 2>&1)
echo "$T_OUT" | grep -q "test result: ok" || { echo "$T_OUT"; fail "f1b-2c tests failed"; }
echo "$T_OUT" | grep -q "FAILED" && { echo "$T_OUT"; fail "f1b-2c tests failed"; }
pass "DaemonHost answers the core seam headless (no tauri runtime)"

# 5. end to end: the built daemon boots the core headless against a temp store,
# and survives a SIGKILL of the app with its core still running.
(cd "$TAURI" && cargo build --bin jeff_daemon --bin jeff-desktop --quiet 2>&1) || fail "binaries failed to build"
TMP="/tmp/jeff-f1b2c-store"; SOCK="/tmp/jeff-f1b2c.sock"
rm -rf "$TMP" "$SOCK"; mkdir -p "$TMP"
JEFF_DAEMON_SOCKET="$SOCK" JEFF_DAEMON_STORE_DIR="$TMP" JEFF_DAEMON_RUN_CORE=1 \
  "$TAURI/target/debug/jeff_daemon" >/tmp/jeff-f1b2c-daemon.log 2>&1 &
DPID=$!
sleep 5
kill -0 "$DPID" 2>/dev/null || { cat /tmp/jeff-f1b2c-daemon.log; fail "daemon died starting the core"; }
grep -q "core running headless" /tmp/jeff-f1b2c-daemon.log || { cat /tmp/jeff-f1b2c-daemon.log; fail "daemon did not host the core"; }
test -f "$TMP/jeff_store.sqlite3" || fail "daemon did not build the world model store"

STATUS=$(python3 - "$SOCK" <<'PY'
import socket, struct, json, sys
try:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect(sys.argv[1])
    r = json.dumps({"id":1,"method":"status","params":None}).encode()
    s.sendall(struct.pack(">I", len(r)) + r)
    n = struct.unpack(">I", s.recv(4))[0]; b = b""
    while len(b) < n: b += s.recv(n - len(b))
    print(json.loads(b)["result"]["core_running"])
except Exception as e:
    print("ERR", e)
PY
)
[ "$STATUS" = "True" ] || { kill -9 "$DPID" 2>/dev/null; fail "daemon does not report a running core (got: $STATUS)"; }

# the app crashing must not take the daemon's core down.
pkill -9 -x jeff-desktop 2>/dev/null
sleep 2
kill -0 "$DPID" 2>/dev/null || { fail "daemon did not survive the app being killed"; }
kill -9 "$DPID" 2>/dev/null
rm -rf "$TMP" "$SOCK"
pass "daemon boots the core headless, serves status, and survives a SIGKILL of the app"

echo "--- apex f1b-2c check passed ---"
