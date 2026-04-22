# Phase 18: First-Run Onboarding + Secure Key Management — Implementation Plan

**Status:** completed  
**Phase ref:** docs/PHASES_NEXT.md § Phase 18  
**Architecture ref:** docs/ARCHITECTURE.md (Phase 14/16 baseline)  
**Exit criteria location:** docs/PHASES_NEXT.md § Phase 18 → Exit criteria

This document is the authoritative tracker for Phase 18. It is written against the
current codebase (Phase 17 complete) and translates the Phase 18 product contract into
sequenced, testable implementation work.

Per request: implementation does **not** start until this plan is explicitly approved.

---

## 0) Deep-read summary (why this plan looks this way)

### What Phase 18 must deliver (from PHASES_NEXT)
- First-run onboarding inside the overlay (4 steps).
- API key setup validated in-app and stored in macOS Keychain.
- `.env` remains dev fallback; onboarding does not read/write `.env`.
- Wizard cancellable at any step; completion sets `onboarding_complete=true`.
- Tray entry "Set up Jeff again" re-runs onboarding.
- Companion empty/error states:
  - No active task → "Tell me what you're working on." + input.
  - API key invalid → actionable recovery path from UI.
  - No workspace folder set → soft prompt, app still functional.

### Current architecture constraints (from ARCHITECTURE + code)
- Runtime is tray-resident; main window hidden at startup (`main.rs`, `ambient.rs`).
- Overlay (`Overlay.tsx`) is the always-there ambient surface; ideal place for onboarding.
- Core OpenAI calls still read `OPENAI_API_KEY` directly in multiple paths:
  - `providers.rs` (`read_openai_api_key_from_env`)
  - `reasoning.rs` (`OpenAiStreamingReasoningProvider::from_env`)
  - `chat_streaming.rs` (`tts_api_key` from env)
  - `commands.rs` `classify_message_intent` (env read)
- `app_settings` table exists with generic helpers in `store.rs`.
- No onboarding model, commands, UI, or phase18 check script exist yet.

### Core implementation implication
Phase 18 is not only UI wizard work. It requires introducing a key-management seam that
all OpenAI call paths use, otherwise onboarding cannot actually unblock users.

---

## Milestone order

```text
M18.1 (contracts + persistence) → M18.2 (secure key manager + provider wiring)
→ M18.3 (overlay onboarding wizard) → M18.4 (companion empty/error state UX)
→ M18.5 (tray re-run + lifecycle glue) → M18.6 (phase18_check.sh + validation)
```

Why this order:
- M18.1 defines persistent state and command contracts used by all later UI.
- M18.2 makes key setup real (not cosmetic) by moving runtime reads off raw env lookups.
- M18.3 and M18.4 then become deterministic frontend wiring tasks.
- M18.5 stitches re-entry behavior into tray/ambient lifecycle.
- M18.6 is final gate consolidation.

---

## M18.1: Onboarding contracts + app setting keys

**Status:** completed

**Goal:** Introduce explicit onboarding state and API contracts without changing runtime
behavior yet.

### Current state
- `app_settings` exists but no onboarding keys or typed onboarding DTOs.
- No Tauri commands for onboarding status, completion, or workspace preference.

### What to implement

1. Add typed Phase 18 DTOs in `models.rs`:
- `OnboardingStatusDto`
  - `onboarding_complete: bool`
  - `has_stored_api_key: bool`
  - `api_key_source: "keychain" | "env" | "none"`
  - `preferred_workspace_folder: Option<String>`
- `ApiKeyValidationDto`
  - `is_valid: bool`
  - `message: String`

2. Add app-setting key constants in a new module `onboarding.rs` (or `settings_keys.rs`):
- `onboarding_complete`
- `preferred_workspace_folder`
- optional `onboarding_last_completed_at` (for observability only)

3. Add store helper methods (thin wrappers around existing generic settings):
- `get_onboarding_complete() -> Result<bool>`
- `set_onboarding_complete(bool) -> Result<()>`
- `get_preferred_workspace_folder() -> Result<Option<String>>`
- `set_preferred_workspace_folder(Option<&str>) -> Result<()>`

