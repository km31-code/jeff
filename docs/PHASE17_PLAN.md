# Phase 17: Reliability and Productization Gate — Implementation Plan

**Status:** not started  
**Phase ref:** docs/PHASES_NEXT.md § Phase 17  
**Exit criteria location:** docs/PHASES_NEXT.md § Phase 17 → Exit criteria

This document is the authoritative work tracker for Phase 17. Update the status of each
milestone as work completes. If a session ends mid-phase, the next session (or Codex)
resumes from the first milestone not marked `done`.

---

## Milestone order

```
M17.1 (provider seams) → M17.2 (failure modes) → M17.3 (latency) → M17.4 (behavioral) → M17.5 (check.sh)
```

M17.1 first: cleans the call sites that M17.2 adds error types to.
M17.3 and M17.4 can be done in parallel after M17.1.
M17.5 last: aggregates all exit-criteria checks.

---

## M17.1: Provider abstraction seams

**Status:** done

**Goal:** All OpenAI call sites go through a Rust trait interface. Swapping a provider
later (e.g., a local model) is an adapter swap, not a rewrite.

**Current state:**
- `providers.rs` already has 4 trait stubs: `SpeechToTextProvider`, `TextToSpeechProvider`,
  `ReasoningModelProvider`, `EmbeddingsProvider` — all with `#![allow(dead_code)]` and no
  concrete implementations. Zero call sites use them.
- `ClassifierProvider` trait is missing entirely.
- Call sites in `reasoning.rs`, `voice.rs`, `embedding.rs`, `classifier.rs` call OpenAI HTTP
  directly.

**What to do:**

1. In `providers.rs`:
   - Add `ClassifierProvider` trait (method: `classify(&self, text: &str, api_key: &str) -> Result<IntentClassificationDto, String>`).
   - Add `OpenAiReasoningProvider`, `OpenAiSttProvider`, `OpenAiTtsProvider`,
     `OpenAiEmbeddingsProvider`, `OpenAiClassifierProvider` structs — each implements the
     corresponding trait by delegating to the existing OpenAI HTTP logic.
   - Remove `#![allow(dead_code)]`.

2. In `reasoning.rs`, `voice.rs`, `embedding.rs`, `classifier.rs`:
   - Refactor call sites to accept the provider through a parameter or from `JeffState`
     rather than constructing OpenAI HTTP requests inline.
   - No behavior change on the happy path — same HTTP requests, same error semantics.

3. In `lib.rs`: expose the `providers` module publicly for integration tests.

**Constraints:**
- OpenAI is the only implementation in this phase. No second provider.
- All existing Rust unit tests must continue to pass.
- No change to any Tauri command signatures — this is an internal refactor.

**Files touched:**
- `desktop/src-tauri/src/providers.rs` — add trait + 5 OpenAi*Provider structs
- `desktop/src-tauri/src/reasoning.rs` — use ReasoningModelProvider
- `desktop/src-tauri/src/voice.rs` — use SpeechToTextProvider, TextToSpeechProvider
- `desktop/src-tauri/src/embedding.rs` — use EmbeddingsProvider
- `desktop/src-tauri/src/classifier.rs` — use ClassifierProvider
- `desktop/src-tauri/src/lib.rs` — pub mod providers

**Verification:**
- `cargo build` passes.
- `cargo test -- --test-threads=1` passes.
- `grep -r "reqwest\|openai" reasoning.rs voice.rs embedding.rs classifier.rs` shows only
  provider impl bodies, not business-logic call sites.

---

## M17.2: Failure-mode handling

**Status:** pending

**Goal:** Each of the 4 failure scenarios produces a specific, actionable message in the
companion view. No blank crashes, no generic "something went wrong" banners.

**The 4 scenarios and required messages:**

| # | Scenario | Trigger | Companion message |
|---|---|---|---|
| 1 | API timeout | OpenAI request exceeds timeout | "Jeff couldn't reach OpenAI — check your network connection." |
| 2 | Invalid/missing API key | OpenAI returns 401 or key absent | "Your API key isn't working. Open settings to update it." |
| 3 | Missing OS permission | AXIsProcessTrusted or notification permission denied | "Jeff needs [permission] to do this — open System Settings." |
| 4 | DB lock contention | SQLite SQLITE_BUSY after busy_timeout | "Jeff ran into a save conflict. Try again in a moment." |

