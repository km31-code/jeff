#!/usr/bin/env bash
# apex f2a: store-backed memory jobs run headless on the daemon.
#
# consolidation and the idle session-summary are store-backed (no perception), so
# they belong with the background schedulers -- exactly one process owns them and
# the headless daemon runs them overnight. before f2a they were app-only side
# tasks gated to the perception profiles and never ran while the app was closed.
# this check proves the move: the schedulers live in core_runtime under the
# background-scheduler gate, the app-only polls are retired, and consolidation's
# due-gate is a host-agnostic function the daemon can call.

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
TAURI="$ROOT_DIR/desktop/src-tauri"
CORE="$SRC/core_runtime.rs"
CONS="$SRC/consolidation.rs"
MEM="$SRC/memory.rs"
APP_POLLS="$SRC/app_polls.rs"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex f2a memory jobs on the daemon check ---"

# 1. the schedulers exist in core_runtime and are wired under the background gate.
grep -q "fn spawn_memory_consolidation" "$CORE" || fail "consolidation scheduler missing from core_runtime"
grep -q "fn spawn_memory_session_summary" "$CORE" || fail "session-summary scheduler missing from core_runtime"
# both must be pushed inside the runs_background_schedulers() block, next to the
# standing-job/speculation schedulers -- not the perception block.
BG_BLOCK="$(awk '/if profile.runs_background_schedulers\(\)/{f=1} f{print} f&&/^    }/{exit}' "$CORE")"
printf '%s\n' "$BG_BLOCK" | grep -q "spawn_memory_consolidation(" || fail "consolidation not wired under runs_background_schedulers()"
printf '%s\n' "$BG_BLOCK" | grep -q "spawn_memory_session_summary(" || fail "session summary not wired under runs_background_schedulers()"
pass "memory consolidation + session-summary schedulers run under the background-scheduler profiles"

# 2. the app-only polls are retired (no dead perception-gated copies left).
grep -q "spawn_memory_consolidation_poll" "$APP_POLLS" && fail "retired app-only consolidation poll still present"
grep -q "spawn_memory_session_summary_poll" "$APP_POLLS" && fail "retired app-only session-summary poll still present"
# and the perception-gated side tasks must no longer spawn them.
SIDE="$(awk '/fn spawn_side_tasks/{f=1} f{print} f&&/^    }/{exit}' "$CORE")"
printf '%s\n' "$SIDE" | grep -q "memory_consolidation" && fail "consolidation still spawned from perception side tasks"
printf '%s\n' "$SIDE" | grep -q "memory_session_summary" && fail "session summary still spawned from perception side tasks"
pass "the app-only memory polls are retired and no longer on the perception side tasks"

# 3. consolidation's due-gate is host-agnostic (callable by the daemon), with its
#    privacy gate, pending-work gate, and idle/2am timing all inside it.
grep -q "pub fn consolidate_if_due" "$CONS" || fail "host-agnostic consolidate_if_due missing"
grep -q "get_privacy_user_profile_memory_enabled" "$CONS" || fail "consolidation privacy gate not in the host-agnostic path"
grep -q "unconsolidated_episode_count" "$CONS" || fail "consolidation pending-work gate missing"
grep -q "CONSOLIDATION_IDLE_SECONDS: i64 = 10 \\* 60" "$CONS" || fail "10-minute idle trigger missing"
grep -q "CONSOLIDATION_LAST_2AM_KEY" "$CONS" || fail "daily 2am catch-up guard missing"
grep -q "pub const SESSION_IDLE_SECONDS" "$MEM" || fail "session-summary idle constant not relocated to memory.rs"
pass "consolidate_if_due carries the full gate and the daemon can call it"

# 4. warning-free compile + the f2a unit tests.
CHECK_OUT=$(cd "$TAURI" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

for t in \
  f2a_memory_jobs_are_owned_by_the_background_scheduler_profiles \
  f2a_consolidate_if_due_is_gated_by_privacy_and_pending_work \
  f2a_consolidate_if_due_runs_during_a_lull; do
  grep -qrn "fn $t" "$SRC" || fail "expected f2a test $t is missing"
done
T_OUT=$(cd "$TAURI" && cargo test --lib f2a_ --quiet 2>&1)
echo "$T_OUT" | grep -q "test result: ok" || { echo "$T_OUT"; fail "f2a tests failed"; }
echo "$T_OUT" | grep -q "FAILED" && { echo "$T_OUT"; fail "f2a tests failed"; }
pass "f2a unit tests pass (profile ownership; privacy/pending gate; runs during a lull, idempotent)"

# 5. the reconciled memory milestones still pass at their new locations. these run
#    targeted test subsets (b3_/b4_), so they are deterministic. apex_f1a is not
#    re-run here: it runs the full backend suite, which is subject to a
#    pre-existing SQLite WAL shared-memory flake under heavy parallelism; its f2a
#    reconciliation (the core no longer invokes the retired polls) is proven above.
JEFF_SKIP_ADJACENT_GATES=1 bash "$ROOT_DIR/scripts/apex_b3_check.sh" >/dev/null 2>&1 || fail "apex_b3 (session summary) regressed"
JEFF_SKIP_ADJACENT_GATES=1 bash "$ROOT_DIR/scripts/apex_b4_check.sh" >/dev/null 2>&1 || fail "apex_b4 (consolidation) regressed"
# f1a symbol-level reconciliation: the retired polls are gone from the core's
# app_polls invocation list (deterministic, no full-suite run).
grep -q "crate::app_polls::spawn_memory_consolidation_poll" "$CORE" && fail "core still invokes the retired consolidation poll"
grep -q "crate::app_polls::spawn_memory_session_summary_poll" "$CORE" && fail "core still invokes the retired session-summary poll"
pass "reconciled apex_b3 and apex_b4 still pass; f1a poll-invocation reconciliation holds"

echo "--- apex f2a check passed ---"
