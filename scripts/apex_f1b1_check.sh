#!/usr/bin/env bash
# apex f1b-1: decouple the core from the Tauri AppHandle. the core_runtime
# schedulers (f1a) now run against a CoreHost seam -- emit + world-model state
# access + a transitional AppHandle bridge -- so a headless daemon (f1b-3) can
# host them with a non-tauri CoreHost. in-process the seam is TauriHost and
# behavior is unchanged.

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
CORE="$SRC/core_runtime.rs"
MAIN="$SRC/main.rs"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex f1b-1 core/apphandle decoupling check ---"

# 1. the CoreHost seam exists with an in-process (tauri) and testable (fake) impl.
test -f "$CORE" || fail "core_runtime.rs missing"
grep -q "pub trait CoreHost" "$CORE" || fail "CoreHost seam trait missing"
grep -q "pub struct TauriHost" "$CORE" || fail "in-process TauriHost impl missing"
grep -q "impl CoreHost for TauriHost" "$CORE" || fail "TauriHost does not implement CoreHost"
grep -q "struct FakeHost" "$CORE" || fail "non-tauri FakeHost test double missing"
grep -q "impl CoreHost for FakeHost" "$CORE" || fail "FakeHost does not implement CoreHost"
pass "CoreHost seam with TauriHost (in-process) and FakeHost (headless) present"

# 2. the seam exposes emit, quiet gating, state access, and the transitional
# bridge -- and start() takes the seam, not an AppHandle.
grep -q "fn emit(&self, event: &str, payload: serde_json::Value)" "$CORE" || fail "CoreHost::emit missing"
grep -q "fn is_quiet_mode(&self) -> bool" "$CORE" || fail "CoreHost::is_quiet_mode missing"
for accessor in with_jeff_state with_context_state with_calendar_state with_typing_state; do
  grep -q "fn $accessor(&self, f: &mut dyn FnMut" "$CORE" || fail "CoreHost::$accessor state accessor missing"
done
grep -q "fn tauri_app(&self) -> Option<AppHandle>" "$CORE" || fail "transitional tauri_app bridge missing"
grep -qE "pub fn start\(host: Arc<dyn CoreHost>\) -> CoreHandle" "$CORE" || fail "start() does not take the CoreHost seam"
pass "start() takes Arc<dyn CoreHost>; emit/quiet/state-access/bridge on the trait"

# 3. the loops route their OWN i/o through the host, not the raw handle.
# the schedulers no longer capture an AppHandle; state reads and emits go through
# the seam. (helper calls still use host.tauri_app() -- decoupled in f1b-1b.)
grep -q "host.emit(" "$CORE" || fail "loops do not emit through the host"
grep -q "host.with_jeff_state(" "$CORE" || fail "loops do not read state through the host"
grep -q "host.is_quiet_mode()" "$CORE" || fail "loops do not gate through the host"
# every direct tauri managed-state read must live inside the TauriHost impl (one
# per accessor: quiet + 4 with_* = 5), never in a scheduler loop body.
DIRECT_STATE_READS=$(grep -c "try_state::<" "$CORE")
[ "$DIRECT_STATE_READS" -le 5 ] || fail "direct try_state reads leaked into loop bodies ($DIRECT_STATE_READS > 5)"
pass "scheduler loops emit, gate, and read state through the CoreHost seam"

# 4. main.rs constructs the in-process host and starts the core with it.
grep -q "core_runtime::start(Arc::new(core_runtime::TauriHost::new(" "$MAIN" \
  || fail "main.rs does not start the core with a TauriHost"
pass "main.rs wires the in-process TauriHost into the core"

# 5. cargo check is warning-free.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

# 6. the seam is proven headless (no tauri runtime) + full suite green.
for t in \
  f1b1_fake_host_routes_events_without_a_webview \
  f1b1_fake_host_gates_and_has_no_tauri_bridge; do
  grep -q "fn $t" "$CORE" || fail "expected f1b-1 test $t is missing"
done
F1B1_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test --bin jeff-desktop f1b1_ --quiet 2>&1)
echo "$F1B1_TEST_OUT" | grep -q "test result: ok" || { echo "$F1B1_TEST_OUT"; fail "f1b-1 headless-host tests failed"; }
echo "$F1B1_TEST_OUT" | grep -q "FAILED" && { echo "$F1B1_TEST_OUT"; fail "f1b-1 headless-host tests failed"; }
pass "CoreHost seam drives events/gating headless (FakeHost, no tauri runtime)"

TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test --quiet 2>&1)
echo "$TEST_OUT" | grep -q "FAILED" && { echo "$TEST_OUT" | tail -20; fail "backend test suite has failures"; }
echo "$TEST_OUT" | grep -q "test result: ok" || { echo "$TEST_OUT" | tail -20; fail "backend test suite did not report ok"; }
pass "full backend test suite passes"

# 7. frontend is untouched but must still typecheck and test green.
FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend lint did not run"; }
pass "frontend TypeScript check passes"

FRONTEND_TEST_OUT=$(cd "$DESKTOP" && npm test -- --run 2>&1)
echo "$FRONTEND_TEST_OUT" | grep -qE "Test Files.*passed" || { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
echo "$FRONTEND_TEST_OUT" | grep -qE "[0-9]+ failed" && { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
pass "frontend test suite passes"

# 8. adjacent gates: f1a consolidation invariants + the full Apex spine.
if [ "${JEFF_SKIP_ADJACENT_GATES:-0}" != "1" ]; then
  if ! F1A_OUT=$(JEFF_SKIP_ADJACENT_GATES=1 bash "$ROOT_DIR/scripts/apex_f1a_check.sh" 2>&1); then
    echo "$F1A_OUT"
    fail "apex f1a consolidation gate regressed"
  fi
  pass "apex f1a consolidation gate still passes"
  if ! E7_OUT=$(JEFF_SKIP_ADJACENT_GATES=1 bash "$ROOT_DIR/scripts/apex_e7_check.sh" 2>&1); then
    echo "$E7_OUT"
    fail "apex e7 ship gate regressed"
  fi
  pass "apex e7 ship gate still passes"
fi

echo "--- apex f1b-1 check passed ---"
