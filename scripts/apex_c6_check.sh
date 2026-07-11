#!/usr/bin/env bash
# apex c6 check: judgment eval suite.
# Verifies the stage-2 fixture corpus, deterministic evaluator, 85% agreement
# gate, voice-transcript character cases, and release workflow wiring.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
EVAL_JSON="$ROOT_DIR/eval/judgment_eval.json"
CHAR_EVAL_JSON="$ROOT_DIR/eval/character_eval.json"
CORE="$SRC/judgment_eval_core.rs"
SYNTH="$SRC/synthesis.rs"
BIN="$SRC/bin/judgment_eval.rs"
HARNESS="$ROOT_DIR/scripts/judgment_eval.sh"
RELEASE="$ROOT_DIR/.github/workflows/release.yml"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex c6 judgment eval suite check ---"

# 1. shared evaluator is production-adjacent and synthesis calls it.
test -f "$CORE" || fail "judgment_eval_core.rs missing"
grep -q "pub fn evaluate_stage2_economics" "$CORE" || fail "stage-2 economics evaluator missing"
grep -q "pub fn evaluate_stage2_fixture" "$CORE" || fail "fixture evaluator missing"
grep -q "JudgmentStage2Input" "$CORE" || fail "stage-2 input struct missing"
grep -q "evaluate_stage2_economics" "$SYNTH" || fail "synthesis fallback does not use shared evaluator"
grep -q "fallback_stage2_economics" "$SYNTH" || fail "production fallback economics adapter missing"
pass "shared stage-2 evaluator is used by synthesis fallback and eval runner"

# 2. fixture corpus has required coverage and valid labels.
test -f "$EVAL_JSON" || fail "eval/judgment_eval.json missing"
python3 - "$EVAL_JSON" <<'PY' || fail "judgment_eval.json schema/coverage failed"
import json
import sys
from collections import Counter

path = sys.argv[1]
with open(path, "r", encoding="utf-8") as handle:
    cases = json.load(handle)
if not isinstance(cases, list):
    raise SystemExit("root must be a list")
if len(cases) < 20:
    raise SystemExit(f"expected >=20 cases, got {len(cases)}")
ids = [case.get("id") for case in cases]
if len(ids) != len(set(ids)):
    raise SystemExit("duplicate ids")
required_categories = {
    "deep_focus_hold",
    "natural_boundary_speak",
    "deadline_escalation",
    "repeated_ignore_suppression",
    "multi_signal_integration",
    "quiet_mode_suppression",
    "low_confidence_drop",
}
categories = Counter(case.get("category") for case in cases)
missing = sorted(required_categories - set(categories))
if missing:
    raise SystemExit(f"missing categories: {missing}")
decisions = Counter()
for case in cases:
    for key in ["snapshot", "candidate", "ledger", "expected"]:
        if key not in case:
            raise SystemExit(f"{case.get('id')} missing {key}")
    expected = case["expected"]
    decision = expected.get("decision")
    if decision not in {"speak", "hold", "drop"}:
        raise SystemExit(f"{case.get('id')} invalid decision {decision}")
    channels = expected.get("channels")
    if not isinstance(channels, list) or not channels:
        raise SystemExit(f"{case.get('id')} missing channels")
    for channel in channels:
        if channel not in {"voice", "bubble", "notification", "silent_card"}:
            raise SystemExit(f"{case.get('id')} invalid channel {channel}")
    decisions[decision] += 1
for decision in ["speak", "hold", "drop"]:
    if decisions[decision] == 0:
        raise SystemExit(f"missing {decision} cases")
print(f"{len(cases)} cases; categories={dict(categories)}; decisions={dict(decisions)}")
PY
pass "judgment eval corpus covers required categories and speak/hold/drop labels"

# 3. harness and Rust runner enforce the 85% pass bar.
test -x "$HARNESS" || fail "scripts/judgment_eval.sh missing or not executable"
test -f "$BIN" || fail "judgment_eval Rust runner missing"
grep -q -- "--bin judgment_eval" "$HARNESS" || fail "harness does not call Rust judgment_eval runner"
grep -q "DEFAULT_PASS_BAR: f32 = 0.85" "$BIN" || fail "default 85% pass bar missing"
JUDGMENT_OUT=$("$HARNESS" 2>&1)
echo "$JUDGMENT_OUT" | grep -q "24/24 passed" || { echo "$JUDGMENT_OUT"; fail "judgment eval did not pass all seeded cases"; }
echo "$JUDGMENT_OUT" | grep -q "agreement 100.0%" || { echo "$JUDGMENT_OUT"; fail "judgment eval agreement missing"; }
pass "judgment eval harness passes at >=85% agreement"

# 4. character eval has the 10 C6 voice transcript cases.
python3 - "$CHAR_EVAL_JSON" <<'PY' || fail "voice transcript character cases missing/invalid"
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as handle:
    cases = json.load(handle)
voice = [case for case in cases if str(case.get("id", "")).startswith("c6_voice_transcript_")]
if len(voice) != 10:
    raise SystemExit(f"expected 10 c6 voice cases, got {len(voice)}")
if any(case.get("violations") for case in voice):
    raise SystemExit("voice transcript cases must be clean positives")
if not all("[voice" in case.get("input", "") for case in voice):
    raise SystemExit("voice transcript cases must be labeled as voice inputs")
print("voice cases:", ", ".join(case["id"] for case in voice))
PY
pass "character eval includes 10 clean voice-transcript cases"

# 5. release workflow wires both judgment and voice character eval gates.
grep -q "bash scripts/judgment_eval.sh" "$RELEASE" || fail "release test job does not run judgment eval"
grep -q "voice transcript character eval" "$RELEASE" || fail "release workflow lacks dedicated voice eval step"
grep -q "c6_voice_transcript_" "$RELEASE" || fail "release voice eval does not filter c6 voice cases"
grep -q "JEFF_CHARACTER_EVAL_SAMPLE_SIZE=10" "$RELEASE" || fail "release voice eval does not run all 10 voice cases"
pass "release workflow runs judgment eval and dedicated voice transcript character eval"

# 6. compile, focused tests, corpus gate, and adjacent C1/C2 gates.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

C6_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test c6_ --quiet 2>&1)
echo "$C6_TEST_OUT" | grep -q "test result: ok" || { echo "$C6_TEST_OUT"; fail "c6 tests failed"; }
echo "$C6_TEST_OUT" | grep -q "FAILED" && { echo "$C6_TEST_OUT"; fail "c6 tests failed"; }
pass "c6 stage-2 economics tests pass"

bash "$ROOT_DIR/scripts/phase32_check.sh" >/dev/null 2>&1 || fail "phase32 character-eval corpus gate regressed"
pass "character eval corpus gate still passes"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend TypeScript check did not run"; }
pass "frontend TypeScript check passes"

bash "$ROOT_DIR/scripts/apex_c1_check.sh" >/dev/null 2>&1 || fail "apex c1 two-stage gate regressed"
pass "apex c1 two-stage gate still passes"

bash "$ROOT_DIR/scripts/apex_c2_check.sh" >/dev/null 2>&1 || fail "apex c2 ledger/focus gate regressed"
pass "apex c2 ledger/focus gate still passes"

echo "SKIP: live voice-transcript character grading remains OpenAI-key-gated in"
echo "      scripts/character_eval.sh and the release workflow, consistent with A5."
echo "--- apex c6 check passed ---"
