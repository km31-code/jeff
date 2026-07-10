#!/usr/bin/env bash
# apex b7 check: WorkUnderstanding comprehension pass.
# Verifies the content-observation-gated judgment pass, named budget, memory
# episode persistence, snapshot surfacing, and audit visibility. No external
# API calls required for the behavioral tests.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
WORK_RS="$SRC/work_understanding.rs"
AWARENESS_RS="$SRC/awareness_core.rs"
MAIN_RS="$SRC/main.rs"
SELECTION_RS="$SRC/selection_capture.rs"
COST_RS="$SRC/cost_governor.rs"
MEMORY_RS="$SRC/memory.rs"
APP_TSX="$DESKTOP/src/App.tsx"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex b7 WorkUnderstanding check ---"

# 1. Core module and model-call contract.
test -f "$WORK_RS" || fail "work_understanding.rs missing"
grep -q "pub struct WorkUnderstanding" "$WORK_RS" || fail "WorkUnderstanding struct missing"
grep -q "argument_summary" "$WORK_RS" || fail "argument_summary field missing"
grep -q "weak_points" "$WORK_RS" || fail "weak_points field missing"
grep -q "stuck_signal" "$WORK_RS" || fail "stuck_signal field missing"
grep -q "candidate_observation" "$WORK_RS" || fail "candidate_observation field missing"
grep -q "ModelRequest::new" "$WORK_RS" || fail "model request missing"
grep -q "Tier::Judgment" "$WORK_RS" || fail "WorkUnderstanding is not judgment-tier"
grep -q "WORK_UNDERSTANDING_BUDGET_KEY" "$WORK_RS" || fail "named budget key missing from pass"
grep -q "Document text from content observation opt-in" "$WORK_RS" \
  || fail "prompt does not document raw-text opt-in boundary"
grep -q "WORK_UNDERSTANDING_INTERVAL_SECONDS: i64 = 5 \\* 60" "$WORK_RS" \
  || fail "5-minute trigger interval missing"
pass "WorkUnderstanding JSON contract, judgment call, raw-text boundary, and interval present"

# 2. Trigger, privacy, budget, and memory wiring.
grep -q "mod work_understanding;" "$MAIN_RS" || fail "work_understanding module not registered"
grep -q "maybe_spawn_work_understanding" "$MAIN_RS" || fail "AX content poll does not trigger pass"
grep -q "maybe_spawn_work_understanding" "$SELECTION_RS" || fail "browser observation does not trigger pass"
grep -q "AmbientState" "$WORK_RS" || fail "quiet-mode skip missing"
grep -q "get_content_observation_enabled(task_id)" "$WORK_RS" \
  || fail "content-observation opt-in gate missing"
grep -q "KIND_WORK_UNDERSTANDING" "$WORK_RS" || fail "work understanding episode persistence missing"
grep -q "WORK_UNDERSTANDING_BUDGET_KEY" "$COST_RS" || fail "cost governor budget key missing"
grep -q "WORK_UNDERSTANDING_BUDGET_KEY" "$WORK_RS" || fail "pass does not use budget key"
grep -q "work_understanding:latest" "$WORK_RS" || fail "latest snapshot setting missing"
grep -q "pub const KIND_WORK_UNDERSTANDING" "$MEMORY_RS" || fail "memory kind missing"
pass "trigger, quiet/privacy gates, budget key, and memory episode wiring present"

# 3. Snapshot and audit visibility.
grep -q "work_understanding: Option<WorkUnderstanding>" "$AWARENESS_RS" \
  || fail "snapshot does not hold latest WorkUnderstanding"
grep -q "work understanding:" "$AWARENESS_RS" || fail "snapshot summary omits WorkUnderstanding"
grep -q "b7_snapshot_summary_includes_work_understanding_weakest_point" "$AWARENESS_RS" \
  || fail "snapshot summary test missing"
grep -q "memory-episodes-list" "$APP_TSX" || fail "memory episode audit list missing"
grep -q "Comprehension" "$APP_TSX" || fail "Privacy Center comprehension copy missing"
grep -q "b7_seeded_circular_document_produces_weak_point_and_episode" "$WORK_RS" \
  || fail "circular-document weak point test missing"
grep -q "b7_content_observation_off_or_unchanged_blocks_trigger" "$WORK_RS" \
  || fail "content-observation-off/no-change trigger test missing"
pass "snapshot, Privacy Center audit visibility, and B7 behavioral tests present"

CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

B7_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test b7_ --quiet 2>&1)
echo "$B7_TEST_OUT" | grep -q "test result: ok" || { echo "$B7_TEST_OUT"; fail "b7 tests failed"; }
echo "$B7_TEST_OUT" | grep -q "FAILED" && { echo "$B7_TEST_OUT"; fail "b7 tests failed"; }
pass "B7 JSON, circular-doc, trigger-gate, and snapshot-summary tests pass"

A4_OUT=$(cd "$ROOT_DIR" && ./scripts/apex_a4_check.sh 2>&1)
echo "$A4_OUT" | grep -q "apex a4 check passed" || { echo "$A4_OUT"; fail "A4 cost governor gate failed"; }
pass "A4 cost governor gate passes"

B3_OUT=$(cd "$ROOT_DIR" && ./scripts/apex_b3_check.sh 2>&1)
echo "$B3_OUT" | grep -q "apex b3 check passed" || { echo "$B3_OUT"; fail "B3 memory gate failed"; }
pass "B3 memory gate passes"

PHASE31_OUT=$(cd "$ROOT_DIR" && ./scripts/phase31_check.sh 2>&1)
echo "$PHASE31_OUT" | grep -q "phase 31 checks" || { echo "$PHASE31_OUT"; fail "phase31 check did not run"; }
echo "$PHASE31_OUT" | grep -q "FAIL" && { echo "$PHASE31_OUT"; fail "phase31 raw-text audit failed"; }
pass "phase31 raw-text audit passes"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend TypeScript check did not run"; }
pass "frontend TypeScript check passes"

FRONTEND_TEST_OUT=$(cd "$DESKTOP" && npm test -- --run 2>&1)
echo "$FRONTEND_TEST_OUT" | grep -qE "Tests +[0-9]+ passed" || { echo "$FRONTEND_TEST_OUT"; fail "frontend tests did not run"; }
if echo "$FRONTEND_TEST_OUT" | grep -qE "[1-9][0-9]* failed"; then
  echo "$FRONTEND_TEST_OUT"
  fail "frontend tests failed"
fi
pass "frontend tests pass"

echo "--- apex b7 check passed ---"
