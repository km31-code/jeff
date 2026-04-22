#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
PROVIDERS_RS="$ROOT_DIR/desktop/src-tauri/src/providers.rs"
ERRORS_RS="$ROOT_DIR/desktop/src-tauri/src/errors.rs"
APP_TSX="$ROOT_DIR/desktop/src/App.tsx"
LATENCY_RS="$ROOT_DIR/desktop/src-tauri/src/latency.rs"

echo "--- phase 17 reliability + productization gate check ---"

fail() { echo "FAIL: $1" >&2; exit 1; }
pass() { echo "PASS: $1"; }

# 1. providers.rs contains all 5 trait definitions
for trait_name in \
  SpeechToTextProvider \
  TextToSpeechProvider \
  ReasoningModelProvider \
  EmbeddingsProvider \
  ClassifierProvider; do
  grep -q "trait ${trait_name}" "$PROVIDERS_RS" || \
    fail "missing trait definition in providers.rs: ${trait_name}"
done
pass "providers.rs contains all 5 provider trait definitions"

# 2. providers.rs contains all 5 OpenAI provider structs
for struct_name in \
  OpenAiReasoningProvider \
  OpenAiSttProvider \
  OpenAiTtsProvider \
  OpenAiEmbeddingsProvider \
  OpenAiClassifierProvider; do
  grep -q "struct ${struct_name}" "$PROVIDERS_RS" || \
    fail "missing OpenAI provider struct in providers.rs: ${struct_name}"
done
pass "providers.rs contains all 5 OpenAI provider structs"

# 3. allow(dead_code) suppression is removed from providers.rs
if grep -q "#!\\[allow(dead_code)\\]" "$PROVIDERS_RS"; then
  fail "providers.rs still contains #![allow(dead_code)]"
fi
pass "providers.rs no longer suppresses dead_code"

# 4. errors.rs exists and declares JeffError
test -f "$ERRORS_RS" || fail "errors.rs is missing"
grep -q "enum JeffError" "$ERRORS_RS" || fail "JeffError enum missing from errors.rs"
pass "errors.rs exists and declares JeffError"

# 5. all four required JeffError variants are present
for variant in ApiTimeout InvalidApiKey MissingOsPermission DbLockContention; do
  grep -q "$variant" "$ERRORS_RS" || fail "errors.rs missing JeffError variant: $variant"
done
pass "errors.rs contains all four required JeffError variants"

# 6. frontend error branch is wired for phase 17 messaging
grep -q "jeff-error-banner" "$APP_TSX" || fail "App.tsx missing jeff-error-banner test id"
grep -q "mapJeffErrorMessage" "$APP_TSX" || fail "App.tsx missing Jeff error mapping function"
pass "frontend error mapping branch is present in App.tsx"

# 7. latency.rs exists and contains all 4 budget constants
test -f "$LATENCY_RS" || fail "latency.rs is missing"
for constant_name in \
  STARTUP_BUDGET_MS \
  FIRST_TOKEN_BUDGET_MS \
  FIRST_AUDIO_BUDGET_MS \
  CLASSIFIER_BUDGET_MS; do
  grep -q "$constant_name" "$LATENCY_RS" || fail "latency.rs missing constant: $constant_name"
done
pass "latency.rs contains all 4 budget constants"

# 8. startup budget test passes
cargo test --manifest-path "$ROOT_DIR/desktop/src-tauri/Cargo.toml" \
  startup_budget_is_met -- --test-threads=1
pass "startup_budget_is_met test passes"

# 9. optional live intent eval budget assertion
if [ -n "${OPENAI_API_KEY:-}" ]; then
  if ! eval_output=$(cargo test --manifest-path "$ROOT_DIR/desktop/src-tauri/Cargo.toml" \
    --test intent_eval -- --nocapture 2>&1); then
    echo "$eval_output"
    fail "intent_eval live harness failed"
  fi
  echo "$eval_output"

  p50_ms=$(echo "$eval_output" | sed -n 's/.*p50=\([0-9][0-9]*\)ms.*/\1/p' | tail -1)
  [ -n "$p50_ms" ] || fail "could not parse p50 latency from intent_eval output"
  case "$p50_ms" in
    ''|*[!0-9]*)
      fail "parsed p50 latency is not numeric: '$p50_ms'"
      ;;
  esac
  [ "$p50_ms" -lt 150 ] || fail "intent_eval p50 ${p50_ms}ms exceeds 150ms budget"
  pass "intent_eval live harness reports p50 < 150ms (${p50_ms}ms)"
else
  echo "SKIP: OPENAI_API_KEY not set — skipping live intent_eval latency assertion"
fi

# 10-15. regression gate: phase 11-16 scripts must pass
"$ROOT_DIR/scripts/phase11_check.sh"
pass "phase11_check.sh passed"

"$ROOT_DIR/scripts/phase12_check.sh"
pass "phase12_check.sh passed"

"$ROOT_DIR/scripts/phase13_check.sh"
pass "phase13_check.sh passed"

"$ROOT_DIR/scripts/phase14_check.sh"
pass "phase14_check.sh passed"

"$ROOT_DIR/scripts/phase15_check.sh"
pass "phase15_check.sh passed"

"$ROOT_DIR/scripts/phase16_check.sh"
pass "phase16_check.sh passed"

echo ""
echo "phase 17 checks passed"
