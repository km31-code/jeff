#!/usr/bin/env bash
# apex c7 check: override channel.
# Verifies deterministic crisis classification, bypass delivery, quiet-mode
# downgrade, per-class controls, feedback logging, watcher/calendar wiring, and
# crisis eval fixtures. No external API calls required.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
CORE="$SRC/crisis_core.rs"
CRISIS="$SRC/crisis.rs"
MAIN="$SRC/main.rs"
# apex f1a moved the calendar poll (which wires the meeting/deadline crisis
# classes) out of main.rs into core_runtime.
CORE_RUNTIME="$SRC/core_runtime.rs"
WATCHER="$SRC/watcher.rs"
COMMANDS="$SRC/commands.rs"
MODELS="$SRC/models.rs"
APP_TSX="$DESKTOP/src/App.tsx"
OVERLAY="$DESKTOP/src/Overlay.tsx"
TAURI_CLIENT="$DESKTOP/src/tauriClient.ts"
STYLES="$DESKTOP/src/styles.css"
EVAL_JSON="$ROOT_DIR/eval/crisis_eval.json"
HARNESS="$ROOT_DIR/scripts/crisis_eval.sh"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex c7 override channel check ---"

# 1. deterministic crisis classes and classifiers.
test -f "$CRISIS" || fail "crisis.rs missing"
test -f "$CORE" || fail "crisis_core.rs missing"
grep -q "pub enum CrisisClass" "$CORE" || fail "CrisisClass enum missing"
for class in DeadlineCollision MeetingImminent DataLossRisk AwaitedReplyLanded StandingJobCritical; do
  grep -q "$class" "$CORE" || fail "CrisisClass missing $class"
done
grep -q "detect_meeting_imminent" "$CORE" || fail "meeting detector missing"
grep -q "detect_deadline_collision" "$CORE" || fail "deadline detector missing"
grep -q "detect_data_loss_risk" "$CORE" || fail "data-loss detector missing"
if grep -Eq "ModelRouter|Tier::|generate_async|STAGE2|interruption_ledger|cooldown|focus_score" "$CORE"; then
  fail "crisis classification depends on model, stage2, ledger, cooldown, or focus"
fi
pass "crisis classification is deterministic and independent of stage2/ledger/focus gates"

# 2. delivery, quiet downgrade, logging, and feedback.
grep -q "CRISIS_FIRED_EVENT" "$CRISIS" || fail "crisis fired event missing"
grep -q "crisis://fired" "$CRISIS" "$OVERLAY" || fail "crisis event not wired to overlay"
grep -q "CRISIS_LOG_EVENT_TYPE" "$CRISIS" || fail "crisis firing log event missing"
grep -q "CRISIS_FEEDBACK_EVENT_TYPE" "$CRISIS" || fail "crisis feedback log event missing"
grep -q "record_feedback" "$CRISIS" || fail "feedback logger missing"
grep -q "persistent_card" "$CRISIS" || fail "quiet downgrade persistent card missing"
grep -q "dispatch_notification" "$CRISIS" || fail "non-quiet crisis notification missing"
grep -q "voice_if_session_open" "$CRISIS" "$MODELS" || fail "voice-if-session-open marker missing"
pass "delivery logs firing, emits persistent card, notifies when not quiet, and records feedback"

# 3. main monitor and watcher signal integration.
grep -q "maybe_fire_meeting_imminent" "$CORE_RUNTIME" || fail "meeting crisis not wired into calendar poll"
grep -q "maybe_fire_deadline_collision" "$CORE_RUNTIME" || fail "deadline crisis not wired into calendar poll"
grep -q "crisis_event_matches_context" "$MAIN" || fail "movement-toward-event check missing"
grep -q "set_mass_deletion_notify" "$WATCHER" || fail "watcher mass-deletion callback missing"
grep -q "is_mass_deletion_signal" "$WATCHER" || fail "watcher does not test mass-deletion signal"
grep -q "fire_data_loss_risk" "$MAIN" || fail "data-loss crisis not wired from watcher"
pass "calendar and watcher paths feed the override channel"