**What to do:**

1. Create `desktop/src-tauri/src/errors.rs`:
   - Define `JeffError` enum with variants: `ApiTimeout`, `InvalidApiKey`,
     `MissingOsPermission(String)`, `DbLockContention`.
   - Implement `std::fmt::Display` — each variant returns its user-facing message string.
   - Implement `Into<String>` so commands can return `Err(e.to_string())` for Tauri.

2. In `commands.rs`:
   - At the command boundary (the `#[tauri::command]` fn body), catch the 4 failure
     conditions and return the appropriate `JeffError` variant stringified.
   - Specifically: reqwest timeout → `ApiTimeout`; 401 response → `InvalidApiKey`;
     permission false → `MissingOsPermission`; SQLITE_BUSY after timeout → `DbLockContention`.

3. In `lib.rs`: expose `errors` module.

4. In `desktop/src/App.tsx`:
   - In the Tauri `invoke` error handler (the `.catch` / `try/catch` paths), map each
     known error string prefix to the exact companion-displayed message.
   - Any unrecognized error gets a safe fallback message, never silence.
   - Add a `data-testid="jeff-error-banner"` attribute to the error display element so the
     check script can verify the render branch exists.

**Constraints:**
- No change to the happy path — only error branches touched.
- Messages are string constants defined in `errors.rs`, not constructed at runtime.
- Frontend mapping is exhaustive: unrecognized error code → safe generic fallback, not silence.

**Files touched:**
- `desktop/src-tauri/src/errors.rs` (new)
- `desktop/src-tauri/src/lib.rs`
- `desktop/src-tauri/src/commands.rs`
- `desktop/src/App.tsx`

**Verification:**
- `cargo build` passes.
- `cargo test` passes.
- `grep -q "JeffError"` finds the enum in errors.rs.
- All 4 variant names appear in errors.rs.
- `grep -q "jeff-error-banner\|ApiTimeout\|InvalidApiKey"` finds the mapping in App.tsx.

---

## M17.3: Latency budget assertions

**Status:** pending

**Goal:** Latency budgets are code-level constants. At least one budget (startup) is
asserted by an offline Rust test. The classifier budget is asserted by the existing eval
harness. LLM and TTS budgets are asserted structurally (no blocking ops before request fire).

**The 4 budgets:**

| Budget | Target | Assertion method |
|---|---|---|
| Startup to companion-ready | < 2000 ms | Rust unit test: measure JeffState init wall time |
| First LLM token | < 1000 ms | Structural: no blocking op between command receipt and first HTTP write |
| First audio token | < 400 ms after first LLM token | Structural: TTS synthesis starts on first token, not after stream complete |
| Classifier p50 | < 150 ms | Eval harness assertion (gated on OPENAI_API_KEY) |

**What to do:**

1. Create `desktop/src-tauri/src/latency.rs`:
   - Define the 4 constants:
     ```rust
     pub const STARTUP_BUDGET_MS: u64 = 2000;
     pub const FIRST_TOKEN_BUDGET_MS: u64 = 1000;
     pub const FIRST_AUDIO_BUDGET_MS: u64 = 400;
     pub const CLASSIFIER_BUDGET_MS: u64 = 150;
     ```
   - Add a `#[test] fn startup_budget_is_met()` that:
     - Initializes `JeffState` with a temporary SQLite file (no OpenAI calls).
     - Measures wall time using `std::time::Instant`.
     - Asserts elapsed < `STARTUP_BUDGET_MS`.

2. In `tests/intent_eval.rs`:
   - After computing latency percentiles, add an assertion:
     `assert!(p50_ms < latency::CLASSIFIER_BUDGET_MS, "classifier p50 {}ms exceeds budget", p50_ms);`
   - This test is already gated on `OPENAI_API_KEY`.

3. In `lib.rs`: expose `latency` module.

