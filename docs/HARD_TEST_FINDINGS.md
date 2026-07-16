# Jeff — Hard Test Findings

Live-path validation findings from executing `docs/HARD_TEST_PLAN.md`. One row per
issue. This is the real roadmap once the run completes.

- Severity: `blocker` (stops deploy) / `degraded` (works, worse than intended) /
  `cosmetic` (polish).
- Type: `bug` (fix now) / `ceiling` (product decision — the design, not a defect).

## Findings

| # | Phase | Symptom | Severity | Type | Status |
|---|-------|---------|----------|------|--------|
| 1 | 0 | `tauri build` bundled the wrong executable: `Jeff.app/Contents/MacOS/` contained `agent_eval` (an eval harness) instead of `jeff-desktop`, because the crate has 8 auto-discovered bins and no declared default, so the bundler grabbed the alphabetically-first one. A shipped `.app` would have launched a test harness, not the app. | blocker | bug | fixed — added `default-run = "jeff-desktop"` to `[package]` in `src-tauri/Cargo.toml`; rebundle now produces `CFBundleExecutable = jeff-desktop` |
| 2 | 0 | DMG bundling step (`bundle_dmg.sh`) fails during `tauri build`. Root cause: no signing identity (`signingIdentity: null`). `.app` bundles fine. | cosmetic | ceiling | deferred to Phase 7 (signed/notarized build); does not block dev-binary testing |
| 3 | 0/1 | Overlay showed "watching jeff-v1-smoke" — a smoke-test task from 2026-04-28 was still `is_active=1`, pinning the watcher to `~/Desktop/jeff-v1-smoke`, a folder since deleted. | degraded | bug | fixed in real DB (snapshotted): deactivated the smoke task, cleared dead `watched_folders`/`watched_file_registry`/`task_focus_log`, repointed `preferred_workspace_folder` to the repo `docs/`. Data-hygiene only; no code change. |
| 4 | 1 | Anthropic API key authenticates but has $0 credit ("credit balance too low"). Router only auto-falls-back to OpenAI when the key is *absent*, not when it's unfunded, so all Claude tiers (Conversation/Judgment/Craft) would error at runtime. | blocker (for Claude path) | ceiling | user adding Anthropic credit; interim workaround: `tier_model_map` routes to OpenAI. Revert (delete the setting) once Anthropic is funded. Possible latent bug: consider treating a runtime credit/auth error as a fallback trigger, not just a missing key. |
| 5 | 1 | Responses felt flat/generic ("I'm a coworker here to assist with your tasks. What do you need help with?") — exactly the characterless assistant-speak VISION forbids. Root cause: the persona system prompt (character.rs:11) is strong and correct, but `gpt-4o-mini` ignores nuanced persona instructions. | degraded | bug | mitigated: routed the tiers the user directly feels (Conversation + Craft) to `gpt-4o`, which follows the persona sharply. Empirical proof: identical prompt → gpt-4o "I'm Jeff, your coworker." vs gpt-4o-mini filler. Real top-tier = Claude once funded (the prompt is tuned for it). Reflex/Judgment stay on gpt-4o-mini for cost. |
| 6 | 1 | Active task auto-named from a throwaway greeting ("hey what are you"); renders as a faded title above the thread and reads like a ghost/rendering artifact. | cosmetic | ceiling | open — task auto-titling should skip greetings/short non-work openers, or defer naming until there's real work. |
| 7 | 1 | A prominent "Talk to Jeff" button sits between the thread and the input, redundant with the adjacent "mic" button. Adds clutter to the primary surface. | cosmetic | ceiling | open — needs a design decision (consolidate to one voice affordance). |

## Autonomous run log (2026-07-15, keys present, Anthropic unfunded)
- Phase 1 character eval (live, gpt-4o-mini): 14/15 (93%), above 90% bar. ~$0.001.
- Phase 1 goal eval (live): pass (exit 0).
- Phase 1 A/B pipeline: verified end-to-end on a 2-case smoke (legacy gpt-4o-mini
  vs apex gpt-4o). Full 20-case Claude run is post-credit.
- Phase 1 cached-token ratio check: 0.000 on OpenAI — it is an Anthropic
  prompt-caching metric, so genuinely gated on Claude.
