#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
CLASSIFIER_RS="$ROOT_DIR/desktop/src-tauri/src/classifier.rs"
PROVIDERS_RS="$ROOT_DIR/desktop/src-tauri/src/providers.rs"
MODELS_RS="$ROOT_DIR/desktop/src-tauri/src/models.rs"
COMMANDS_RS="$ROOT_DIR/desktop/src-tauri/src/commands.rs"
MAIN_RS="$ROOT_DIR/desktop/src-tauri/src/main.rs"
TAURI_CLIENT_TS="$ROOT_DIR/desktop/src/tauriClient.ts"
APP_TSX="$ROOT_DIR/desktop/src/App.tsx"
EVAL_SET="$ROOT_DIR/tests/fixtures/intent_eval_set.json"
EVAL_HARNESS="$ROOT_DIR/desktop/src-tauri/tests/intent_eval.rs"

echo "--- phase 14 real intent understanding check ---"

fail() { echo "FAIL: $1" >&2; exit 1; }
pass() { echo "PASS: $1"; }

# 1. classifier.rs exists with required symbols (m14.1)
# apex a1: classification entry point moved to model_router.classify (reflex
# tier); classifier.rs keeps the system prompt and response parsing.
test -f "$CLASSIFIER_RS" || fail "classifier.rs missing"
MODEL_ROUTER_RS="$ROOT_DIR/desktop/src-tauri/src/model_router.rs"
grep -q "fn classify" "$MODEL_ROUTER_RS" || fail "classify function missing from model_router.rs"
grep -q "fn parse_classification" "$CLASSIFIER_RS" || fail "parse_classification function missing from classifier.rs"
# request formatting may live in classifier.rs or provider seam after phase 17 refactor.
if ! grep -q "response_format" "$CLASSIFIER_RS" && ! grep -q "response_format" "$PROVIDERS_RS"; then
  fail "response_format json_object missing from classifier/provider path"
fi
grep -q "SYSTEM_PROMPT" "$CLASSIFIER_RS" || fail "SYSTEM_PROMPT constant missing from classifier.rs"
pass "classifier.rs present with required symbols (m14.1)"

# 2. intent DTOs declared in models.rs (m14.1)
grep -q "IntentLabel" "$MODELS_RS" || fail "IntentLabel missing from models.rs"
grep -q "IntentSlotsDto" "$MODELS_RS" || fail "IntentSlotsDto missing from models.rs"
grep -q "IntentClassificationDto" "$MODELS_RS" || fail "IntentClassificationDto missing from models.rs"
grep -q "rename_all.*lowercase" "$MODELS_RS" || fail "serde rename_all lowercase missing from IntentLabel"
pass "all phase 14 DTOs declared in models.rs (m14.1)"

# 3. IntentLabel variants declared
grep -q "Answer" "$MODELS_RS" || fail "IntentLabel::Answer variant missing"
grep -q "Revision" "$MODELS_RS" || fail "IntentLabel::Revision variant missing"
grep -q "Subtask" "$MODELS_RS" || fail "IntentLabel::Subtask variant missing"
grep -q "Suggestion" "$MODELS_RS" || fail "IntentLabel::Suggestion variant missing"
grep -q "Unknown" "$MODELS_RS" || fail "IntentLabel::Unknown variant missing"
pass "all 5 IntentLabel variants present (m14.1)"

# 4. classify_message_intent command registered (m14.1)
grep -q "fn classify_message_intent" "$COMMANDS_RS" || fail "classify_message_intent command missing from commands.rs"
grep -q "classify_message_intent" "$MAIN_RS" || fail "classify_message_intent not registered in main.rs"
grep -q "mod classifier" "$MAIN_RS" || fail "mod classifier not declared in main.rs"
pass "classify_message_intent command registered (m14.1)"

# 5. eval set present and has 40 entries (m14.2)
test -f "$EVAL_SET" || fail "intent_eval_set.json missing"
entry_count=$(python3 -c "import json,sys; data=json.load(open('$EVAL_SET')); print(len(data))" 2>/dev/null || echo "0")
[ "$entry_count" -eq 40 ] || fail "intent_eval_set.json should have 40 entries, got $entry_count"
pass "eval set present with 40 labeled examples (m14.2)"

# 6. eval set covers all required intent categories
python3 -c "
import json, sys
data = json.load(open('$EVAL_SET'))
intents = [d['expected_intent'] for d in data]
for required in ['answer', 'revision', 'subtask', 'suggestion', 'unknown']:
    count = intents.count(required)
    if count < 5:
        print(f'expected at least 5 {required} examples, got {count}', file=sys.stderr)
        sys.exit(1)
" || fail "eval set does not cover all required intent categories"
pass "eval set covers answer/revision/subtask/suggestion/unknown categories (m14.2)"

