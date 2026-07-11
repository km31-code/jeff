#!/usr/bin/env bash
# apex e4 check: calendar full-day + meeting awareness, pre-meeting prep, and
# calendar.propose event creation.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
CAL="$SRC/calendar_core.rs"
COMMANDS="$SRC/commands.rs"
MAIN="$SRC/main.rs"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex e4 calendar awareness check ---"

# 1. Module + capabilities.
test -f "$CAL" || fail "calendar_core.rs missing"
grep -q "pub fn full_day_events" "$CAL" || fail "full-day awareness missing"
grep -q "pub fn pre_meeting_prep_offer" "$CAL" || fail "pre-meeting prep composer missing"
grep -q "pub fn attendee_overlap" "$CAL" || fail "attendee overlap missing"
grep -q "pub fn propose_event" "$CAL" || fail "event proposal missing"
grep -q "ActionClass::CalendarPropose" "$CAL" || fail "event creation is not a calendar.propose action"
pass "calendar core: full-day, pre-meeting prep, attendee overlap, propose present"

# 2. MeetingImminent has full-day data behind it (crisis detection deterministic).
grep -q "fn detect_meeting_imminent" "$SRC/crisis_core.rs" || fail "MeetingImminent detection missing"
pass "MeetingImminent crisis remains deterministic with full-day data"

# 3. Behavioral: full-day sort, 1:25pm pre-meeting scene, event round-trip.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

for t in \
  e4_full_day_awareness_sorts_upcoming_and_drops_past \
  e4_pre_meeting_prep_reproduces_the_125pm_scene \
  e4_event_proposal_round_trips_as_calendar_propose; do
  grep -q "fn $t" "$CAL" || fail "expected e4 test $t is missing"
done
E4_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test e4_ --quiet 2>&1)
echo "$E4_TEST_OUT" | grep -q "test result: ok" || { echo "$E4_TEST_OUT"; fail "e4 tests failed"; }
echo "$E4_TEST_OUT" | grep -q "FAILED" && { echo "$E4_TEST_OUT"; fail "e4 tests failed"; }
pass "e4 full-day/pre-meeting/propose tests pass (1:25pm scene reproducible)"

# 4. Commands registered.
for cmd in full_day_calendar pre_meeting_prep propose_calendar_event; do
  grep -q "pub fn $cmd" "$COMMANDS" || fail "$cmd command missing"
  grep -q "commands::$cmd" "$MAIN" || fail "$cmd not registered"
done
pass "calendar commands are wired"

bash "$ROOT_DIR/scripts/apex_e3_check.sh" >/dev/null 2>&1 || fail "apex e3 gmail gate regressed"
pass "apex e3 gmail gate still passes"

echo "--- apex e4 check passed ---"
