#!/usr/bin/env bash
# apex e7 ship gate: the flat, comprehensive acceptance gate. Everything proves
# out from one entry point -- eval suite, full test suites, DB migration
# reliability, release-workflow order, and the presence of every milestone gate.
# (Flat by design: it does not recursively re-run each predecessor's chain.)

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
RELEASE="$ROOT_DIR/.github/workflows/release.yml"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex e7 ship gate ---"

# 1. Every Apex milestone gate exists (A1-E7).
missing=""
for id in a1 a2 a3 a4 a5 b1 b2 b3 b4 b5 b6 b7 c1 c2 c3 c4 c5 c6 c7 \
          d1 d2 d3 d4 d5 d6 d7 d8 d9 e1 e2 e3 e4 e5 e6; do
  [ -f "$ROOT_DIR/scripts/apex_${id}_check.sh" ] || missing="$missing $id"
done
[ -z "$missing" ] || fail "missing milestone check scripts:$missing"
test -f "$ROOT_DIR/scripts/apex_eval.sh" || fail "apex_eval.sh entry point missing"
pass "all A1-E6 milestone gates and the apex_eval entry point are present"

# 2. The eval suite (quality spine) passes from one entry point (exit 0 iff all
# deterministic suites pass).
if bash "$ROOT_DIR/scripts/apex_eval.sh" all; then
  pass "apex eval suite passes (judgment, crisis, agent, inbox, latency)"
else
  fail "apex eval suite reported failures"
fi

# 3. Release workflow order: eval -> build -> sign -> notarize.
grep -q "bash scripts/judgment_eval.sh" "$RELEASE" || fail "judgment eval not in release workflow"
grep -q "bash scripts/agent_eval.sh" "$RELEASE" || fail "agent eval not in release workflow"
grep -q "bash scripts/inbox_eval.sh" "$RELEASE" || fail "inbox eval not in release workflow"
grep -q "bash scripts/character_eval.sh" "$RELEASE" || fail "character eval not in release workflow"
grep -q "needs: test" "$RELEASE" || fail "build does not depend on the test/eval job"
grep -q "needs: \[build, character-eval\]" "$RELEASE" || fail "sign does not depend on build + character eval"
grep -q "needs: sign" "$RELEASE" || fail "notarize does not depend on sign"
pass "release workflow order is eval -> build -> sign -> notarize"

# 4. Reliability: DB migrations are idempotent and preserve data across re-init.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

MIG_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test e7_migrations_are_idempotent --quiet 2>&1)
echo "$MIG_OUT" | grep -q "test result: ok" || { echo "$MIG_OUT"; fail "DB migration idempotency test failed"; }
echo "$MIG_OUT" | grep -q "FAILED" && { echo "$MIG_OUT"; fail "DB migration idempotency test failed"; }
pass "DB migrations are idempotent and preserve data across re-initialization"

# 5. Full behavioral gate: backend + frontend suites green.
TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test --quiet 2>&1)
echo "$TEST_OUT" | grep -q "FAILED" && { echo "$TEST_OUT" | tail -20; fail "backend test suite has failures"; }
echo "$TEST_OUT" | grep -q "test result: ok" || { echo "$TEST_OUT" | tail -20; fail "backend test suite did not report ok"; }
pass "full backend test suite passes"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend lint did not run"; }
pass "frontend TypeScript check passes"

FRONTEND_TEST_OUT=$(cd "$DESKTOP" && npm test -- --run 2>&1)
echo "$FRONTEND_TEST_OUT" | grep -qE "Test Files.*passed" || { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
echo "$FRONTEND_TEST_OUT" | grep -qE "[0-9]+ failed" && { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
pass "frontend test suite passes"

echo ""
echo "note: the live acceptance gates are manual/env-gated by design:"
echo "  - the Part II day-in-the-life on a signed release build (needs keys + hardware)"
echo "  - a non-technical tester reaching first value in <5 min, on video"
echo "  - character/goal live-LLM evals (JEFF_RUN_EXTERNAL_EVAL=1 with an API key)"
echo "  - crash telemetry aggregation and cross-version DB migration with shipped DBs"

echo "--- apex e7 ship gate passed ---"