**Constraints:**
- `startup_budget_is_met` must be fully offline — temp SQLite, no HTTP.
- LLM and TTS structural checks are not automated tests in this phase; they are verified
  by reading `streaming.rs` and confirming `spawn_tts_chunk` is called on first token event,
  not on stream complete. The check script grep-asserts the structural pattern.
- Live latency tests (requiring OPENAI_API_KEY) skip cleanly when the key is absent.

**Files touched:**
- `desktop/src-tauri/src/latency.rs` (new)
- `desktop/src-tauri/src/lib.rs`
- `tests/intent_eval.rs`

**Verification:**
- `cargo test startup_budget_is_met` passes in < 2s measured time.
- All 4 constants present in latency.rs.
- Eval harness (with key) asserts p50 < 150ms.

---

## M17.4: Behavioral assertions for phases 11–16

**Status:** pending

**Goal:** Each existing phase11–16 check script gets at least one assertion that tests
runtime behavior or a named unit test, not only symbol presence. Existing grep checks
are never removed — behavioral checks are added on top.

**Per-phase additions:**

### Phase 11
The check script already verifies overlay flags in Rust code (decorations, always_on_top,
focused) and runs `cargo test ambient`. This is sufficient behavioral coverage.
**Addition:** Parse `tauri.conf.json` to assert the main window has `"visible": false` and
the overlay window entry (if defined there) matches expected flags. The check already does
this for `"visible": false` (check #14). Verify #14 is present and add one more: assert
`"alwaysOnTop": true` exists in the overlay window config in tauri.conf.json.

### Phase 12
Check script currently runs `cargo test` implicitly. 
**Addition:** Explicitly run the streaming-specific tests by name and assert they pass:
```bash
cargo test phrase_needs_synthesis -- --test-threads=1
cargo test streaming -- --test-threads=1
```

### Phase 13
Watcher has 6 unit tests but none named for debounce behavior.
**Addition:** Add `fn watcher_debounces_rapid_events()` to `watcher.rs` — tests that the
debounce accumulator deduplicates events fired within the debounce window. Uses tempdir,
no real filesystem watcher. Then add to `phase13_check.sh`:
```bash
grep -q "fn watcher_debounces_rapid_events" "$WATCHER_RS" || fail "..."
cargo test watcher_debounces_rapid_events -- --test-threads=1
```

### Phase 14
The eval harness already runs and prints latency. 
**Addition:** In `phase14_check.sh`, after running the eval, parse the p50 line from stdout
and assert it is numeric (verifies the harness ran, not just compiled).

### Phase 15
`proactive.rs` has 7 tests covering cooldowns and drift. Missing: quiet-mode suppression.
**Addition:** Add `fn quiet_mode_suppresses_reorientation()` to `proactive.rs` — sets
quiet mode true on a mock `AmbientState`, calls `generate_reorientation`, asserts it
returns without firing. Then add to `phase15_check.sh`:
```bash
grep -q "fn quiet_mode_suppresses_reorientation" "$PROACTIVE_RS" || fail "..."
cargo test quiet_mode_suppresses_reorientation -- --test-threads=1
```

### Phase 16
Check script already runs `cargo test` and explicitly checks for the chain cancel test.
The cancel test `subtask_chain_cancel_leaves_no_pending_approval_proposals` is verified
by name in check #8. This is sufficient behavioral coverage.
**Addition:** Run that test explicitly by name to confirm it passes in isolation:
```bash
cargo test subtask_chain_cancel_leaves_no_pending_approval_proposals -- --test-threads=1
```

**Files touched:**
- `desktop/src-tauri/src/watcher.rs` — add `fn watcher_debounces_rapid_events()`
- `desktop/src-tauri/src/proactive.rs` — add `fn quiet_mode_suppresses_reorientation()`
- `scripts/phase11_check.sh` — add tauri.conf.json alwaysOnTop check
- `scripts/phase12_check.sh` — add explicit streaming test run
- `scripts/phase13_check.sh` — add watcher debounce test check + run
- `scripts/phase14_check.sh` — add latency line parse check
- `scripts/phase15_check.sh` — add quiet mode test check + run
- `scripts/phase16_check.sh` — add explicit cancel-rollback test run

**Constraints:**
- All new tests are offline (tempdir / mock state, no HTTP, no real filesystem watcher).
- Existing grep checks in each script are not removed.
- Phase 11 and 16 changes are small (one assertion each); 13 and 15 require new test code.

**Verification:**
- `cargo test watcher_debounces_rapid_events` passes.
- `cargo test quiet_mode_suppresses_reorientation` passes.
- All of `phase11_check.sh` through `phase16_check.sh` run without error.

---

## M17.5: phase17_check.sh

**Status:** pending

**Goal:** Single script that verifies all 4 Phase 17 exit criteria and calls each of
phase11–16_check.sh as a regression gate.

**Checks (in order):**

```
1.  providers.rs contains all 5 trait definitions
2.  providers.rs contains all 5 OpenAi*Provider struct definitions
3.  #![allow(dead_code)] is no longer in providers.rs
4.  errors.rs exists and contains JeffError enum
5.  All 4 error variant names present in errors.rs: ApiTimeout, InvalidApiKey,
    MissingOsPermission, DbLockContention
6.  jeff-error-banner (or error mapping string) present in App.tsx
7.  latency.rs exists and contains all 4 budget constants
8.  cargo test startup_budget_is_met passes
9.  (gated: OPENAI_API_KEY set) cargo test --test intent_eval reports p50 < 150ms
10. phase11_check.sh passes
11. phase12_check.sh passes
12. phase13_check.sh passes
13. phase14_check.sh passes (eval harness skipped if no key)
14. phase15_check.sh passes
15. phase16_check.sh passes
```

**Files touched:**
- `scripts/phase17_check.sh` (new)

**Verification:** `bash scripts/phase17_check.sh` exits 0 with all PASS lines.

---

## Files created in Phase 17

| File | Purpose |
|---|---|
| `desktop/src-tauri/src/errors.rs` | JeffError enum, 4 variants, user-facing messages |
| `desktop/src-tauri/src/latency.rs` | 4 budget constants + startup_budget_is_met test |
| `scripts/phase17_check.sh` | exit criteria verification script |

## Files modified in Phase 17

| File | Change |
|---|---|
| `desktop/src-tauri/src/providers.rs` | ClassifierProvider trait + 5 OpenAi*Provider impls; remove dead_code allow |
| `desktop/src-tauri/src/lib.rs` | pub mod errors; pub mod latency |
| `desktop/src-tauri/src/reasoning.rs` | call sites use ReasoningModelProvider |
| `desktop/src-tauri/src/voice.rs` | call sites use SttProvider, TtsProvider |
| `desktop/src-tauri/src/embedding.rs` | call sites use EmbeddingsProvider |
| `desktop/src-tauri/src/classifier.rs` | call sites use ClassifierProvider |
| `desktop/src-tauri/src/commands.rs` | return JeffError variants at error boundary |
| `desktop/src/App.tsx` | map JeffError codes to companion messages |
| `desktop/src-tauri/src/watcher.rs` | add watcher_debounces_rapid_events test |
| `desktop/src-tauri/src/proactive.rs` | add quiet_mode_suppresses_reorientation test |
| `scripts/phase11_check.sh` | add tauri.conf.json alwaysOnTop structural check |
| `scripts/phase12_check.sh` | add explicit streaming test run by name |
| `scripts/phase13_check.sh` | add watcher debounce test check + run |
| `scripts/phase14_check.sh` | add eval latency line parse check |
| `scripts/phase15_check.sh` | add quiet mode test check + run |
| `scripts/phase16_check.sh` | add cancel-rollback test run by name |
| `tests/intent_eval.rs` | assert p50 < CLASSIFIER_BUDGET_MS |

---

## Exit criteria (from PHASES_NEXT.md)

- [x] M17.1 done — phase17_check.sh verifies provider interfaces and OpenAI impls conform.
- [ ] M17.2 done — the 4 failure modes each produce a correct, specific UI message.
- [ ] M17.3 done — all 4 latency budgets are measured and pass on a reference machine.
- [ ] M17.4 done — phase17_check.sh runs behavioral assertions for phase 11–16 and passes.
- [ ] M17.5 done — `bash scripts/phase17_check.sh` exits 0 with all PASS lines.
