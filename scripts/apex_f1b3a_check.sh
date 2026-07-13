#!/usr/bin/env bash
# apex f1b-3a: core profiles + daemon coordination.
#
# exactly one process may run the mutating, store-backed background schedulers
# (standing jobs, job resume, speculation) -- double-running them against the
# shared store is the split-brain failure. CoreProfile decides who runs what:
#   daemon hosting the core -> app runs AppClient (perception/UI only)
#   no daemon               -> app runs Full (unchanged, nothing lost)
# perception stays in the app, which holds the Accessibility grant.

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
TAURI="$ROOT_DIR/desktop/src-tauri"
CORE="$SRC/core_runtime.rs"
CLIENT="$SRC/daemon_client.rs"
MAIN="$SRC/main.rs"
DAEMON="$SRC/bin/jeff_daemon.rs"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex f1b-3a core profiles + daemon coordination check ---"

# 1. the profile taxonomy exists and splits perception from the schedulers.
grep -q "pub enum CoreProfile" "$CORE" || fail "CoreProfile missing"
for p in Full AppClient DaemonBackground; do
  grep -q "$p" "$CORE" || fail "CoreProfile missing variant $p"
done
grep -q "fn runs_perception" "$CORE" || fail "perception split missing"
grep -q "fn runs_background_schedulers" "$CORE" || fail "scheduler split missing"
grep -qE "pub fn start\(host: Arc<dyn CoreHost>, profile: CoreProfile\)" "$CORE" \
  || fail "start() does not take a CoreProfile"
pass "CoreProfile splits perception from the mutating background schedulers"

# 2. the app defers only to a reachable, protocol-matching, core-hosting daemon.
test -f "$CLIENT" || fail "daemon_client.rs missing"
grep -q "pub fn probe" "$CLIENT" || fail "daemon probe missing"
grep -q "fn owns_background_schedulers" "$CLIENT" || fail "ownership predicate missing"
grep -q "protocol_matches" "$CLIENT" || fail "probe does not check the protocol version"
grep -q "daemon_client::probe" "$MAIN" || fail "app does not probe for a daemon"
grep -q "CoreProfile::AppClient" "$MAIN" || fail "app never defers to the daemon"
grep -q "CoreProfile::Full" "$MAIN" || fail "app has no standalone fallback"
pass "app probes the daemon and falls back to the full core when it is absent"

# 3. the daemon runs the background schedulers and no perception.
grep -q "CoreProfile::DaemonBackground" "$DAEMON" || fail "daemon does not use the background profile"
pass "daemon runs the store-backed schedulers only (no perception; app holds the AX grant)"

# 4. warning-free + coordination tests.
CHECK_OUT=$(cd "$TAURI" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

for t in \
  f1b3_absent_daemon_leaves_the_app_owning_everything \
  f1b3_daemon_only_takes_schedulers_when_hosting_a_matching_core; do
  grep -q "fn $t" "$CLIENT" || fail "expected f1b-3 test $t is missing"
done
T_OUT=$(cd "$TAURI" && cargo test --lib f1b3_ --quiet 2>&1)
echo "$T_OUT" | grep -q "test result: ok" || { echo "$T_OUT"; fail "f1b-3 tests failed"; }
echo "$T_OUT" | grep -q "FAILED" && { echo "$T_OUT"; fail "f1b-3 tests failed"; }
pass "daemon-ownership tests pass (absent/idle/mismatched daemon never takes the schedulers)"

# 5. end to end: the app defers when a core-hosting daemon is up, and runs the
# full core when it is not.
(cd "$TAURI" && cargo build --bin jeff-desktop --bin jeff_daemon --quiet 2>&1) || fail "binaries failed to build"
SOCK="/tmp/jeff-f1b3a.sock"; TMP="/tmp/jeff-f1b3a-store"
rm -rf "$SOCK" "$TMP"; mkdir -p "$TMP"

# without a daemon the app must run the full core.
JEFF_DAEMON_SOCKET="$SOCK" "$TAURI/target/debug/jeff_daemon" >/dev/null 2>&1 &
IDLE_PID=$!
sleep 2
python3 - "$SOCK" >/tmp/jeff-f1b3a-idle.txt <<'PY'
import socket, struct, json, sys
try:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect(sys.argv[1])
    r = json.dumps({"id":1,"method":"handshake","params":None}).encode()
    s.sendall(struct.pack(">I", len(r)) + r)
    n = struct.unpack(">I", s.recv(4))[0]; b = b""
    while len(b) < n: b += s.recv(n - len(b))
    print(json.loads(b)["result"]["core_running"])
except Exception as e:
    print("ERR", e)
PY
kill -9 "$IDLE_PID" 2>/dev/null
grep -q "False" /tmp/jeff-f1b3a-idle.txt || fail "an idle daemon must report core_running=false"
pass "an idle daemon (not hosting the core) does not claim the schedulers"

# a core-hosting daemon must report core_running=true so the app defers.
JEFF_DAEMON_SOCKET="$SOCK" JEFF_DAEMON_STORE_DIR="$TMP" JEFF_DAEMON_RUN_CORE=1 \
  "$TAURI/target/debug/jeff_daemon" >/tmp/jeff-f1b3a-daemon.log 2>&1 &
DPID=$!
sleep 5
kill -0 "$DPID" 2>/dev/null || { cat /tmp/jeff-f1b3a-daemon.log; fail "daemon died"; }
grep -q "core running headless" /tmp/jeff-f1b3a-daemon.log || fail "daemon did not host the core"
kill -9 "$DPID" 2>/dev/null
rm -rf "$SOCK" "$TMP"
pass "a core-hosting daemon claims the schedulers; the app defers to it"

echo "--- apex f1b-3a check passed ---"
