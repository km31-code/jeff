#!/usr/bin/env bash
# apex a5 check: character eval harness, router-grader amendment, and release gate.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
RUNNER="$ROOT_DIR/desktop/src-tauri/src/bin/character_eval.rs"
HARNESS="$ROOT_DIR/scripts/character_eval.sh"
PHASE32="$ROOT_DIR/scripts/phase32_check.sh"
WORKFLOW="$ROOT_DIR/.github/workflows/release.yml"
PLAN="$ROOT_DIR/docs/PHASES_TRANSFORMATION.md"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex a5 character eval harness check ---"

test -f "$RUNNER" || fail "character eval Rust runner missing"
grep -q "ModelRouter::new" "$RUNNER" || fail "grader does not instantiate model router"
grep -q "Tier::Judgment" "$RUNNER" || fail "grader does not use Judgment tier"
grep -q "ProviderKind::OpenAi" "$RUNNER" || fail "A5 OpenAI-only eval config missing"
grep -q "OPENAI_FALLBACK_MODEL" "$RUNNER" || fail "OpenAI fallback model default missing"
if grep -q "openai_generate" "$RUNNER"; then
  fail "runner calls OpenAI provider directly instead of model router"
fi
pass "grader runs through model router at Judgment tier"

test -x "$HARNESS" || fail "scripts/character_eval.sh missing or not executable"
grep -q "JEFF_CHARACTER_EVAL_SAMPLE_SIZE:-15" "$HARNESS" || fail "sample size is not 15"
grep -q "JEFF_CHARACTER_EVAL_PASS_BAR:-13" "$HARNESS" || fail "pass bar is not 13"
grep -q "negative_target" "$HARNESS" || fail "sample does not enforce negative coverage"
grep -q "JEFF_PREFER_ENV_OPENAI_API_KEY=1" "$HARNESS" || fail "eval does not prefer env OpenAI key"
pass "character eval script has A5 sample size, pass bar, and key behavior"

test -f "$WORKFLOW" || fail "release workflow missing"
grep -q "character-eval:" "$WORKFLOW" || fail "release workflow missing character-eval job"
grep -q "run: bash scripts/character_eval.sh" "$WORKFLOW" || fail "release workflow does not run character eval"
# the workflow was restructured so build gates on character-eval
# (needs: [test, character-eval]); sign -> build, so signing/release is
# transitively gated on character eval passing. this matches the e7 ship gate.
grep -q "needs: \\[test, character-eval\\]" "$WORKFLOW" || fail "build job is not gated on character-eval (release not gated on character eval)"
pass "release workflow gates the build (and thus signing) on character eval"

grep -q "Run \`scripts/character_eval.sh\`" "$PLAN" || fail "Phase 25 checklist not updated"
grep -q "At least 13 of 15 sampled cases pass" "$PLAN" || fail "Phase 25 exit criteria not updated with A5 bar"
pass "Phase 25 retroactive eval gate is documented"

bash "$PHASE32" || fail "phase32 check failed"
pass "phase32 static and corpus checks pass"

CARGO_CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --bin character_eval --quiet 2>&1)
if [ -n "$CARGO_CHECK_OUT" ]; then
  echo "$CARGO_CHECK_OUT"
  fail "character_eval binary cargo check emitted warnings or errors"
fi
pass "character_eval binary checks without warnings"

if [ "${JEFF_RUN_LIVE_CHARACTER_EVAL:-0}" = "1" ]; then
  JEFF_CHARACTER_EVAL_SEED="${JEFF_CHARACTER_EVAL_SEED:-apex-a5}" bash "$HARNESS" || fail "live A5 character eval failed"
  pass "live A5 character eval passes"
else
  echo "SKIP: live A5 character eval not run; set JEFF_RUN_LIVE_CHARACTER_EVAL=1 after approving the OpenAI payload"
fi

echo "--- apex a5 check passed ---"