- Phase 2 embedding model (bge-small-en-v1.5-q8_0): downloaded to the app models
  dir and checksum-verified (36,806,944 bytes, sha256 match).
- Phase 2 llama.cpp: installed via brew; verified it serves REAL 384-dim bge
  embeddings (CPU mode). GPU/Metal backend crashes on load (llama.cpp bug), so it
  must run with `-ngl 0`.
- Phase 3 Realtime pre-check: OpenAI key mints a `gpt-realtime-2.1` session
  (HTTP 200) — Realtime IS enabled on the org. Voice is unblocked pending mic
  permission.
- Total estimated LLM spend across the whole session so far: ~$0.01.

Finding 8 (Phase 2, degraded/bug): `JEFF_LOCAL_LLAMACPP_SERVER` is env-var only
(local_runtime.rs:576) with no persisted setting, so a Finder-launched app can
never pick up the local runtime. Needs a stored config + onboarding entry to be
usable outside a dev shell.

Finding 9 (Phase 2, bug): the local sidecar spawn (local_runtime.rs:273) starts
llama-server with no `-ngl 0`, so on Apple Silicon with the current brew
llama.cpp it crashes on model load via the Metal backend (`GGML_ASSERT(buf_dst)`).
Until llama.cpp fixes it, the app must pass `JEFF_LOCAL_LLAMACPP_ARGS="-ngl 0"`,
or the spawn should default to CPU for the embedding sidecar. Also: `start()`
requires the reasoning model (`reflex-instruct.gguf`), which is a separate
(larger) download not yet fetched.

## What I need from you (precise, ordered by leverage)

Each item is something only you can do (accounts, OS permission dialogs, real
content, judging). Do as many as you like; each unblocks the phase named.

1. macOS permission grants (2 min, unblocks Phases 2/3/4). Open System Settings →
   Privacy & Security and toggle Jeff on under: Accessibility (window/document
   awareness), Microphone (voice), Automation (native doc edits in Pages/Word).
   If Jeff isn't listed yet, it appears the first time it requests each.

2. Anthropic credit — DO THIS LAST per your call (2 min, unblocks the real
   product). console.anthropic.com → Billing → add credit ($5-10 is plenty at the
   $2-5/day budgets) and set a monthly spend cap while you're there. Tell me when
   done and I flip Jeff to Claude by deleting one DB setting; then I run the real
   20-pair A/B and hand you the blind packet to judge (Phase 1, ~30 min of your
   time).

3. Phase 4 hands (5 min setup): Chrome → chrome://extensions → Developer mode →
   Load unpacked → select `browser-extension/selection-capture/`. Have one real
   Google Doc and one real Pages doc open. Then I run the tracked-change +
   graduation tests.

4. Phase 5 connected world (heavy, optional): stand up bring-your-own MCP servers
   for Gmail / Calendar / Drive / web search and complete their OAuth against your
   Google account; hand me the tokens. Then I wire and test triage/prep/research.

5. Phase 7 signed build (later): Apple Developer ID cert + signing identity +
   app-specific password for notarization.

6. Phase 8 the week (the real acceptance test): use Jeff as your coworker for five
   working days, then perform the day-in-the-life on the signed build.

## Phase verdicts

| Phase | Title | Verdict | Date |
|-------|-------|---------|------|
| 0 | Pre-flight: real product boots | PASS (gates green, built app boots real DB, overlay on hotkey) | 2026-07-15 |
| 1 | The Brain, live | PARTIAL — live on OpenAI (gpt-4o), responds in character; Claude A/B blocked on Anthropic credit | 2026-07-15 |
| 2 | Perception and memory, live | PARTIAL — real bge embeddings verified working (CPU); app-wiring blocked on findings 8/9 + needs Accessibility + a real doc | 2026-07-15 |
| 3 | Voice, live | PRE-CHECK PASS — Realtime enabled on the org, session mints; needs mic permission + a spoken session | 2026-07-15 |
| 4 | The hands, live | not started | — |
| 5 | The connected world, live | not started | — |
| 6 | The agent runtime, live | not started | — |
| 7 | Signed build + onboarding | not started | — |
| 8 | The week | not started | — |