# 4. Privacy Center controls and overlay card.
grep -q "CrisisClassControlDto" "$MODELS" || fail "crisis controls DTO missing"
grep -q "CrisisCardDto" "$MODELS" || fail "crisis card DTO missing"
grep -q "crisis_controls" "$MODELS" "$COMMANDS" "$TAURI_CLIENT" || fail "dashboard crisis controls missing"
grep -q "set_crisis_class_enabled" "$COMMANDS" "$MAIN" || fail "set_crisis_class_enabled not registered"
grep -q "record_crisis_feedback" "$COMMANDS" "$MAIN" || fail "record_crisis_feedback not registered"
grep -q "privacy-surface-crisis" "$APP_TSX" || fail "Privacy Center crisis controls missing"
grep -q "privacy-toggle-crisis" "$APP_TSX" || fail "per-class crisis toggles missing"
grep -q "overlay-crisis-card" "$OVERLAY" "$STYLES" || fail "persistent crisis card missing"
grep -q "crisis-not-urgent" "$OVERLAY" || fail "not-urgent feedback action missing"
pass "per-class controls, persistent card, and feedback UI are wired"

# 5. crisis eval fixtures include seeded done-when cases.
test -f "$EVAL_JSON" || fail "eval/crisis_eval.json missing"
python3 - "$EVAL_JSON" <<'PY' || fail "crisis eval schema/coverage failed"
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as handle:
    cases = json.load(handle)
if len(cases) < 17:
    raise SystemExit("expected at least 17 crisis cases")
ids = {case["id"] for case in cases}
for required in ["x001_meeting_10m_fire", "x002_meeting_40m_no_fire", "x008_mass_deletion_fire"]:
    if required not in ids:
        raise SystemExit(f"missing {required}")
classes = {case["class"] for case in cases}
for required in ["meeting_imminent", "deadline_collision", "data_loss_risk", "awaited_reply_landed", "standing_job_critical"]:
    if required not in classes:
        raise SystemExit(f"missing class {required}")
if not any(case["expected_fire"] for case in cases):
    raise SystemExit("missing fire cases")
if not any(not case["expected_fire"] for case in cases):
    raise SystemExit("missing no-fire cases")
print(f"{len(cases)} crisis cases; classes={sorted(classes)}")
PY
pass "crisis eval corpus covers seeded fire/no-fire cases"

test -x "$HARNESS" || fail "scripts/crisis_eval.sh missing or not executable"
CRISIS_OUT=$("$HARNESS" 2>&1)
echo "$CRISIS_OUT" | grep -q "17/17 passed" || { echo "$CRISIS_OUT"; fail "crisis eval failed"; }
pass "crisis eval passes"

# 6. compile, tests, frontend, and adjacent gates.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

C7_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test c7_ --quiet 2>&1)
echo "$C7_TEST_OUT" | grep -q "test result: ok" || { echo "$C7_TEST_OUT"; fail "c7 tests failed"; }
echo "$C7_TEST_OUT" | grep -q "warning:" && { echo "$C7_TEST_OUT"; fail "c7 tests emitted warnings"; }
echo "$C7_TEST_OUT" | grep -q "FAILED" && { echo "$C7_TEST_OUT"; fail "c7 tests failed"; }
pass "c7 detector, toggle, quiet downgrade, and feedback tests pass without warnings"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend TypeScript check did not run"; }
pass "frontend TypeScript check passes"

bash "$ROOT_DIR/scripts/apex_c1_check.sh" >/dev/null 2>&1 || fail "apex c1 two-stage gate regressed"
pass "apex c1 two-stage gate still passes"

bash "$ROOT_DIR/scripts/apex_c6_check.sh" >/dev/null 2>&1 || fail "apex c6 judgment eval gate regressed"
pass "apex c6 judgment eval gate still passes"

echo "--- apex c7 check passed ---"
