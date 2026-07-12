#!/usr/bin/env bash
# apex e2 check: web research tools -- search/fetch, rate limit, query log,
# user-name guard, source ledger, and the unlocked web agent contracts.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
WEB="$SRC/web_tools.rs"
STORE="$SRC/store.rs"
COMMANDS="$SRC/commands.rs"
MAIN="$SRC/main.rs"
CONTRACTS="$ROOT_DIR/eval/agent_eval/contracts.json"
APP_TSX="$DESKTOP/src/App.tsx"
TAURI_CLIENT="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex e2 web research check ---"

# 1. Module + web substrate.
test -f "$WEB" || fail "web_tools.rs missing"
grep -q "CREATE TABLE IF NOT EXISTS web_query_log" "$STORE" || fail "web_query_log table missing"
grep -q "pub fn web_search" "$WEB" || fail "web.search missing"
grep -q "pub fn web_fetch" "$WEB" || fail "web.fetch missing"
grep -q "fn connected_search" "$WEB" || fail "connected web.search path missing"
grep -q "fn connected_fetch" "$WEB" || fail "connected web.fetch path missing"
grep -q "invoke_first_enabled_tool" "$WEB" || fail "web tools do not use the governed MCP bus"
grep -q "fn readable_extract" "$WEB" || fail "readable extraction missing"
grep -q "pub fn build_source_ledger" "$WEB" || fail "per-job source ledger missing"
grep -q "WEB_RATE_LIMIT_PER_HOUR: i64 = 10" "$WEB" || fail "10/hr rate limit missing"
grep -q "pub fn query_blocked_by_user_guard" "$WEB" || fail "user-name query guard missing"
grep -q "pub fn list_web_query_log" "$WEB" || fail "query log missing"
pass "web substrate: search/fetch/readability/ledger/rate-limit/guard/log present"

# 2. The 5 web-research agent contracts are unlocked (no longer e2-gated).
GATED=$(python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print(sum(1 for c in d if c.get('gated')))" "$CONTRACTS")
TOTAL=$(python3 -c "import json,sys; print(len(json.load(open(sys.argv[1]))))" "$CONTRACTS")
WEB_COUNT=$(python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print(sum(1 for c in d if c['category']=='web_research'))" "$CONTRACTS")
[ "$GATED" -eq 0 ] || fail "expected 0 gated contracts after e2, got $GATED"
[ "$WEB_COUNT" -ge 5 ] || fail "expected >=5 web-research contracts, got $WEB_COUNT"
[ "$TOTAL" -ge 20 ] || fail "expected >=20 contracts, got $TOTAL"
pass "all $WEB_COUNT web-research contracts unlocked ($TOTAL contracts, 0 gated)"
ZERO_CITATION_GATES=$(python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print(sum(1 for c in d if c['category']=='web_research' and c.get('require_zero_fabricated_citations')))" "$CONTRACTS")
[ "$ZERO_CITATION_GATES" -eq "$WEB_COUNT" ] || fail "every web contract must reject fabricated citations"

# 3. Behavioral: web contracts run and cite sources with zero fabrication.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

EVAL_OUT=$(bash "$ROOT_DIR/scripts/agent_eval.sh" 2>&1)
echo "$EVAL_OUT" | grep -qE "^\[FAIL\]" && { echo "$EVAL_OUT"; fail "an agent contract failed"; }
for wc in web_find_sources web_verify_claim web_recent_developments web_fact_check web_related_work; do
  echo "$EVAL_OUT" | grep -qE "^\[PASS\] $wc " || { echo "$EVAL_OUT"; fail "web contract $wc did not pass"; }
done
echo "$EVAL_OUT" | grep -q "0 e2-gated skipped" || { echo "$EVAL_OUT"; fail "contracts still e2-gated"; }
pass "all web-research contracts pass with grounded, cited output"

for t in \
  e2_search_ranks_and_logs_and_fetch_extracts \
  e2_source_ledger_carries_urls_for_citation \
  e2_rate_limit_trips_after_ten_per_hour \
  e2_user_name_guard_blocks_matching_query \
  e2_connected_web_search_and_fetch_use_discovered_mcp_tools; do
  grep -q "fn $t" "$WEB" || fail "expected e2 test $t is missing"
done
if ! E2_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test e2_ --quiet 2>&1); then
  echo "$E2_TEST_OUT"
  fail "e2 tests failed"
fi
echo "$E2_TEST_OUT" | grep -q "test result: ok" || { echo "$E2_TEST_OUT"; fail "e2 tests failed"; }
echo "$E2_TEST_OUT" | grep -q "FAILED" && { echo "$E2_TEST_OUT"; fail "e2 tests failed"; }
pass "e2 search/fetch/rate-limit/guard tests pass"

# 4. Commands + Privacy Center web surface.
for cmd in list_web_query_log set_web_user_name_guard web_search web_fetch; do
  grep -q "pub fn $cmd" "$COMMANDS" || fail "$cmd command missing"
  grep -q "commands::$cmd" "$MAIN" || fail "$cmd not registered"
done
grep -q "listWebQueryLog" "$TAURI_CLIENT" || fail "frontend web query log binding missing"
grep -q "privacy-surface-web-research" "$APP_TSX" || fail "Privacy Center web surface missing"
grep -q "web-user-guard-input" "$APP_TSX" || fail "user-name guard control missing"
grep -q "web-query-log" "$APP_TSX" || fail "web query log surface missing"
grep -q "web-source-selection-card" "$APP_TSX" || fail "interactive source-selection card missing"
grep -q "web-source-select" "$APP_TSX" || fail "source selection approval control missing"
pass "commands and Privacy Center web-research surface are wired"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend lint did not run"; }
pass "frontend TypeScript check passes"

FRONTEND_TEST_OUT=$(cd "$DESKTOP" && npm test -- --run 2>&1)
echo "$FRONTEND_TEST_OUT" | grep -qE "Test Files.*passed" || { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
echo "$FRONTEND_TEST_OUT" | grep -qE "[0-9]+ failed" && { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
pass "frontend tests pass"

if [ "${JEFF_SKIP_ADJACENT_GATES:-0}" != "1" ]; then
  if ! ADJACENT_OUT=$(JEFF_SKIP_ADJACENT_GATES=1 bash "$ROOT_DIR/scripts/apex_e1_check.sh" 2>&1); then
    echo "$ADJACENT_OUT"
    fail "apex e1 tool bus gate regressed"
  fi
  pass "apex e1 tool bus gate still passes"
fi

echo "--- apex e2 check passed ---"
