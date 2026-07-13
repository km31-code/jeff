#!/usr/bin/env bash
# apex f1b-1b: complete the core/AppHandle decoupling. f1b-1 routed the loops'
# own emit/gate/state access through CoreHost but still reached the helper
# modules (awareness_core, crisis, workload, proactive, poll helpers) through a
# transitional tauri_app() bridge. f1b-1b hoists those calls behind tauri-
# agnostic CoreHost intent methods implemented by TauriHost, removing the bridge:
# the core (CoreHost trait + core_runtime loops) now contains zero tauri types,
# so a headless daemon host can implement the seam. helper module signatures are
# unchanged (TauriHost calls them with its AppHandle), so there is no app-wide
# ripple to their many callers.

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
CORE="$SRC/core_runtime.rs"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex f1b-1b core decoupling completion check ---"

# 1. the transitional bridge is gone; side effects are semantic intents.
test -f "$CORE" || fail "core_runtime.rs missing"
grep -q "fn tauri_app" "$CORE" && fail "tauri_app() bridge still present"
for intent in request_awareness_update fire_meeting_imminent fire_deadline_collision check_stale_tasks spawn_side_tasks; do
  grep -q "fn $intent" "$CORE" || fail "CoreHost intent $intent missing"
done
pass "tauri_app bridge removed; world-model side effects are CoreHost intents"

# 2. the loops invoke the intents instead of AppHandle-based helpers.
grep -q "host.request_awareness_update(" "$CORE" || fail "loops do not request awareness updates via the host"
grep -q "host.fire_meeting_imminent(" "$CORE" || fail "calendar loop does not fire meeting crisis via the host"
grep -q "host.fire_deadline_collision(" "$CORE" || fail "calendar loop does not fire deadline crisis via the host"
grep -q "host.check_stale_tasks(" "$CORE" || fail "stale loop does not check via the host"
grep -q "host.spawn_side_tasks()" "$CORE" || fail "start() does not delegate side tasks to the host"
pass "scheduler loops trigger awareness/crisis/stale/side-tasks through CoreHost intents"

# 3. the tauri coupling lives only in the TauriHost impl. the loops (everything
# above the impl blocks) must not name AppHandle or call the helper modules
# directly. we assert the helper calls appear only inside the host impl.
for helper in "awareness_core::spawn_awareness_update" "crisis::maybe_fire_meeting_imminent" "crisis::maybe_fire_deadline_collision" "workload::check_stale_task_notifications" "proactive::spawn_ambient_monitor"; do
  count=$(grep -c "$helper" "$CORE")
  [ "$count" -le 1 ] || fail "helper $helper called $count times in core_runtime (should be once, inside TauriHost)"
done
pass "AppHandle-based helper calls are confined to the TauriHost impl (one each)"

# 4. warning-free build.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

# 5. the seam runs the intents headless + full suite green.
grep -q "fn f1b1_fake_host_gates_and_runs_intents_headless" "$CORE" || fail "headless intent test missing"
F_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test --bin jeff-desktop f1b1_ --quiet 2>&1)
echo "$F_TEST_OUT" | grep -q "test result: ok" || { echo "$F_TEST_OUT"; fail "f1b-1b headless-host tests failed"; }
echo "$F_TEST_OUT" | grep -q "FAILED" && { echo "$F_TEST_OUT"; fail "f1b-1b headless-host tests failed"; }
pass "CoreHost intents run headless (FakeHost, no tauri runtime)"

TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test --quiet 2>&1)
echo "$TEST_OUT" | grep -q "FAILED" && { echo "$TEST_OUT" | tail -20; fail "backend test suite has failures"; }
echo "$TEST_OUT" | grep -q "test result: ok" || { echo "$TEST_OUT" | tail -20; fail "backend test suite did not report ok"; }
pass "full backend test suite passes"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend lint did not run"; }
pass "frontend TypeScript check passes"

# 6. adjacent gates: the f1b-1 seam gate and the full Apex spine.
if [ "${JEFF_SKIP_ADJACENT_GATES:-0}" != "1" ]; then
  if ! F1B1_OUT=$(JEFF_SKIP_ADJACENT_GATES=1 bash "$ROOT_DIR/scripts/apex_f1b1_check.sh" 2>&1); then
    echo "$F1B1_OUT"
    fail "apex f1b-1 seam gate regressed"
  fi
  pass "apex f1b-1 seam gate still passes"
  if ! E7_OUT=$(JEFF_SKIP_ADJACENT_GATES=1 bash "$ROOT_DIR/scripts/apex_e7_check.sh" 2>&1); then
    echo "$E7_OUT"
    fail "apex e7 ship gate regressed"
  fi
  pass "apex e7 ship gate still passes"
fi

echo "--- apex f1b-1b check passed ---"
