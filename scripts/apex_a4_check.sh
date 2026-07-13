#!/usr/bin/env bash
# apex a4 check: cost governor and spend visibility.
# Verifies durable usage metering, tier budgets with graceful degradation,
# visible Privacy Center spend controls, and focused runtime tests. No external
# API calls are required.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
COST_GOVERNOR_RS="$SRC/cost_governor.rs"
STORE_RS="$SRC/store.rs"
MODEL_ROUTER_RS="$SRC/model_router.rs"
COMMANDS_RS="$SRC/commands.rs"
MAIN_RS="$SRC/main.rs"
MODELS_RS="$SRC/models.rs"
APP_TSX="$DESKTOP/src/App.tsx"
APP_TEST_TSX="$DESKTOP/src/App.test.tsx"
TAURI_CLIENT_TS="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex a4 cost governor check ---"

test -f "$COST_GOVERNOR_RS" || fail "cost_governor.rs missing"
grep -q "llm_usage_log" "$STORE_RS" || fail "llm_usage_log table missing"
grep -q "append_llm_usage_log" "$STORE_RS" || fail "llm usage append method missing"
grep -q "sum_llm_usage_today" "$STORE_RS" || fail "today spend aggregation missing"
grep -q "llm_usage_history" "$STORE_RS" || fail "7-day spend history query missing"
grep -q "CostGovernorStatusDto" "$MODELS_RS" || fail "cost governor status DTO missing"
pass "durable usage log and spend DTOs are present"

grep -q "pub fn preflight" "$COST_GOVERNOR_RS" || fail "budget preflight missing"
grep -q "pub fn record_usage" "$COST_GOVERNOR_RS" || fail "usage recorder missing"
grep -q "pub fn set_daily_budget_usd" "$COST_GOVERNOR_RS" || fail "budget setter missing"
grep -q "SPECULATION_BUDGET_KEY" "$COST_GOVERNOR_RS" || fail "speculation sub-budget hook missing"
grep -q "CONSOLIDATION_BUDGET_KEY" "$COST_GOVERNOR_RS" || fail "consolidation sub-budget hook missing"
grep -q "Tier::Craft => vec!\\[Tier::Craft, Tier::Judgment, Tier::Conversation\\]" "$COST_GOVERNOR_RS" \
  || fail "Craft degradation chain missing"
grep -q "Tier::Judgment => vec!\\[Tier::Judgment, Tier::Conversation\\]" "$COST_GOVERNOR_RS" \
  || fail "Judgment degradation chain missing"
grep -q "Cost governor moved" "$COST_GOVERNOR_RS" || fail "companion spend notice missing"
pass "budget preflight, degradation map, and notices are present"

grep -q "cost_governor::preflight" "$MODEL_ROUTER_RS" || fail "router does not preflight budgets"
grep -q "cost_governor::record_usage" "$MODEL_ROUTER_RS" || fail "router does not persist usage"
grep -q "model_router_budget_degraded" "$MODEL_ROUTER_RS" || fail "router degradation log missing"
grep -q "let purpose = request" "$MODEL_ROUTER_RS" || fail "router does not preserve request purpose"
grep -q "intent_classification" "$MODEL_ROUTER_RS" || fail "intent classification purpose missing"
pass "router records usage and degrades over-budget work"

grep -q "get_cost_governor_status" "$COMMANDS_RS" || fail "status command missing"
grep -q "set_llm_daily_budget" "$COMMANDS_RS" || fail "budget command missing"
grep -q "commands::get_cost_governor_status" "$MAIN_RS" || fail "status command not registered"
grep -q "commands::set_llm_daily_budget" "$MAIN_RS" || fail "budget command not registered"
grep -q "cost_governor: crate::cost_governor::status" "$COMMANDS_RS" || fail "privacy dashboard lacks spend status"
pass "commands and Privacy Center dashboard are wired"

grep -q "CostGovernorStatusDto" "$TAURI_CLIENT_TS" || fail "frontend cost DTO missing"
grep -q "setLlmDailyBudget" "$TAURI_CLIENT_TS" || fail "frontend budget command missing"
grep -q "privacy-surface-spend" "$APP_TSX" || fail "Privacy Center spend section missing"
grep -q "cost-governor-today" "$APP_TSX" || fail "today spend UI missing"
grep -q "cost-budget-" "$APP_TSX" || fail "budget edit fields missing"
grep -q "cost-history-list" "$APP_TSX" || fail "7-day history UI missing"
grep -q "shows spend status and edits tier budgets" "$APP_TEST_TSX" || fail "frontend spend test missing"
pass "frontend spend visibility and budget editing are wired"

CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

A4_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test a4_ --quiet 2>&1)
echo "$A4_TEST_OUT" | grep -q "test result: ok" || { echo "$A4_TEST_OUT"; fail "a4-focused tests failed"; }
echo "$A4_TEST_OUT" | grep -q "FAILED" && { echo "$A4_TEST_OUT"; fail "a4-focused tests failed"; }
pass "a4 forced-budget, spend-total, and runaway-loop tests pass"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend TypeScript check did not run"; }
pass "frontend TypeScript check passes"

FRONTEND_TEST_OUT=$(cd "$DESKTOP" && npm test -- --run 2>&1)
# the frontend test count grows as suites are added; assert green, not a fixed
# count (matches the e7 ship gate's flexible pattern).
echo "$FRONTEND_TEST_OUT" | grep -qE "Test Files.*passed" || { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
echo "$FRONTEND_TEST_OUT" | grep -qE "[0-9]+ failed" && { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
pass "frontend tests pass"

echo "--- apex a4 check passed ---"
