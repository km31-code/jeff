#!/usr/bin/env bash
# phase 32 check: character eval corpus, guide, and harness.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
EVAL_JSON="$ROOT_DIR/eval/character_eval.json"
GUIDE="$ROOT_DIR/eval/character_eval_guide.md"
HARNESS="$ROOT_DIR/scripts/character_eval.sh"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- phase 32 character eval check ---"

test -f "$EVAL_JSON" || fail "eval/character_eval.json missing"
python3 - "$EVAL_JSON" <<'PY' || fail "character_eval.json failed schema/distribution checks"
import json
import sys
from collections import Counter

path = sys.argv[1]
with open(path, "r", encoding="utf-8") as handle:
    cases = json.load(handle)

assert isinstance(cases, list), "eval file must be an array"
assert len(cases) >= 30, f"{len(cases)} cases"

taxonomy = [
    "FillerPhrase",
    "PermissionSeeking",
    "DisagreementAsQuestion",
    "TrailingSummary",
    "ResultWithoutAssessment",
    "ExcessiveHedge",
    "NonAnswer",
    "SelfNarration",
]
counts = Counter()
clean = 0
ids = set()
for case in cases:
    assert isinstance(case.get("id"), str) and case["id"], "missing id"
    assert case["id"] not in ids, f"duplicate id {case['id']}"
    ids.add(case["id"])
    for key in ["context", "input", "jeff_output"]:
        assert isinstance(case.get(key), str) and case[key].strip(), f"{case['id']} missing {key}"
    violations = case.get("violations")
    assert isinstance(violations, list), f"{case['id']} violations must be array"
    if not violations:
        clean += 1
    for violation in violations:
        assert violation in taxonomy, f"{case['id']} unknown violation {violation}"
        counts[violation] += 1

assert clean >= 18, f"{clean} clean cases"
for violation in taxonomy:
    assert counts[violation] >= 2, f"{violation} has {counts[violation]} cases"
PY
pass "eval corpus parses with >=30 cases, >=18 clean cases, and all violation types covered twice"

test -x "$HARNESS" || fail "scripts/character_eval.sh is missing or not executable"
grep -q "JEFF_CHARACTER_EVAL_SAMPLE_SIZE:-15" "$HARNESS" || fail "character eval sample size is not 15"
grep -q "JEFF_CHARACTER_EVAL_PASS_BAR:-13" "$HARNESS" || fail "character eval pass bar is not 13"
grep -q -- "--bin character_eval" "$HARNESS" || fail "character eval does not call Rust router runner"
pass "character eval harness is executable with A5 sample size and pass bar"

test -f "$GUIDE" || fail "eval/character_eval_guide.md missing"
for token in FillerPhrase PermissionSeeking DisagreementAsQuestion TrailingSummary ResultWithoutAssessment ExcessiveHedge NonAnswer SelfNarration; do
  grep -q "$token" "$GUIDE" || fail "guide missing $token"
done
grep -q "13/15" "$GUIDE" || fail "guide missing amended pass bar"
pass "character eval guide documents taxonomy and run policy"

CARGO_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test character_eval_json --quiet 2>&1)
echo "$CARGO_OUT" | grep -q "test result: ok" || { echo "$CARGO_OUT"; fail "character eval JSON integration test failed"; }
echo "$CARGO_OUT" | grep -q "FAILED" && { echo "$CARGO_OUT"; fail "character eval JSON integration test failed"; }
pass "character eval JSON integration test passes"

if [ -n "${OPENAI_API_KEY:-}" ]; then
  bash "$HARNESS" || fail "live character eval failed"
  pass "live character eval passes"
else
  echo "SKIP: OPENAI_API_KEY not set in shell; live character eval not run by phase32_check.sh"
fi

echo "--- phase 32 check passed ---"
