#!/usr/bin/env bash
# apex d7 check: agent eval suite over delegated-job contracts.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
CORE="$SRC/agent_eval_core.rs"
BIN="$SRC/bin/agent_eval.rs"
CONTRACTS="$ROOT_DIR/eval/agent_eval/contracts.json"
RUNNER="$ROOT_DIR/scripts/agent_eval.sh"
RELEASE="$ROOT_DIR/.github/workflows/release.yml"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex d7 agent eval suite check ---"

# 1. Suite files exist and are wired.
test -f "$CORE" || fail "agent_eval_core.rs missing"
test -f "$BIN" || fail "agent_eval bin missing"
test -f "$CONTRACTS" || fail "agent_eval contracts.json missing"
test -f "$RUNNER" || fail "agent_eval.sh runner missing"
grep -q "pub mod agent_eval_core" "$SRC/lib.rs" || fail "agent_eval_core not exported from lib"
grep -q "pub fn evaluate_agent_contract" "$CORE" || fail "evaluate_agent_contract missing"
grep -q "bash scripts/agent_eval.sh" "$RELEASE" || fail "agent eval not wired into release workflow"
pass "agent eval core, runner, contracts, and CI wiring are present"

# 2. Contract set: 15 non-gated + 5 e2-gated, with the required categories.
COUNTS=$(python3 - "$CONTRACTS" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
gated = sum(1 for c in data if c.get("gated"))
non_gated = len(data) - gated
cats = {c["category"] for c in data if not c.get("gated")}
required = {"drafting", "revision", "citation", "impossible", "steering", "budget"}
missing = required - cats
print(f"{len(data)} {non_gated} {gated} {'|'.join(sorted(missing))}")
PY
)
TOTAL=$(echo "$COUNTS" | cut -d' ' -f1)
NON_GATED=$(echo "$COUNTS" | cut -d' ' -f2)
GATED=$(echo "$COUNTS" | cut -d' ' -f3)
MISSING=$(echo "$COUNTS" | cut -d' ' -f4)
# e2 unlocked the 5 web-research contracts, so all 20 are non-gated now.
[ "$NON_GATED" -ge 15 ] || fail "expected >=15 non-gated contracts, got $NON_GATED"
[ -z "$MISSING" ] || fail "non-gated contracts missing categories: $MISSING"
pass "contracts: $NON_GATED non-gated + $GATED e2-gated ($TOTAL total), all categories covered"

# 3. Behavioral: contracts run against the deterministic runtime and >=12/15 pass.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

EVAL_OUT=$(bash "$RUNNER" 2>&1)
echo "$EVAL_OUT" | tail -3
echo "$EVAL_OUT" | grep -qE "non-gated contracts passed" || { echo "$EVAL_OUT"; fail "agent eval did not report a result"; }
echo "$EVAL_OUT" | grep -qE "^\[FAIL\]" && { echo "$EVAL_OUT"; fail "an agent eval contract failed"; }
pass "agent eval passes at or above the 12/15 bar"

# 4. Core unit tests exercise the evaluator (pass, honesty, budget, steering, negative).
for t in \
  d7_completed_contract_passes_delivery_contract \
  d7_impossible_contract_requires_honesty_and_capability_request \
  d7_budget_contract_reports_partial_delivery \
  d7_steering_contract_is_reflected_in_steps \
  d7_detects_contract_violation; do
  grep -q "fn $t" "$CORE" || fail "expected d7 test $t is missing from agent_eval_core.rs"
done
D7_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test d7_ --quiet 2>&1)
echo "$D7_TEST_OUT" | grep -q "test result: ok" || { echo "$D7_TEST_OUT"; fail "d7 tests failed"; }
echo "$D7_TEST_OUT" | grep -q "FAILED" && { echo "$D7_TEST_OUT"; fail "d7 tests failed"; }
D7_PASSED=$(echo "$D7_TEST_OUT" | grep -oE "[0-9]+ passed" | awk '{s+=$1} END{print s+0}')
[ "$D7_PASSED" -ge 5 ] || { echo "$D7_TEST_OUT"; fail "expected >=5 d7 tests to run, saw $D7_PASSED"; }
pass "d7 evaluator unit tests pass ($D7_PASSED passed)"

# 5. Adjacent runtime gate still green.
bash "$ROOT_DIR/scripts/apex_d6_check.sh" >/dev/null 2>&1 || fail "apex d6 gate regressed"
pass "apex d6 gate still passes"

echo "--- apex d7 check passed ---"
