#!/usr/bin/env bash
# apex e6 check: onboarding v2 + bundled inference (no-key path).

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
ONBOARD="$SRC/onboarding.rs"
COMMANDS="$SRC/commands.rs"
MAIN="$SRC/main.rs"
OVERLAY="$DESKTOP/src/Overlay.tsx"
TAURI_CLIENT="$DESKTOP/src/tauriClient.ts"
RELAY="$ROOT_DIR/relay/server.mjs"
RELAY_TEST="$ROOT_DIR/relay/server.test.mjs"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex e6 onboarding + bundled inference check ---"

# 1. Inference choice: bundled vs byok.
grep -q "INFERENCE_MODE_BUNDLED" "$ONBOARD" || fail "bundled inference mode missing"
grep -q "INFERENCE_MODE_BYOK" "$ONBOARD" || fail "byok inference mode missing"
grep -q "pub fn get_inference_mode" "$ONBOARD" || fail "get_inference_mode missing"
grep -q "pub fn set_inference_mode" "$ONBOARD" || fail "set_inference_mode missing"
grep -q "pub fn onboarding_ready" "$ONBOARD" || fail "onboarding readiness gate missing"
grep -q "pub inference_mode: String" "$SRC/models.rs" || fail "onboarding status lacks inference_mode"
pass "inference choice (bundled/byok) and readiness gate present"

# 2. Bundled path completes onboarding without a key; guard is wired.
grep -q "onboarding_ready(&state.store" "$COMMANDS" || fail "complete_onboarding is not gated by readiness"
grep -q "pub fn set_inference_mode" "$COMMANDS" || fail "set_inference_mode command missing"
grep -q "commands::set_inference_mode" "$MAIN" || fail "set_inference_mode not registered"
grep -q "ProviderKind::Bundled" "$SRC/model_router.rs" || fail "bundled provider is not routed"
grep -q "openai_generate_blocking_with_credentials" "$SRC/model_router.rs" || fail "bundled provider does not dispatch to relay"
grep -q "configure_bundled_inference" "$COMMANDS" || fail "bundled token provisioning command missing"
grep -q "commands::configure_bundled_inference" "$MAIN" || fail "bundled token provisioning command not registered"
grep -q "resolve_bundled_inference_token" "$SRC/secrets.rs" || fail "scoped relay token is not read from keychain"
grep -q "BUNDLED_TOKEN_EXPIRES_AT_KEY" "$ONBOARD" || fail "bundled token expiry is not tracked"
pass "bundled path provisions a scoped token and routes through a real provider"

# 3. The relay enforces scoped, expiring tokens, quota, server-side model pinning,
# and server-only upstream credentials.
test -f "$RELAY" || fail "bundled relay service missing"
test -f "$RELAY_TEST" || fail "bundled relay tests missing"
grep -q "inference:chat" "$RELAY" || fail "relay scope enforcement missing"
grep -q "TOKEN_REQUEST_LIMIT" "$RELAY" || fail "relay request quota missing"
grep -q "timingSafeEqual" "$RELAY" || fail "relay token signature verification missing"
grep -q "model: upstreamModel" "$RELAY" || fail "relay does not pin the upstream model"
RELAY_TEST_OUT=$(cd "$ROOT_DIR/relay" && npm test 2>&1)
echo "$RELAY_TEST_OUT" | grep -q "# pass 2" || { echo "$RELAY_TEST_OUT"; fail "bundled relay tests failed"; }
echo "$RELAY_TEST_OUT" | grep -q "# fail 0" || { echo "$RELAY_TEST_OUT"; fail "bundled relay tests failed"; }
pass "metered relay issues scoped tokens and rejects unauthorized calls"

# 4. Behavioral: bundled completes without a key and the provider uses the
# configured endpoint/token rather than the user's OpenAI key.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

grep -q "fn e6_bundled_inference_completes_onboarding_without_a_key" "$ONBOARD" || fail "e6 onboarding test missing"
grep -q "fn e6_bundled_provider_uses_scoped_endpoint_and_reports_usage" "$SRC/providers.rs" || fail "e6 provider integration test missing"
grep -q "fn e6_bundled_mode_routes_every_cloud_tier_through_relay_provider" "$SRC/model_router.rs" || fail "e6 router test missing"
E6_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test e6_ --quiet 2>&1)
echo "$E6_TEST_OUT" | grep -q "test result: ok" || { echo "$E6_TEST_OUT"; fail "e6 tests failed"; }
echo "$E6_TEST_OUT" | grep -q "FAILED" && { echo "$E6_TEST_OUT"; fail "e6 tests failed"; }
pass "e6 bundled-no-key onboarding test passes"

# 5. Onboarding wizard exposes and provisions the bundled choice.
grep -q "setInferenceMode" "$TAURI_CLIENT" || fail "frontend setInferenceMode binding missing"
grep -q "configureBundledInference" "$TAURI_CLIENT" || fail "frontend bundled provisioning binding missing"
grep -q "handleConfigureBundledInference" "$OVERLAY" || fail "onboarding bundled button does not provision a token"
grep -q "onboarding-inference-bundled" "$OVERLAY" || fail "onboarding bundled-inference control missing"
grep -q "onboarding-step-1" "$OVERLAY" || fail "onboarding wizard steps missing"
pass "onboarding wizard exposes the bundled (no-key) inference choice"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend lint did not run"; }
pass "frontend TypeScript check passes"

FRONTEND_TEST_OUT=$(cd "$DESKTOP" && npm test -- --run 2>&1)
echo "$FRONTEND_TEST_OUT" | grep -qE "Test Files.*passed" || { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
echo "$FRONTEND_TEST_OUT" | grep -qE "[0-9]+ failed" && { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
pass "frontend tests pass"

if [ "${JEFF_SKIP_ADJACENT_GATES:-0}" != "1" ]; then
  E5_OUT=$(JEFF_SKIP_ADJACENT_GATES=1 bash "$ROOT_DIR/scripts/apex_e5_check.sh" 2>&1) || {
    echo "$E5_OUT"
    fail "apex e5 drive gate regressed"
  }
  echo "$E5_OUT"
  pass "apex e5 drive gate still passes"
fi

echo "--- apex e6 check passed ---"
