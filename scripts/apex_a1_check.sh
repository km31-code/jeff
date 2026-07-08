#!/usr/bin/env bash
# apex a1 check: model router and capability tiers.
# verifies the router exists, every reasoning call site declares a tier,
# no model-name strings leak outside the router/providers boundary, and
# the router unit tests pass. runs the live classification eval when an
# OPENAI_API_KEY is available.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
MODEL_ROUTER_RS="$SRC/model_router.rs"
ANTHROPIC_RS="$SRC/providers/anthropic.rs"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex a1 model router check ---"

# 1. router module with required symbols
test -f "$MODEL_ROUTER_RS" || fail "model_router.rs missing"
grep -q "pub enum Tier" "$MODEL_ROUTER_RS" || fail "Tier enum missing"
grep -q "Reflex" "$MODEL_ROUTER_RS" || fail "Reflex tier missing"
grep -q "pub struct RouterConfig" "$MODEL_ROUTER_RS" || fail "RouterConfig missing"
grep -q "pub struct ModelRouter" "$MODEL_ROUTER_RS" || fail "ModelRouter missing"
grep -q "pub struct ModelRequest" "$MODEL_ROUTER_RS" || fail "ModelRequest missing"
grep -q "pub struct ModelResponse" "$MODEL_ROUTER_RS" || fail "ModelResponse missing"
grep -q "pub struct SystemBlock" "$MODEL_ROUTER_RS" || fail "SystemBlock missing (a2 plumbing)"
grep -q "pub struct LlmUsage" "$MODEL_ROUTER_RS" || fail "LlmUsage missing"
grep -q "pub fn route(" "$MODEL_ROUTER_RS" || fail "route(request) API missing"
grep -q "pub fn route_streaming" "$MODEL_ROUTER_RS" || fail "route_streaming(request) API missing"
grep -q "fn stream" "$MODEL_ROUTER_RS" || fail "router stream dispatch missing"
grep -q "fn classify" "$MODEL_ROUTER_RS" || fail "router classify missing"
grep -q "fn handle" "$MODEL_ROUTER_RS" || fail "tier handle constructor missing"
grep -q "model_router_fallback" "$MODEL_ROUTER_RS" || fail "missing-key fallback notice missing"
pass "model_router.rs present with required symbols"

# 2. anthropic adapter
test -f "$ANTHROPIC_RS" || fail "providers/anthropic.rs missing"
grep -q "anthropic.com/v1/messages" "$ANTHROPIC_RS" || fail "anthropic messages endpoint missing"
grep -q "fn stream" "$ANTHROPIC_RS" || fail "anthropic streaming missing"
grep -q "content_block_delta" "$ANTHROPIC_RS" || fail "anthropic sse parsing missing"
pass "anthropic adapter present"

# 3. anthropic key management
grep -q "resolve_anthropic_api_key" "$SRC/secrets.rs" || fail "anthropic key resolution missing from secrets.rs"
grep -q "store_anthropic_api_key" "$SRC/commands.rs" || fail "store_anthropic_api_key command missing"
grep -q "commands::store_anthropic_api_key" "$SRC/main.rs" || fail "anthropic key command not registered"
grep -q "commands::set_tier_model_map" "$SRC/main.rs" || fail "tier config command not registered"
pass "anthropic key + tier config commands wired"

# 4. grep gate: no llm model-name strings outside model_router.rs and providers/
LEAKS=$(grep -rn "gpt-4o\|gpt-4\|claude-3\|claude-son\|claude-hai\|claude-opus\|text-embedding" "$SRC" --include="*.rs" \
  | grep -v "model_router.rs" \
  | grep -v "src/providers" || true)
if [ -n "$LEAKS" ]; then
  echo "$LEAKS"
  fail "model-name strings found outside model_router.rs / providers/"
fi
pass "no model strings outside router/providers boundary"

# 5. call sites declare tiers
grep -q "Tier::Conversation" "$SRC/chat_streaming.rs" || fail "chat streaming does not declare conversation tier"
grep -q "Tier::Judgment" "$SRC/awareness_core.rs" || fail "synthesis does not declare judgment tier"
grep -q "craft_reasoning" "$SRC/commands.rs" || fail "craft tier not used in commands"
grep -q "judgment_reasoning" "$SRC/commands.rs" || fail "judgment tier not used in commands"
grep -q "model_router: Arc<ModelRouter>" "$SRC/state.rs" || fail "model router not held in JeffState"
grep -q "model_router.classify" "$SRC/commands.rs" || fail "classification not routed through router"
grep -q "model_router.stream" "$SRC/chat_streaming.rs" || fail "streaming chat does not call router"
grep -q "state.craft_reasoning().as_ref()" "$SRC/commands.rs" || fail "revision path does not use craft tier handle"
grep -q "state.craft_reasoning()" "$SRC/commands.rs" || fail "subtask path does not use craft tier handle"
grep -q "state.judgment_reasoning().as_ref()" "$SRC/commands.rs" || fail "proactive path does not use judgment tier handle"
pass "call sites declare tiers (conversation/judgment/craft/reflex)"

# 6. usage logging for the a4 cost governor
grep -q "llm_usage" "$MODEL_ROUTER_RS" || fail "usage logging missing"
pass "usage logging present"

# 7. blind a/b gate packet is present and well-formed
"$ROOT_DIR/scripts/apex_a1_ab_generate.sh" --check
"$ROOT_DIR/scripts/apex_a1_ab_packet.sh" --check
"$ROOT_DIR/scripts/apex_a1_ab_score.sh" --check
pass "blind a/b packet protocol is present"

# 8. behavioral: router unit tests
ROUTER_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test model_router --quiet 2>&1)
echo "$ROUTER_TEST_OUT" | grep -q "test result: ok" || fail "model_router unit tests failed"
echo "$ROUTER_TEST_OUT" | grep -q "FAILED" && fail "model_router unit tests failed"
pass "model_router unit tests pass"

# 9. behavioral: live classification through the router. this sends eval
# prompts to the configured provider, so it requires an explicit opt-in in
# addition to OPENAI_API_KEY.
if [ "${JEFF_RUN_EXTERNAL_EVAL:-}" = "1" ] && [ -n "${OPENAI_API_KEY:-}" ]; then
  if ! EVAL_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && \
    JEFF_PREFER_ENV_OPENAI_API_KEY=1 \
    JEFF_CLASSIFY_TIMEOUT_OPENAI_MS=5000 \
    JEFF_INTENT_EVAL_P50_BUDGET_MS=5000 \
    JEFF_INTENT_EVAL_P95_BUDGET_MS=10000 \
    cargo test --test intent_eval --quiet 2>&1); then
    echo "$EVAL_OUT"
    fail "live intent eval through router failed"
  fi
  echo "$EVAL_OUT" | grep -q "test result: ok" || fail "live intent eval through router failed"
  echo "$EVAL_OUT" | grep -q "FAILED" && fail "live intent eval through router failed"
  pass "live intent eval through router passes"
else
  echo "SKIP: set JEFF_RUN_EXTERNAL_EVAL=1 with OPENAI_API_KEY to run live external eval"
fi

echo "--- apex a1 check passed ---"