4. Add command contract skeletons in `commands.rs`:
- `get_onboarding_status`
- `complete_onboarding`
- `set_preferred_workspace_folder`
- `clear_preferred_workspace_folder`
- `validate_openai_api_key` (validation only for now; persistence done in M18.2)

5. Register new commands in `main.rs` invoke handler.

### Constraints
- No behavioral changes yet in chat/streaming paths.
- No wizard UI yet.
- API key is still env-backed until M18.2 lands.

### Files touched
- `desktop/src-tauri/src/models.rs`
- `desktop/src-tauri/src/store.rs`
- `desktop/src-tauri/src/commands.rs`
- `desktop/src-tauri/src/main.rs`
- `desktop/src-tauri/src/onboarding.rs` (new)

### Verification
- `cargo build` passes.
- `cargo test -- --test-threads=1` passes.
- `rg` confirms onboarding commands are registered in `main.rs`.

---

## M18.2: Secure key manager + OpenAI path migration

**Status:** completed

**Goal:** Make key setup actually drive runtime behavior by centralizing key resolution and
switching OpenAI callers to it.

### Current state
- OpenAI key is read directly from env in multiple call paths.
- No keychain dependency in Tauri backend.

### What to implement

1. Add keychain integration dependency and runtime plugin registration:
- Add `tauri-plugin-keychain` (or official current package name) in
  `desktop/src-tauri/Cargo.toml`.
- Register plugin in `main.rs`.
- Update capability permissions in `desktop/src-tauri/capabilities/default.json`.

2. Add `desktop/src-tauri/src/secrets.rs` (new):
- `OpenAiKeyStore` interface + concrete keychain-backed implementation.
- Fixed service/account identifiers:
  - service: `com.jeff.desktop`
  - account: `openai_api_key`
- Operations:
  - `set_openai_api_key(key: &str)`
  - `get_openai_api_key() -> Option<String>`
  - `delete_openai_api_key()`

3. Add unified resolver function (single source of truth):
- `resolve_openai_api_key()` priority:
  1. keychain value
  2. env fallback (`OPENAI_API_KEY`) for dev-only compatibility
- Return metadata for onboarding status: source keychain/env/none.

4. Wire all OpenAI call paths to resolver (remove direct env reads):
- `providers.rs` provider impls.
- `reasoning.rs` streaming provider.
- `chat_streaming.rs` TTS key lookup.
- `commands.rs` classifier key lookup.

5. Add commands for key persistence/recovery path:
- `store_openai_api_key(api_key: String)`
- `delete_openai_api_key()`
- `get_onboarding_status` reports `has_stored_api_key` and source.

6. Add key validation command behavior:
- `validate_openai_api_key(api_key)` performs bounded API check (timeout) and returns
  `ApiKeyValidationDto`.
- Does not persist on failure.

### Constraints
- `.env` fallback remains for developers.
- Onboarding flow never writes `.env`.
- Error messages must continue mapping through Phase 17 `JeffError` handling.

### Files touched
- `desktop/src-tauri/Cargo.toml`
- `desktop/src-tauri/src/main.rs`
- `desktop/src-tauri/capabilities/default.json`
- `desktop/src-tauri/src/secrets.rs` (new)
- `desktop/src-tauri/src/providers.rs`
- `desktop/src-tauri/src/reasoning.rs`
- `desktop/src-tauri/src/chat_streaming.rs`
- `desktop/src-tauri/src/commands.rs`
- `desktop/src-tauri/src/lib.rs` (expose module for tests if needed)

### Verification
- `cargo build` passes with keychain plugin enabled.
- Unit tests for resolver precedence pass:
  - keychain present → uses keychain.
  - keychain absent + env set → uses env.
  - both absent → invalid-key path.
- Existing phase17 key error mapping still works in UI (`API_KEY_MESSAGE` branch).

---

## M18.3: Overlay first-run onboarding wizard (4 steps)

**Status:** completed

**Goal:** Implement the required first-run wizard entirely inside overlay UX.

### Current state
- Overlay has chat UI only; no onboarding state machine.
- No file picker usage.

### What to implement

1. Add frontend onboarding types + wrappers:
- In `tauriClient.ts`:
  - `OnboardingStatusDto`, `ApiKeyValidationDto`
  - command wrappers for onboarding and key commands from M18.1/M18.2.

