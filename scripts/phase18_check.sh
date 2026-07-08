#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
COMMANDS_RS="$ROOT_DIR/desktop/src-tauri/src/commands.rs"
MAIN_RS="$ROOT_DIR/desktop/src-tauri/src/main.rs"
ONBOARDING_RS="$ROOT_DIR/desktop/src-tauri/src/onboarding.rs"
SECRETS_RS="$ROOT_DIR/desktop/src-tauri/src/secrets.rs"
PROVIDERS_RS="$ROOT_DIR/desktop/src-tauri/src/providers.rs"
REASONING_RS="$ROOT_DIR/desktop/src-tauri/src/reasoning.rs"
CHAT_STREAMING_RS="$ROOT_DIR/desktop/src-tauri/src/chat_streaming.rs"
OVERLAY_TSX="$ROOT_DIR/desktop/src/Overlay.tsx"
APP_TSX="$ROOT_DIR/desktop/src/App.tsx"
AMBIENT_RS="$ROOT_DIR/desktop/src-tauri/src/ambient.rs"
TAURI_CLIENT_TS="$ROOT_DIR/desktop/src/tauriClient.ts"
AMBIENT_CLIENT_TS="$ROOT_DIR/desktop/src/ambientClient.ts"

fail() { echo "FAIL: $1" >&2; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- phase 18 onboarding + secure key management check ---"

for symbol in \
  get_onboarding_status \
  complete_onboarding \
  set_preferred_workspace_folder \
  clear_preferred_workspace_folder \
  validate_openai_api_key \
  store_openai_api_key \
  delete_openai_api_key \
  get_workspace_prompt_dismissed \
  set_workspace_prompt_dismissed; do
  grep -q "fn ${symbol}" "$COMMANDS_RS" || fail "missing command function: ${symbol}"
  grep -q "commands::${symbol}" "$MAIN_RS" || fail "missing invoke registration: ${symbol}"
done
pass "onboarding/key commands exist and are registered"

for key in \
  APP_SETTING_ONBOARDING_COMPLETE \
  APP_SETTING_PREFERRED_WORKSPACE_FOLDER \
  APP_SETTING_ONBOARDING_LAST_COMPLETED_AT \
  APP_SETTING_WORKSPACE_PROMPT_DISMISSED; do
  grep -q "$key" "$ONBOARDING_RS" || fail "missing onboarding app-setting key constant: $key"
done
pass "onboarding app-setting key constants are present"

for symbol in \
  resolve_openai_api_key \
  resolve_openai_api_key_required \
  store_openai_api_key \
  delete_openai_api_key \
  OPENAI_KEYCHAIN_SERVICE \
  OPENAI_KEYCHAIN_ACCOUNT; do
  grep -q "$symbol" "$SECRETS_RS" || fail "missing secrets symbol: ${symbol}"
done
pass "secrets resolver and keychain store symbols are present"

# apex a1: the classify path delegates key resolution to the model router,
# whose provider adapters use the unified resolver (asserted on providers.rs).
grep -q "resolve_openai_api_key_required" "$PROVIDERS_RS" || fail "providers.rs does not use unified key resolver"
grep -q "model_router.classify" "$COMMANDS_RS" || fail "commands.rs classify path does not route through model router"
grep -q "resolve_openai_api_key().api_key" "$CHAT_STREAMING_RS" || fail "chat_streaming.rs tts path does not use unified key resolver"
pass "runtime call paths read OpenAI key via unified resolver"

if grep -q 'std::env::var("OPENAI_API_KEY")' "$PROVIDERS_RS" "$REASONING_RS" "$CHAT_STREAMING_RS"; then
  fail "direct OPENAI_API_KEY env reads remain in migrated runtime call paths"
fi
pass "direct env reads were removed from migrated runtime call paths"

for step_id in onboarding-step-1 onboarding-step-2 onboarding-step-3 onboarding-step-4; do
  grep -q "$step_id" "$OVERLAY_TSX" || fail "overlay missing onboarding step marker: ${step_id}"
done
pass "overlay contains 4-step onboarding wizard markers"

grep -q "Tell me what you're working on." "$APP_TSX" || fail "App.tsx missing no-active-task prompt copy"
grep -q "overlay-no-active-task" "$OVERLAY_TSX" || fail "Overlay.tsx missing no-active-task prompt branch"
pass "no-active-task empty-state copy is present"

grep -q "Update API key" "$APP_TSX" || fail "App.tsx missing API-key recovery CTA"
grep -q "overlay-fix-api-key" "$OVERLAY_TSX" || fail "Overlay.tsx missing API-key recovery CTA"
pass "API-key recovery CTAs are present"

grep -q "companion-workspace-soft-prompt" "$APP_TSX" || fail "App.tsx missing workspace soft-prompt branch"
pass "workspace folder soft prompt is present"

grep -q '"tray:setup"' "$AMBIENT_RS" || fail "ambient tray missing setup menu id"
grep -q "Set up Jeff again" "$AMBIENT_RS" || fail "ambient tray missing setup label"
grep -q "ambient://open-onboarding" "$AMBIENT_RS" || fail "ambient onboarding event bridge missing"
grep -q "open_onboarding_flow_at_step" "$AMBIENT_RS" || fail "ambient.rs missing step-aware onboarding function"
grep -q "fn ambient_open_onboarding_at_step" "$AMBIENT_RS" || fail "ambient.rs missing ambient_open_onboarding_at_step command"
pass "tray setup re-run onboarding flow is wired with step support"

# step-aware onboarding command is registered in main.rs
grep -q "ambient_open_onboarding_at_step" "$MAIN_RS" || fail "main.rs missing ambient_open_onboarding_at_step registration"
pass "step-aware onboarding command is registered"

grep -q "getOnboardingStatus" "$TAURI_CLIENT_TS" || fail "tauriClient.ts missing onboarding status wrapper"
grep -q "storeOpenAiApiKey" "$TAURI_CLIENT_TS" || fail "tauriClient.ts missing key store wrapper"
grep -q "completeOnboarding" "$TAURI_CLIENT_TS" || fail "tauriClient.ts missing complete onboarding wrapper"
grep -q "getWorkspacePromptDismissed" "$TAURI_CLIENT_TS" || fail "tauriClient.ts missing workspace prompt dismissed getter"
grep -q "setWorkspacePromptDismissed" "$TAURI_CLIENT_TS" || fail "tauriClient.ts missing workspace prompt dismissed setter"
pass "frontend IPC wrappers for onboarding are present"

# step-aware onboarding IPC is present in ambientClient
grep -q "openOnboardingAtStep" "$AMBIENT_CLIENT_TS" || fail "ambientClient.ts missing openOnboardingAtStep wrapper"
pass "ambientClient.ts step-aware onboarding IPC wrapper is present"

(
  cd "$ROOT_DIR/desktop"
  npm run lint
  npm run test
)
pass "frontend lint and tests passed"

cargo build --manifest-path "$ROOT_DIR/desktop/src-tauri/Cargo.toml"
cargo test --manifest-path "$ROOT_DIR/desktop/src-tauri/Cargo.toml" -- --test-threads=1
pass "backend build and tests passed"

echo ""
echo "phase 18 checks passed"
