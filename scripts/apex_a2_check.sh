#!/usr/bin/env bash
# apex a2 check: cache-stable prompt assembly.
# Verifies character prompts are emitted as ordered SystemBlock lists, runtime
# character prompt paths preserve block metadata to the router/provider layer,
# Anthropic receives cache-control blocks, and cache metrics are accumulated.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
CHARACTER_RS="$SRC/character.rs"
MODEL_ROUTER_RS="$SRC/model_router.rs"
ANTHROPIC_RS="$SRC/providers/anthropic.rs"
LATENCY_RS="$SRC/latency.rs"
COMMANDS_RS="$SRC/commands.rs"
MAIN_RS="$SRC/main.rs"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex a2 cache-stable prompt assembly check ---"

test -f "$CHARACTER_RS" || fail "character.rs missing"
test -f "$MODEL_ROUTER_RS" || fail "model_router.rs missing"
test -f "$ANTHROPIC_RS" || fail "providers/anthropic.rs missing"
test -f "$LATENCY_RS" || fail "latency.rs missing"

# 1. Character builders emit ordered blocks with explicit cache hints.
grep -q "pub fn build_chat_system_blocks" "$CHARACTER_RS" || fail "chat SystemBlock builder missing"
grep -q "pub fn build_revision_system_blocks" "$CHARACTER_RS" || fail "revision SystemBlock builder missing"
grep -q "pub fn build_reorientation_system_blocks" "$CHARACTER_RS" || fail "reorientation SystemBlock builder missing"
grep -q "pub fn build_subtask_system_blocks" "$CHARACTER_RS" || fail "subtask SystemBlock builder missing"
grep -q "stable_block(base_character_prompt())" "$CHARACTER_RS" || fail "block 1 is not the static base character prompt"
grep -q "CacheHint::Stable" "$CHARACTER_RS" || fail "stable cache hint missing from character builders"
grep -q "CacheHint::Session" "$CHARACTER_RS" || fail "session cache hint missing from character builders"
grep -q "CacheHint::Volatile" "$CHARACTER_RS" || fail "volatile cache hint missing from character builders"
grep -q "a2_block_one_is_byte_stable_across_chat_builds" "$CHARACTER_RS" || fail "byte-stability unit test missing"
grep -q "a2_scripted_conversation_cacheable_ratio_exceeds_seventy_percent" "$CHARACTER_RS" || fail "20-turn cache-ratio test missing"
pass "character builders expose ordered cache-hinted SystemBlock lists"

# 2. Runtime character prompt paths preserve blocks instead of calling the
# compatibility string builders outside character.rs.
LEGACY_CALLS=$(grep -R "build_chat_system_prompt\|build_revision_system_prompt\|build_reorientation_system_prompt\|build_subtask_system_prompt" "$SRC" --include="*.rs" || true)
LEGACY_CALLS=$(printf "%s\n" "$LEGACY_CALLS" | grep -v "$CHARACTER_RS" || true)
if [ -n "$LEGACY_CALLS" ]; then
  echo "$LEGACY_CALLS"
  fail "character string builder call outside character.rs"
fi
grep -q "generate_response_blocks" "$SRC/chat.rs" || fail "chat generation does not preserve SystemBlock metadata"
grep -q "stream_blocks" "$SRC/chat_streaming.rs" || fail "chat streaming does not preserve SystemBlock metadata"
grep -q "generate_response_blocks" "$SRC/revision.rs" || fail "revision generation does not preserve SystemBlock metadata"
grep -q "generate_response_blocks" "$SRC/proactive.rs" || fail "proactive reorientation does not preserve SystemBlock metadata"
grep -q "generate_blocks_async" "$SRC/awareness_core.rs" || fail "async awareness synthesis does not preserve SystemBlock metadata"
grep -q "generate_response_blocks" "$SRC/subtask.rs" || fail "subtask generation does not preserve SystemBlock metadata"
pass "runtime character prompt paths preserve block metadata"

# 3. Router and Anthropic adapter consume the block shape.
grep -q "pub fn new_blocks" "$MODEL_ROUTER_RS" || fail "ModelRequest::new_blocks missing"
grep -q "pub fn generate_blocks" "$MODEL_ROUTER_RS" || fail "router generate_blocks missing"
grep -q "pub async fn generate_blocks_async" "$MODEL_ROUTER_RS" || fail "router generate_blocks_async missing"
grep -q "pub fn stream_blocks" "$MODEL_ROUTER_RS" || fail "router stream_blocks missing"
grep -q "join_system_blocks(&request.system_blocks)" "$MODEL_ROUTER_RS" || fail "OpenAI prefix-stable block join missing"
grep -q "system_blocks: &request.system_blocks" "$MODEL_ROUTER_RS" || fail "Anthropic blocking route does not receive SystemBlocks"
grep -q "pub system_blocks: &'a \\[SystemBlock\\]" "$ANTHROPIC_RS" || fail "AnthropicRequest lacks SystemBlock slice"
grep -q "fn system_content_blocks" "$ANTHROPIC_RS" || fail "Anthropic system content block builder missing"
grep -q "cache_control" "$ANTHROPIC_RS" || fail "Anthropic cache_control missing"
grep -q "CacheHint::Stable | CacheHint::Session" "$ANTHROPIC_RS" || fail "Anthropic cache breakpoint hints missing"
pass "router and Anthropic adapter consume SystemBlock cache hints"

# 4. Cache usage metrics are accumulated, logged, and exposed for debugging.
grep -q "pub fn cached_ratio" "$MODEL_ROUTER_RS" || fail "per-call cached_ratio missing"
grep -q "record_llm_usage" "$MODEL_ROUTER_RS" || fail "router does not record usage into latency metrics"
grep -q "cumulative_cached_ratio" "$MODEL_ROUTER_RS" || fail "usage log lacks cumulative cached ratio"
grep -q "pub struct LlmCacheMetrics" "$LATENCY_RS" || fail "LlmCacheMetrics missing"
grep -q "pub fn record_llm_usage" "$LATENCY_RS" || fail "latency usage accumulator missing"
grep -q "pub fn llm_cache_metrics" "$LATENCY_RS" || fail "latency cache metric reader missing"
grep -q "a2_cached_ratio_accumulates_llm_usage" "$LATENCY_RS" || fail "cache metric accumulator test missing"
grep -q "debug_llm_cache_metrics" "$COMMANDS_RS" || fail "debug cache metrics command missing"
grep -q "commands::debug_llm_cache_metrics" "$MAIN_RS" || fail "debug cache metrics command not registered"
pass "cached-token ratio metrics are accumulated and exposed"

# 5. Behavioral gates: compile, byte stability, scripted cache ratio, and router
# block tests all pass without requiring external API calls.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

A2_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test a2_ --quiet 2>&1)
echo "$A2_TEST_OUT" | grep -q "test result: ok" || { echo "$A2_TEST_OUT"; fail "a2-focused tests failed"; }
echo "$A2_TEST_OUT" | grep -q "FAILED" && { echo "$A2_TEST_OUT"; fail "a2-focused tests failed"; }
pass "a2 byte-stability and 20-turn cache-ratio tests pass"

ROUTER_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test model_router --quiet 2>&1)
echo "$ROUTER_TEST_OUT" | grep -q "test result: ok" || { echo "$ROUTER_TEST_OUT"; fail "model_router tests failed"; }
echo "$ROUTER_TEST_OUT" | grep -q "FAILED" && { echo "$ROUTER_TEST_OUT"; fail "model_router tests failed"; }
pass "model_router tests pass"

echo "--- apex a2 check passed ---"