2. Add dialog plugin for directory picker (if not already installed):
- Add `@tauri-apps/plugin-dialog` in `desktop/package.json`.
- Register corresponding Rust plugin + capability permissions.

3. Implement overlay onboarding state machine in `Overlay.tsx`:
- Entry condition: `!onboarding_complete` from `get_onboarding_status`.
- Step container rendered in expanded overlay mode.
- Steps (exactly 4):
  1. What Jeff is (3-sentence copy + continue CTA)
  2. API key setup
     - input
     - `validate_openai_api_key`
     - on success `store_openai_api_key`
  3. Workspace folder
     - `Choose folder` via dialog
     - optional skip path
     - persist preferred folder setting
  4. Ready
     - show hotkey from ambient state
     - CTA "Start with your first message"
     - completion triggers `complete_onboarding`

4. Cancellable flow:
- `Cancel` control available on all steps.
- Cancel hides wizard but does not set `onboarding_complete`.

5. Onboarding state UX polish:
- loading spinners for validation/persistence calls.
- explicit success/failure copy on key validation step.

### Constraints
- No new window; wizard stays in overlay.
- No `.env` read/write from wizard code.
- Step count fixed at 4 to match phase contract and check script gate.

### Files touched
- `desktop/src/Overlay.tsx`
- `desktop/src/tauriClient.ts`
- `desktop/src/styles.css`
- `desktop/package.json`
- `desktop/src-tauri/src/main.rs` (dialog plugin registration if needed)
- `desktop/src-tauri/capabilities/default.json`

### Verification
- Overlay renders 4-step wizard when onboarding incomplete.
- Successful step flow sets `onboarding_complete=true`.
- Cancel exits wizard without completing onboarding.
- Folder step supports choose + skip.

---

## M18.4: Companion empty/error state UX completion

**Status:** completed

**Goal:** Satisfy Phase 18 UX requirements in companion surfaces (not only overlay wizard).

### Current state
- No-active-task handling exists but is split between home/workspace/overlay and not
  productized to required copy/flow.
- API key error message exists but no direct recovery CTA.
- Workspace folder soft prompt is not explicit.

### What to implement

1. No active task state in companion UX:
- Add explicit empty-state card with required copy:
  - "Tell me what you're working on."
- Add text input + CTA that:
  - creates a task from prompt-derived title,
  - sets it active,
  - routes the same input through normal message path.
- Ensure no blank/blocked state in either `App.tsx` companion or `Overlay.tsx`.

2. API key invalid recovery:
- Add actionable CTA near `jeff-error-banner` when message maps to invalid key:
  - "Update API key"
  - opens onboarding at step 2 (or dedicated key settings panel).
- Keep existing friendly message from Phase 17.

3. No workspace folder soft prompt:
- If preferred folder is missing and watcher inactive, show a non-blocking card:
  - explains optional folder setup,
  - offers "Choose folder" and "Skip for now".
- App remains fully usable without folder setup.

4. Onboarding-aware startup behavior:
- If onboarding incomplete and overlay opens, force expanded mode and wizard-first view.

### Constraints
- These states must be non-blocking.
- Must not regress current companion chat workflow.

### Files touched
- `desktop/src/App.tsx`
- `desktop/src/Overlay.tsx`
- `desktop/src/styles.css`
- `desktop/src/tauriClient.ts`

### Verification
- Manual path: no task → prompt shown → can create + send first message.
- Invalid key error path offers direct key update CTA and recovers without terminal.
- Missing workspace folder path shows soft prompt but interaction remains available.

---

## M18.5: Tray re-run onboarding + ambient lifecycle glue

**Status:** completed

**Goal:** Add explicit "Set up Jeff again" entry in tray and reconnect it to overlay wizard.

### Current state
- Tray menu has Show / Open Full Workspace / Quiet / Quit only.

### What to implement

1. Add tray item in `ambient.rs`:
- Menu id: `tray:setup`
- Label: `Set up Jeff again`

2. Event bridge:
- On click:
  - show overlay
  - set overlay mode expanded
  - emit `ambient://open-onboarding` event

3. Overlay handling:
- Listen for `ambient://open-onboarding` and enter wizard at step 1.

4. Keep existing tray behavior unchanged for other items.

### Constraints
- No focus steal regression from Phase 11.
- No changes to quit behavior.

