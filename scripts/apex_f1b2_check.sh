#!/usr/bin/env bash
# apex f1b-2: local IPC transport + jeff-daemon binary skeleton. the wire the
# f1b-3 process split runs on -- a unix-socket framed-json request/response
# protocol plus a server->client event stream -- built and proven in isolation,
# off the shipping app's critical path (nothing in the app links it yet).

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
IPC="$SRC/daemon_ipc.rs"
DAEMON_BIN="$SRC/bin/jeff_daemon.rs"
LIB="$SRC/lib.rs"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex f1b-2 ipc transport + daemon skeleton check ---"

# 1. the transport module exists in the shareable lib (so both the daemon bin and
# the app-side client can use it) with the full protocol surface.
test -f "$IPC" || fail "daemon_ipc.rs missing"
grep -q "pub mod daemon_ipc;" "$LIB" || fail "daemon_ipc not exported from the lib"
grep -q "pub const PROTOCOL_VERSION" "$IPC" || fail "versioned protocol constant missing"
for ty in IpcRequest IpcResponse IpcEvent IpcServer IpcClient; do
  grep -q "pub struct $ty" "$IPC" || fail "$ty type missing"
done
grep -q "pub trait IpcHandler" "$IPC" || fail "IpcHandler dispatch trait missing"
grep -q "pub fn write_frame" "$IPC" || fail "framing writer missing"
grep -q "pub fn read_frame" "$IPC" || fail "framing reader missing"
grep -q "pub fn broadcast" "$IPC" || fail "event broadcast missing"
grep -q "UnixListener" "$IPC" || fail "unix-domain-socket transport missing"
pass "daemon_ipc transport (framing, request/response, event stream) in the lib"

# 2. the daemon binary skeleton exists and uses the lib transport + handshake.
test -f "$DAEMON_BIN" || fail "jeff_daemon bin missing"
grep -q "jeff_desktop::daemon_ipc" "$DAEMON_BIN" || fail "daemon does not use the lib transport"
grep -q "IpcServer::bind" "$DAEMON_BIN" || fail "daemon does not bind the ipc server"
grep -q "handshake" "$DAEMON_BIN" || fail "daemon does not answer the handshake"
pass "jeff_daemon binary binds the socket and serves the handshake"

# 3. warning-free build (the new bin + module must not regress the workspace).
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

# 4. transport tests prove request/response, error, event push, and framing.
for t in \
  f1b2_request_response_round_trips_over_the_socket \
  f1b2_unknown_method_returns_a_structured_error \
  f1b2_server_pushes_events_to_a_listening_client \
  f1b2_frame_round_trips_preserve_bytes; do
  grep -q "fn $t" "$IPC" || fail "expected f1b-2 test $t is missing"
done
IPC_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test --lib f1b2_ --quiet 2>&1)
echo "$IPC_TEST_OUT" | grep -q "test result: ok" || { echo "$IPC_TEST_OUT"; fail "f1b-2 transport tests failed"; }
echo "$IPC_TEST_OUT" | grep -q "FAILED" && { echo "$IPC_TEST_OUT"; fail "f1b-2 transport tests failed"; }
pass "ipc transport tests pass (request/response, error, event stream, framing)"

# 5. end-to-end: the built daemon binds a socket and answers a real framed
# handshake from an external client.
DAEMON_BUILD=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo build --bin jeff_daemon --quiet 2>&1)
[ -z "$DAEMON_BUILD" ] || { echo "$DAEMON_BUILD"; fail "jeff_daemon build failed"; }
SOCK="/tmp/jeff-daemon-f1b2-check.sock"
rm -f "$SOCK"
JEFF_DAEMON_SOCKET="$SOCK" "$ROOT_DIR/desktop/src-tauri/target/debug/jeff_daemon" >/dev/null 2>&1 &
DAEMON_PID=$!
sleep 1
HANDSHAKE=$(python3 - "$SOCK" <<'PY'
import socket, struct, json, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
try:
    s.connect(sys.argv[1])
    req = json.dumps({"id":1,"method":"handshake","params":None}).encode()
    s.sendall(struct.pack(">I", len(req)) + req)
    ln = struct.unpack(">I", s.recv(4))[0]
    body = b""
    while len(body) < ln:
        body += s.recv(ln - len(body))
    print(json.loads(body)["result"]["protocol"])
except Exception as e:
    print("ERR", e)
PY
)
kill "$DAEMON_PID" 2>/dev/null
rm -f "$SOCK"
[ "$HANDSHAKE" = "1" ] || { echo "handshake returned: $HANDSHAKE"; fail "daemon did not answer a real framed handshake"; }
pass "built jeff_daemon answers a real framed handshake over the socket"

# 6. full suites still green.
TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test --quiet 2>&1)
echo "$TEST_OUT" | grep -q "FAILED" && { echo "$TEST_OUT" | tail -20; fail "backend test suite has failures"; }
echo "$TEST_OUT" | grep -q "test result: ok" || { echo "$TEST_OUT" | tail -20; fail "backend test suite did not report ok"; }
pass "full backend test suite passes"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend lint did not run"; }
pass "frontend TypeScript check passes"

# 7. adjacent gate: the full Apex spine (the app is untouched by f1b-2).
if [ "${JEFF_SKIP_ADJACENT_GATES:-0}" != "1" ]; then
  if ! E7_OUT=$(JEFF_SKIP_ADJACENT_GATES=1 bash "$ROOT_DIR/scripts/apex_e7_check.sh" 2>&1); then
    echo "$E7_OUT"
    fail "apex e7 ship gate regressed"
  fi
  pass "apex e7 ship gate still passes"
fi

echo "--- apex f1b-2 check passed ---"
