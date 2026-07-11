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
grep -q "BUNDLED_PROVIDER_LABEL" "$SRC/model_router.rs" || fail "bundled provider seam missing in router"
pass "bundled path completes onboarding with no key; router bundled seam present"

# 3. Behavioral: bundled completes without a key.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

grep -q "fn e6_bundled_inference_completes_onboarding_without_a_key" "$ONBOARD" || fail "e6 test missing"
E6_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test e6_ --quiet 2>&1)
echo "$E6_TEST_OUT" | grep -q "test result: ok" || { echo "$E6_TEST_OUT"; fail "e6 tests failed"; }
echo "$E6_TEST_OUT" | grep -q "FAILED" && { echo "$E6_TEST_OUT"; fail "e6 tests failed"; }
pass "e6 bundled-no-key onboarding test passes"

# 4. Onboarding wizard exposes the bundled choice.
grep -q "setInferenceMode" "$TAURI_CLIENT" || fail "frontend setInferenceMode binding missing"
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

bash "$ROOT_DIR/scripts/apex_e5_check.sh" >/dev/null 2>&1 || fail "apex e5 drive gate regressed"
pass "apex e5 drive gate still passes"

echo "--- apex e6 check passed ---"