### Files touched
- `desktop/src-tauri/src/ambient.rs`
- `desktop/src/Overlay.tsx`
- `scripts/phase11_check.sh` (only if tray symbol checks need extension)

### Verification
- Tray menu shows "Set up Jeff again".
- Selecting it always opens onboarding wizard regardless of completion state.

---

## M18.6: Phase 18 validation gate (`phase18_check.sh`) + test coverage

**Status:** completed

**Goal:** Codify Phase 18 exit criteria in a single script and supporting tests.

### What to implement

1. Add `scripts/phase18_check.sh` with ordered checks:
- backend:
  - onboarding commands exist and are registered.
  - `onboarding_complete` app-setting path exists.
  - keychain set/get/delete path exists.
  - unified key resolver exists and env-only direct reads removed from target call paths.
- frontend:
  - overlay wizard has 4 steps (symbol + test id checks).
  - no-active-task companion copy exists.
  - invalid-key recovery CTA exists.
  - workspace-folder soft prompt exists.
  - tray setup event handler exists.
- runtime:
  - `npm run lint`
  - `npm run test`
  - `cargo build --manifest-path desktop/src-tauri/Cargo.toml`
  - targeted cargo tests for onboarding/key resolver logic.

2. Add frontend tests:
- `Overlay` onboarding step progression, cancel behavior, and completion.
- no-active-task quickstart prompt branch.
- invalid-key CTA render branch.

3. Add backend tests:
- onboarding app-setting round-trip.
- key resolver precedence tests.

4. Update this plan file statuses/checklist when gates pass.

### Files touched
- `scripts/phase18_check.sh` (new)
- `desktop/src/App.test.tsx` and/or `desktop/src/Overlay.test.tsx` (new)
- `desktop/src-tauri/src/*` tests as needed
- `docs/PHASE18_PLAN.md` (status updates)

### Verification
- `bash scripts/phase18_check.sh` exits 0.

---

## Proposed command contract (Phase 18)

These are the concrete IPC additions the frontend will rely on:

- `get_onboarding_status() -> OnboardingStatusDto`
- `validate_openai_api_key(api_key: String) -> ApiKeyValidationDto`
- `store_openai_api_key(api_key: String) -> Result<(), String>`
- `delete_openai_api_key() -> Result<(), String>`
- `set_preferred_workspace_folder(folder_path: String) -> Result<(), String>`
- `clear_preferred_workspace_folder() -> Result<(), String>`
- `complete_onboarding() -> Result<(), String>`

Optional (if needed to simplify frontend flows):
- `begin_onboarding_reset()` to clear completion flag only.

---

## Data model additions (app_settings)

No new SQL table is required for Phase 18. Keys in `app_settings`:

- `onboarding_complete` = `"1" | "0"`
- `preferred_workspace_folder` = absolute path string (or absent)
- `onboarding_last_completed_at` (optional observability key)

The API key itself is **not** stored in SQLite.

---

## Key technical decisions locked by this plan

1. Overlay remains the onboarding host surface.
2. API key persistence is keychain-backed; SQLite stores only status metadata.
3. Runtime OpenAI key resolution is centralized and shared by all call paths.
4. `.env` remains fallback for developers only; onboarding never mutates env.
5. Companion empty/error states are part of Phase 18 completion, not deferred polish.

---

## Explicit non-goals for Phase 18

- No launch-at-login work (Phase 19).
- No active-window document title sensing (Phase 20).
- No privacy center dashboard (Phase 21).
- No selection capture or live app action paths (Phases 22–23).
- No release/distribution pipeline work (Phase 24).

---

## Exit criteria checklist (from PHASES_NEXT, translated to code gates)

- [x] M18.1 done — onboarding command/state contracts landed.
- [x] M18.2 done — keychain-backed key management wired across OpenAI paths.
- [x] M18.3 done — 4-step overlay wizard implemented and cancellable.
- [x] M18.4 done — required no-task / bad-key / no-folder UX branches implemented.
- [x] M18.5 done — tray "Set up Jeff again" re-runs wizard.
- [x] M18.6 done — `bash scripts/phase18_check.sh` exits 0.

When all six are checked, Phase 18 is complete.

---

## Completion note

Implementation is complete and validated by `scripts/phase18_check.sh`.