# 7. integration test harness present (m14.2)
test -f "$EVAL_HARNESS" || fail "tests/intent_eval.rs integration test harness missing"
grep -q "OPENAI_API_KEY" "$EVAL_HARNESS" || fail "eval harness does not gate on OPENAI_API_KEY"
grep -q "0.90" "$EVAL_HARNESS" || fail "90% accuracy threshold not enforced in eval harness"
grep -q "expected_slots" "$EVAL_HARNESS" || fail "eval harness does not validate expected slots"
grep -q "slot accuracy" "$EVAL_HARNESS" || fail "eval harness does not print slot accuracy"
if ! grep -q "CLASSIFIER_BUDGET_MS" "$EVAL_HARNESS" && ! grep -q "p50 < 150" "$EVAL_HARNESS"; then
  fail "eval harness does not enforce the classifier p50 budget"
fi
pass "eval harness covers accuracy, slot quality, and latency budget (m14.2)"

# 8. frontend: classifier function and fallback (m14.3)
grep -q "classifyMessageIntentWithFallback" "$APP_TSX" || fail "classifyMessageIntentWithFallback missing from App.tsx"
grep -q "inferMessageIntentKeyword" "$APP_TSX" || fail "inferMessageIntentKeyword (renamed keyword router) missing from App.tsx"
grep -q "intent_classifier_fallback" "$APP_TSX" || fail "intent_classifier_fallback console.warn missing from App.tsx"
grep -q "intent_classifier_timeout" "$APP_TSX" || fail "intent classifier timeout path missing from App.tsx"
pass "classifier integration with keyword fallback present in App.tsx (m14.3)"

# 9. explicit timeout budget configured on frontend and backend classifier
# apex a1: the backend timeout constant moved to model_router.rs with the
# rest of the classification dispatch (CLASSIFY_TIMEOUT_OPENAI_MS).
grep -q "INTENT_CLASSIFIER_TIMEOUT_MS" "$APP_TSX" || fail "frontend classifier timeout constant missing"
grep -q "CLASSIFY_TIMEOUT_OPENAI_MS" "$MODEL_ROUTER_RS" || fail "backend classifier timeout constant missing"
pass "frontend and backend timeout budgets are explicitly configured"

# 10. unknown-intent clarify path is explicit
grep -q "intent === \"unknown\"" "$APP_TSX" || fail "unknown-intent branch missing in App.tsx"
grep -q "Jeff needs clarification" "$APP_TSX" || fail "clarify prompt missing for unknown intent"
pass "unknown-intent clarify path present"

# 11. slots passed downstream to revision and subtask handlers
grep -q "slots.*IntentSlotsDto" "$APP_TSX" || fail "IntentSlotsDto slots param missing from revision/subtask handler signatures"
grep -q "slots?.instruction" "$APP_TSX" || fail "slots.instruction not used in revision handler"
grep -q "slots?.draft_type" "$APP_TSX" || fail "slots.draft_type not used in subtask handler"
grep -q "slots?.target_description" "$APP_TSX" || fail "slots.target_description is not used for revision targeting"
pass "slots passed downstream to revision/subtask handlers including target_description"

# 12. frontend TypeScript types and wrapper (m14.3)
grep -q "IntentLabel" "$TAURI_CLIENT_TS" || fail "IntentLabel type missing from tauriClient.ts"
grep -q "IntentSlotsDto" "$TAURI_CLIENT_TS" || fail "IntentSlotsDto interface missing from tauriClient.ts"
grep -q "IntentClassificationDto" "$TAURI_CLIENT_TS" || fail "IntentClassificationDto interface missing from tauriClient.ts"
grep -q "classifyMessageIntent" "$TAURI_CLIENT_TS" || fail "classifyMessageIntent wrapper missing from tauriClient.ts"
pass "frontend TypeScript types and wrapper present (m14.3)"

# 13. full build + test suite
cd "$ROOT_DIR/desktop"
npm run lint
npm run test
cargo build --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml classifier
pass "build and test suite passed"

# 14. live eval (optional — only when OPENAI_API_KEY is set)
if [ -n "${OPENAI_API_KEY:-}" ]; then
  echo "OPENAI_API_KEY detected — running live eval harness"
  if ! eval_output=$(cargo test --manifest-path src-tauri/Cargo.toml --test intent_eval -- --nocapture 2>&1); then
    echo "$eval_output"
    fail "live eval harness failed"
  fi
  echo "$eval_output"
  p50_ms=$(echo "$eval_output" | sed -n 's/.*p50=\([0-9][0-9]*\)ms.*/\1/p' | tail -1)
  [ -n "$p50_ms" ] || fail "could not parse p50 latency from live eval output"
  case "$p50_ms" in
    ''|*[!0-9]*)
      fail "parsed p50 latency is not numeric: '$p50_ms'"
      ;;
  esac
  pass "live eval passed 90% accuracy threshold"
  pass "live eval produced numeric p50 latency (${p50_ms}ms)"
else
  echo "SKIP: OPENAI_API_KEY not set — skipping live eval (set it to run accuracy check)"
fi

echo ""
echo "phase 14 checks passed"
