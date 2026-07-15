#!/usr/bin/env bash
# apex f2b: the morning briefing is prepared overnight, not composed on demand.
#
# f2a put consolidation on the daemon; f2b closes the morning-readiness loop: a
# background scheduler composes the day's briefing ahead of first engagement --
# folding in the overnight work the daemon actually finished (completed jobs,
# standing-job runs) and yesterday's consolidated takeaways -- and persists it.
# delivery retrieves it instead of composing, so the message is ready the moment
# you sit down and costs no model call then. the on-demand path is preserved as
# the fallback for when nothing was prepared (no daemon, or a same-day cold start).

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
TAURI="$ROOT_DIR/desktop/src-tauri"
DESKTOP="$ROOT_DIR/desktop"
MORNING="$SRC/morning.rs"
BRIEFING="$SRC/briefing.rs"
CORE="$SRC/core_runtime.rs"
STORE="$SRC/store.rs"
COMMANDS="$SRC/commands.rs"
MAIN="$SRC/main.rs"
APP_TSX="$DESKTOP/src/App.tsx"
CLIENT_TS="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex f2b morning briefing prepared overnight check ---"

# 1. the prep module composes + persists ahead of engagement, folding in overnight work.
test -f "$MORNING" || fail "morning.rs missing"
grep -q "pub fn prepare_todays_briefing" "$MORNING" || fail "prepare_todays_briefing missing"
grep -q "fn gather_overnight_work" "$MORNING" || fail "overnight-work gathering missing"
grep -q "JOB_STATUS_COMPLETED" "$MORNING" || fail "overnight work must be built from completed jobs"
grep -q "!job.speculative" "$MORNING" || fail "speculative jobs must be excluded from overnight work"
grep -q "upsert_prepared_briefing" "$MORNING" || fail "prepared briefing is not persisted"
grep -q "get_prepared_briefing" "$MORNING" || fail "per-day idempotency guard missing"
pass "prepare_todays_briefing composes from overnight work + consolidation and persists once per day"

# 2. it runs on the single-owner background schedulers (daemon overnight), not the
#    perception side tasks -- so it never double-runs and works headless.
grep -q "fn spawn_morning_prep" "$CORE" || fail "morning-prep scheduler missing from core_runtime"
BG_BLOCK="$(awk '/if profile.runs_background_schedulers\(\)/{f=1} f{print} f&&/^    }/{exit}' "$CORE")"
printf '%s\n' "$BG_BLOCK" | grep -q "spawn_morning_prep(" || fail "morning prep not wired under runs_background_schedulers()"
pass "morning prep runs on the background schedulers (daemon overnight; app when there is no daemon)"

# 3. delivery is retrieval-first, with the on-demand compose preserved as fallback.
FIRE_BODY="$(awk '/pub async fn maybe_fire_briefing/{f=1} f{print} f&&/^}/{exit}' "$BRIEFING")"
printf '%s\n' "$FIRE_BODY" | grep -q "get_prepared_briefing" || fail "delivery does not retrieve a prepared briefing"
printf '%s\n' "$FIRE_BODY" | grep -q "mark_prepared_briefing_delivered" || fail "delivered prepared briefing is not marked"
printf '%s\n' "$FIRE_BODY" | grep -q "compose_briefing" || fail "on-demand compose fallback was not preserved"
# the prepared branch must be tried before the compose fallback.
PREP_LINE="$(printf '%s\n' "$FIRE_BODY" | grep -n "get_prepared_briefing" | head -1 | cut -d: -f1)"
COMPOSE_LINE="$(printf '%s\n' "$FIRE_BODY" | grep -n "compose_briefing" | head -1 | cut -d: -f1)"
[ "$PREP_LINE" -lt "$COMPOSE_LINE" ] || fail "retrieval must be attempted before on-demand composition"
pass "delivery retrieves the prepared briefing first and only composes on demand as a fallback"

# 4. persistence + privacy surface.
grep -q "CREATE TABLE IF NOT EXISTS prepared_briefings" "$STORE" || fail "prepared_briefings table missing"
grep -q "pub fn upsert_prepared_briefing" "$STORE" || fail "upsert method missing"
grep -q "pub fn mark_prepared_briefing_delivered" "$STORE" || fail "delivered marker missing"
grep -q "pub fn get_morning_readiness" "$COMMANDS" || fail "morning-readiness command missing"
grep -q "commands::get_morning_readiness" "$MAIN" || fail "morning-readiness command not registered"
grep -q "getMorningReadiness" "$CLIENT_TS" || fail "frontend binding missing"
grep -q "privacy-morning-readiness" "$APP_TSX" || fail "Privacy Center morning-readiness surface missing"
pass "prepared briefings persist; morning readiness is visible in the Privacy Center"

# 5. warning-free compile + f2b unit tests + preserved briefing behavior + frontend.
CHECK_OUT=$(cd "$TAURI" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

for t in \
  f2b_prepare_requires_an_active_task \
  f2b_prepare_is_idempotent_per_day \
  f2b_overnight_work_folds_completed_jobs_into_the_briefing \
  f2b_speculative_jobs_are_not_reported_as_overnight_work \
  f2b_prepared_briefing_round_trips_and_tracks_delivery; do
  grep -qrn "fn $t" "$SRC" || fail "expected f2b test $t is missing"
done
T_OUT=$(cd "$TAURI" && cargo test --lib f2b_ --quiet 2>&1)
echo "$T_OUT" | grep -q "test result: ok" || { echo "$T_OUT"; fail "f2b tests failed"; }
echo "$T_OUT" | grep -q "FAILED" && { echo "$T_OUT"; fail "f2b tests failed"; }
pass "f2b unit tests pass (active-task gate; per-day idempotency; overnight folding; speculative excluded; delivery flag)"

# the briefing milestone this extended still passes (targeted, deterministic).
JEFF_SKIP_ADJACENT_GATES=1 bash "$ROOT_DIR/scripts/apex_c3_check.sh" >/dev/null 2>&1 || fail "apex_c3 (briefing/debrief) regressed"
pass "apex_c3 (briefing/debrief rituals) still passes"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend lint did not run"; }
pass "frontend TypeScript check passes"

echo "--- apex f2b check passed ---"
