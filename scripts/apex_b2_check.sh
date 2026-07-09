#!/usr/bin/env bash
# apex b2 check: goal understanding.
# Verifies the reflex-tier structured extractor + deterministic heuristic
# replace prefix matching on the live path, the lull-triggered extraction loop
# is wired, the snapshot reads the extracted goal, and the eval set + harness
# quantify the improvement over the retired matcher. No external API calls
# required (the >=85% llm gate is env-gated, like the A1/A5 live evals).

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
GOAL_RS="$SRC/goal_extraction.rs"
AWARENESS_RS="$SRC/awareness_core.rs"
RELATIONAL_RS="$SRC/relational_model.rs"
STATE_RS="$SRC/state.rs"
MAIN_RS="$SRC/main.rs"
EVAL_JSON="$ROOT_DIR/eval/goal_extraction_eval.json"
EVAL_BIN="$SRC/bin/goal_eval.rs"
HARNESS="$ROOT_DIR/scripts/goal_eval.sh"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex b2 goal understanding check ---"

# 1. extractor module.
test -f "$GOAL_RS" || fail "goal_extraction.rs missing"
grep -q "pub struct GoalExtraction" "$GOAL_RS" || fail "GoalExtraction struct missing"
grep -q "pub fn extract_goal(" "$GOAL_RS" || fail "llm extractor missing"
grep -q "pub fn extract_goal_heuristic" "$GOAL_RS" || fail "heuristic extractor missing"
grep -q "pub fn parse_goal_json" "$GOAL_RS" || fail "structured JSON parsing missing"
grep -q "Tier::Reflex" "$GOAL_RS" || fail "extractor does not run at Reflex tier"
grep -q "evidence_quote" "$GOAL_RS" || fail "structured output lacks evidence_quote"
pass "reflex-tier structured extractor + heuristic present"

# 2. lull-triggered loop wired; never on a response path.
grep -q "mod goal_extraction;" "$MAIN_RS" || fail "goal_extraction module not registered"
grep -q "spawn_goal_extraction_poll" "$MAIN_RS" || fail "goal extraction loop missing"
grep -q "GOAL_LULL_SETTLE_SECONDS" "$MAIN_RS" || fail "lull settle threshold missing"
grep -q "spawn_blocking" "$MAIN_RS" || fail "blocking extractor not moved off the async worker"
grep -q "should_extract" "$STATE_RS" || fail "per-turn extraction dedup guard missing"
grep -q "get_privacy_user_profile_memory_enabled" "$MAIN_RS" \
  || fail "background goal extraction does not respect profile-memory privacy"
if grep -q "recent.reverse()" "$MAIN_RS"; then
  fail "goal extraction loop reverses list_recent_chat_messages output"
fi
grep -q "last_user_message.id" "$MAIN_RS" || fail "dedup must key on message id, not rounded timestamp"
pass "lull-triggered extraction loop wired off the response path"

# 3. snapshot + relational model read the extractor, prefix retired from live path.
grep -q "latest_active_goal_text" "$AWARENESS_RS" || fail "snapshot does not read the stored extracted goal"
grep -q "goal_extraction::extract_goal_heuristic" "$AWARENESS_RS" || fail "snapshot lacks heuristic fallback"
grep -q "pub fn latest_active_goal_text" "$RELATIONAL_RS" || fail "latest_active_goal_text missing"
grep -q "goal_extraction::heuristic_goal_from_message" "$RELATIONAL_RS" \
  || fail "record_message_signals still on the retired prefix matcher"
# the live snapshot path must no longer call the prefix matcher directly.
if grep -q "extract_current_goal(&recent_10)" "$AWARENESS_RS"; then
  fail "snapshot still calls the retired prefix matcher extract_current_goal"
fi
pass "snapshot + relational model consume the extractor; prefix retired from live path"

# 4. eval set + harness.
test -f "$EVAL_JSON" || fail "goal_extraction_eval.json missing"
CASE_COUNT=$(grep -c "\"id\"" "$EVAL_JSON" || true)
[ "$CASE_COUNT" -ge 30 ] || fail "eval set has < 30 cases ($CASE_COUNT)"
grep -q "\"no_goal\"" "$EVAL_JSON" || fail "eval set missing no-goal controls"
grep -q "\"paraphrase\"" "$EVAL_JSON" || fail "eval set missing paraphrase cases"
test -f "$EVAL_BIN" || fail "goal_eval binary missing"
test -x "$HARNESS" || fail "scripts/goal_eval.sh missing or not executable"
pass "labeled eval set (>=30 cases) and harness present"

# 5. behavioral: clean compile, b2 tests, deterministic contrast.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

B2_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test b2_ --quiet 2>&1)
echo "$B2_TEST_OUT" | grep -q "test result: ok" || { echo "$B2_TEST_OUT"; fail "b2 tests failed"; }
echo "$B2_TEST_OUT" | grep -q "FAILED" && { echo "$B2_TEST_OUT"; fail "b2 tests failed"; }
pass "b2 extractor + heuristic-beats-prefix contrast tests pass"

# deterministic eval: the bin enforces prefix < 40% and a clear heuristic margin.
EVAL_OUT=$(bash "$HARNESS" 2>&1)
echo "$EVAL_OUT"
echo "$EVAL_OUT" | grep -q "retired prefix matcher" || fail "eval output does not record the prefix contrast"
echo "$EVAL_OUT" | grep -q "heuristic extractor" || fail "eval output does not record the heuristic score"
pass "goal eval records prefix contrast and heuristic score"

if [ "${JEFF_RUN_EXTERNAL_EVAL:-}" = "1" ] && [ -n "${OPENAI_API_KEY:-}" ]; then
  echo "$EVAL_OUT" | grep -q "llm extractor:" || fail "llm eval did not run"
  pass "live llm goal eval ran (>=85% enforced by the harness)"
else
  echo "SKIP: set JEFF_RUN_EXTERNAL_EVAL=1 with OPENAI_API_KEY to run the >=85% llm goal eval"
fi

echo "--- apex b2 check passed ---"
