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

## Phase verdicts

| Phase | Title | Verdict | Date |
|-------|-------|---------|------|
| 0 | Pre-flight: real product boots | PASS (gates green, built app boots real DB, overlay on hotkey) | 2026-07-15 |
| 1 | The Brain, live | PARTIAL — live on OpenAI (gpt-4o), responds in character; Claude A/B blocked on Anthropic credit | 2026-07-15 |
| 2 | Perception and memory, live | not started | — |
| 3 | Voice, live | not started | — |
| 4 | The hands, live | not started | — |
| 5 | The connected world, live | not started | — |
| 6 | The agent runtime, live | not started | — |
| 7 | Signed build + onboarding | not started | — |
| 8 | The week | not started | — |
