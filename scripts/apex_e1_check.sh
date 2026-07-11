#!/usr/bin/env bash
# apex e1 check: tool bus (MCP client) -- governed connections, typed data
# boundary, per-tool call log, scoping, disconnect-purge.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
BUS="$SRC/tool_bus.rs"
STORE="$SRC/store.rs"
COMMANDS="$SRC/commands.rs"
MAIN="$SRC/main.rs"
APP_TSX="$DESKTOP/src/App.tsx"
TAURI_CLIENT="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex e1 tool bus check ---"

# 1. Module, tables, connection manager.
test -f "$BUS" || fail "tool_bus.rs missing"
grep -q "CREATE TABLE IF NOT EXISTS tool_connections" "$STORE" || fail "tool_connections table missing"
grep -q "CREATE TABLE IF NOT EXISTS tool_connection_tools" "$STORE" || fail "tool discovery table missing"
grep -q "CREATE TABLE IF NOT EXISTS tool_call_log" "$STORE" || fail "tool_call_log table missing"
grep -q "pub fn add_tool_connection" "$BUS" || fail "add connection missing"
grep -q "pub fn remove_tool_connection" "$BUS" || fail "disconnect/remove missing"
grep -q "pub fn set_tool_connection_enabled" "$BUS" || fail "enable/disable missing"
grep -q "pub fn register_connection_tools" "$BUS" || fail "tool discovery missing"
grep -q "TRANSPORT_STDIO" "$BUS" || fail "stdio transport missing"
grep -q "TRANSPORT_HTTP" "$BUS" || fail "http transport missing"
pass "tool bus module, tables, transports, and connection manager present"

# 2. Typed data boundary: ambient context can never enter a tool call.
grep -q "pub struct ToolArguments" "$BUS" || fail "ToolArguments boundary type missing"
grep -q "AMBIENT_CONTEXT_KEYS" "$BUS" || fail "ambient-context denylist missing"
grep -q "MAX_TOOL_ARGUMENTS_BYTES" "$BUS" || fail "payload size cap missing"
# there must be no From<Snapshot>/From<Memory> path into ToolArguments.
if grep -qE "impl From<.*(Snapshot|Memory|Relational|Profile).*> for ToolArguments" "$BUS"; then
  fail "ambient structs must not convert into ToolArguments"
fi
pass "typed data boundary (ToolArguments + denylist + size cap) is enforced"

# 3. Invocation: scoping, logging, disconnect-stops-calls.
grep -q "pub fn invoke_tool" "$BUS" || fail "invoke_tool missing"
grep -q "fn log_call" "$BUS" || fail "per-tool call logging missing"
grep -q "is not in the scope of connection" "$BUS" || fail "per-connection scoping missing"
grep -q "is disconnected" "$BUS" || fail "disconnect does not stop calls"
pass "invocation is scoped, logged, and stopped on disconnect"

# 4. Commands + Privacy Center surface.
for cmd in list_tool_connections add_tool_connection set_tool_connection_enabled remove_tool_connection list_connection_tools list_tool_call_log invoke_tool; do
  grep -q "pub fn $cmd" "$COMMANDS" || fail "$cmd command missing"
  grep -q "commands::$cmd" "$MAIN" || fail "$cmd not registered"
done
grep -q "listToolConnections" "$TAURI_CLIENT" || fail "frontend listToolConnections binding missing"
grep -q "removeToolConnection" "$TAURI_CLIENT" || fail "frontend disconnect binding missing"
grep -q "privacy-surface-tool-bus" "$APP_TSX" || fail "Privacy Center connections surface missing"
grep -q "tool-connection-toggle" "$APP_TSX" || fail "connection disconnect control missing"
grep -q "tool-call-log" "$APP_TSX" || fail "per-tool call log surface missing"
pass "commands and Privacy Center connections/call-log surface are wired"

# 5. Behavioral: boundary, logging, disconnect, scoping tests.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

for t in \
  e1_data_boundary_rejects_ambient_context \
  e1_invocation_is_logged_with_summary_and_timestamp \
  e1_disconnect_stops_calls_and_purges_tools \
  e1_out_of_scope_tool_is_rejected; do
  grep -q "fn $t" "$BUS" || fail "expected e1 test $t is missing"
done
E1_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test e1_ --quiet 2>&1)
echo "$E1_TEST_OUT" | grep -q "test result: ok" || { echo "$E1_TEST_OUT"; fail "e1 tests failed"; }
echo "$E1_TEST_OUT" | grep -q "FAILED" && { echo "$E1_TEST_OUT"; fail "e1 tests failed"; }
E1_PASSED=$(echo "$E1_TEST_OUT" | grep -oE "[0-9]+ passed" | awk '{s+=$1} END{print s+0}')
[ "$E1_PASSED" -ge 4 ] || { echo "$E1_TEST_OUT"; fail "expected >=4 e1 tests, saw $E1_PASSED"; }
pass "e1 boundary/logging/disconnect/scoping tests pass ($E1_PASSED passed)"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend lint did not run"; }
pass "frontend TypeScript check passes"

FRONTEND_TEST_OUT=$(cd "$DESKTOP" && npm test -- --run 2>&1)
echo "$FRONTEND_TEST_OUT" | grep -qE "Test Files.*passed" || { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
echo "$FRONTEND_TEST_OUT" | grep -qE "[0-9]+ failed" && { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
pass "frontend tests pass"

bash "$ROOT_DIR/scripts/apex_d9_check.sh" >/dev/null 2>&1 || fail "apex d9 gate regressed"
pass "apex d9 self-extension gate still passes"

echo "--- apex e1 check passed ---"
