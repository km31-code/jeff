#!/usr/bin/env bash
# apex b4 check: consolidation and memory panel.
# Verifies durable fact consolidation, named consolidation budget visibility,
# pattern promotion, prompt-preview deletion behavior, and the Privacy Center
# memory panel. No external API calls or model downloads required.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
CONSOLIDATION_RS="$SRC/consolidation.rs"
STORE_RS="$SRC/store.rs"
MODELS_RS="$SRC/models.rs"
COMMANDS_RS="$SRC/commands.rs"
MAIN_RS="$SRC/main.rs"
PROACTIVE_RS="$SRC/proactive.rs"
COST_RS="$SRC/cost_governor.rs"
ROUTER_RS="$SRC/model_router.rs"
APP_TSX="$DESKTOP/src/App.tsx"
TAURI_CLIENT_TS="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex b4 consolidation and memory panel check ---"

# 1. consolidation module and fact model.
test -f "$CONSOLIDATION_RS" || fail "consolidation.rs missing"
grep -q "pub fn run_consolidation" "$CONSOLIDATION_RS" || fail "run_consolidation missing"
grep -q "FACT_KIND_PREFERENCE" "$CONSOLIDATION_RS" || fail "preference fact kind missing"
grep -q "FACT_KIND_CONSTRAINT" "$CONSOLIDATION_RS" || fail "constraint fact kind missing"
grep -q "FACT_KIND_DEADLINE" "$CONSOLIDATION_RS" || fail "deadline fact kind missing"
grep -q "FACT_KIND_PERSON" "$CONSOLIDATION_RS" || fail "person fact kind missing"
grep -q "FACT_KIND_PATTERN" "$CONSOLIDATION_RS" || fail "pattern fact kind missing"
grep -q "FACT_KIND_CONTEXT" "$CONSOLIDATION_RS" || fail "context fact kind missing"
grep -q "MERGE_SIMILARITY_THRESHOLD: f32 = 0.85" "$CONSOLIDATION_RS" \
  || fail "merge threshold must be >0.85 check"
grep -q "cosine_similarity" "$CONSOLIDATION_RS" || fail "fact merge does not use cosine similarity"
grep -q "MAX_FACTS: usize = 500" "$CONSOLIDATION_RS" || fail "500-fact cap missing"
grep -q "apply_decay" "$CONSOLIDATION_RS" || fail "decay pass missing"
grep -q "build_memory_prompt_context" "$CONSOLIDATION_RS" || fail "prompt preview builder missing"
pass "consolidation module implements fact kinds, merge, decay, cap, and prompt preview"

# 2. facts schema and DTOs.
grep -q "CREATE TABLE IF NOT EXISTS facts" "$STORE_RS" || fail "facts table missing"
grep -q "evidence_ids_json TEXT NOT NULL" "$STORE_RS" || fail "fact evidence list missing"
grep -q "last_reinforced TEXT NOT NULL" "$STORE_RS" || fail "fact last_reinforced missing"
grep -q "embedding BLOB NOT NULL" "$STORE_RS" || fail "fact embedding blob missing"
grep -q "DELETE FROM facts" "$STORE_RS" || fail "global clear does not delete facts"
grep -q "pub struct FactDto" "$MODELS_RS" || fail "FactDto missing"
grep -q "pub struct ConsolidationReportDto" "$MODELS_RS" || fail "ConsolidationReportDto missing"
grep -q "pub struct MemoryPromptPreviewDto" "$MODELS_RS" || fail "MemoryPromptPreviewDto missing"
pass "facts table, DTOs, and clear path present"

# 3. trigger, budget, and retired drift-count path.
grep -q "mod consolidation;" "$MAIN_RS" || fail "consolidation module not registered"
grep -q "spawn_memory_consolidation_poll" "$MAIN_RS" || fail "consolidation poll missing"
grep -q "MEMORY_CONSOLIDATION_IDLE_SECONDS: i64 = 10 \\* 60" "$MAIN_RS" \
  || fail "10-minute idle trigger missing"
grep -q "MEMORY_CONSOLIDATION_LAST_2AM_KEY" "$MAIN_RS" || fail "02:00 maintenance guard missing"
grep -q "get_privacy_user_profile_memory_enabled" "$MAIN_RS" || fail "consolidation missing privacy gate"
grep -q "CONSOLIDATION_BUDGET_KEY" "$CONSOLIDATION_RS" || fail "consolidation budget key not used"
grep -q "with_budget_key(CONSOLIDATION_BUDGET_KEY)" "$CONSOLIDATION_RS" \
  || fail "Craft consolidation pass not assigned to named budget"
grep -q "CONSOLIDATION_BUDGET_KEY" "$COST_RS" || fail "cost governor lacks consolidation budget"
grep -q "record_usage_for_budget_key" "$ROUTER_RS" || fail "model router cannot log named budget usage"
if grep -q "maybe_record_drift_struggle" "$PROACTIVE_RS"; then
  fail "drift-count struggle heuristic is still live in proactive monitor"
fi
pass "idle/02:00 trigger, named budget, and drift-count retirement verified"

# 4. backend commands and frontend memory panel.
grep -q "pub fn list_facts" "$COMMANDS_RS" || fail "list_facts command missing"
grep -q "pub fn delete_fact" "$COMMANDS_RS" || fail "delete_fact command missing"
grep -q "pub async fn run_memory_consolidation" "$COMMANDS_RS" || fail "manual consolidation command missing"
grep -q "pub fn preview_memory_prompt_context" "$COMMANDS_RS" || fail "prompt preview command missing"
grep -q "pub fn delete_episode" "$COMMANDS_RS" || fail "episode delete command missing"
grep -q "commands::list_facts" "$MAIN_RS" || fail "list_facts not registered"
grep -q "export interface FactDto" "$TAURI_CLIENT_TS" || fail "FactDto frontend type missing"
grep -q "runMemoryConsolidation" "$TAURI_CLIENT_TS" || fail "manual consolidation frontend binding missing"
grep -q "memory-facts-list" "$APP_TSX" || fail "memory facts panel missing"
grep -q "memory-delete-fact" "$APP_TSX" || fail "fact delete UI missing"
grep -q "memory-episodes-list" "$APP_TSX" || fail "episode list UI missing"
grep -q "memory-prompt-preview" "$APP_TSX" || fail "prompt preview UI missing"
pass "commands, frontend bindings, and Privacy Center memory panel wired"

# 5. behavioral gates.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

B4_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test b4_ --quiet 2>&1)
echo "$B4_TEST_OUT" | grep -q "test result: ok" || { echo "$B4_TEST_OUT"; fail "b4 tests failed"; }
echo "$B4_TEST_OUT" | grep -q "FAILED" && { echo "$B4_TEST_OUT"; fail "b4 tests failed"; }
grep -q "b4_near_duplicate_episodes_merge_to_one_fact_with_all_evidence" "$CONSOLIDATION_RS" \
  || fail "near-duplicate merge test missing"
grep -q "b4_decayed_fact_is_dropped_by_job" "$CONSOLIDATION_RS" \
  || fail "decay-drop test missing"
grep -q "b4_delete_fact_removes_it_from_prompt_preview" "$CONSOLIDATION_RS" \
  || fail "prompt-preview delete test missing"
grep -q "b4_consolidation_spend_appears_under_named_sub_budget" "$CONSOLIDATION_RS" \
  || fail "named budget test missing"
pass "b4 merge, decay, delete-preview, budget, and pattern tests pass"

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

echo "--- apex b4 check passed ---"
