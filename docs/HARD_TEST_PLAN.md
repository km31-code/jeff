# Jeff — Hard Test Plan

The purpose of this document is one question: does Jeff genuinely work as a full
product, deployable today, right now, to help real work — or not. The Apex A–E
critical path and Epoch F's in-repo path are built and green, but almost every
live surface (frontier-model judgment, realtime voice, real document edits, live
Gmail/Calendar/Drive, real agent jobs) has only been verified at the
deterministic-spine level. This plan turns the live paths on, one at a time, and
proves each against reality before we build any further epoch.

Binding rule from CLAUDE.md and project memory: green gates and passing tests do
NOT prove Jeff works. Only launching the built binary against the real
`~/Library/Application Support/com.jeff.desktop/jeff_store.sqlite3` and using it
does. Every phase below ends in a runtime proof, not an inspection.

## Legend
- `[YOU]` — only the user can do this (accounts, billing, OS permission dialogs,
  OAuth consent, speaking into the mic, being/​recruiting a human tester).
- `[CLAUDE]` — Claude can do this (wiring env, scripts, gates, builds, log
  triage, bug fixes).
- Severity for findings: `blocker` / `degraded` / `cosmetic`.
- Type for findings: `bug` (fix now) / `ceiling` (product decision).

## Grounded wiring reference (from the actual code)
- Keys: macOS Keychain, service `com.jeff.desktop`, accounts `openai_api_key`,
  `anthropic_api_key`. Also read from env `OPENAI_API_KEY` / `ANTHROPIC_API_KEY`
  (fastest for a dev-build test). Commands: `store_openai_api_key`,
  `store_anthropic_api_key`.
- Tier map: defaults Reflex `gpt-4o-mini` (or local), Conversation
  `claude-haiku-4-5`, Judgment `claude-sonnet-5`, Craft `claude-sonnet-5`.
  Commands: `get_tier_model_map` / `set_tier_model_map`. Recommendation: verify
  these ids are current, set Craft to `claude-opus-4-8`.
- Local runtime (Reflex + embeddings): llama.cpp server. Env
  `JEFF_LOCAL_LLAMACPP_SERVER` (path to `llama-server`), `JEFF_LOCAL_RUNTIME_PORT`,
  `JEFF_LOCAL_RUNTIME_ENDPOINT` (default `http://127.0.0.1:<port>`),
  `JEFF_LOCAL_LLAMACPP_ARGS`. Commands: `download_local_model`,
  `download_curated_embedding_model`.
- Realtime voice: OpenAI Realtime, model `gpt-realtime-2.1`, ephemeral secret
  minted at `https://api.openai.com/v1/realtime/client_secrets` with the OpenAI
  key. Commands: `get_voice_config` / `set_voice_config` / `start_voice_session`.
  Wake word: `JEFF_WAKE_WORD_COMMAND`.
- Hands: browser extension at `browser-extension/selection-capture/`
  (real `manifest.json`). Native docs via AppleScript (needs Automation
  permission). Trust ladder commands present; `email.send` / `file.delete`
  hard-capped at L1.
- MCP tool bus: transports `stdio` / `http` / `loopback`. Real Gmail/Calendar/
  Drive/web = bring-your-own MCP server. OAuth token env pattern
  `JEFF_MCP_OAUTH_TOKEN_<CONNECTIONNAME_UPPERCASED_NONALNUM_TO_UNDERSCORE>`.
  Commands: `add_tool_connection`, `discover_connection_tools`,
  `set_tool_connection_enabled`, `remove_tool_connection`. Web search is gated
  behind an enabled MCP `web.search`/`search` tool.
- Build/run: `cd desktop && npm run tauri dev` (dev), `npm run tauri build`
  (bundle). Signing identity is null in `tauri.conf.json` — a truly signed build
  needs an Apple Developer cert (Phase 7 only). CI: `.github/workflows/release.yml`
  (order: eval -> build -> sign -> notarize).
- Gates: `scripts/apex_e7_check.sh` (flat ship gate), `scripts/apex_eval.sh all`
  (quality spine), `scripts/apex_<id>_check.sh` per milestone,
  `JEFF_SKIP_ADJACENT_GATES=1` to run one standalone,
  `JEFF_RUN_EXTERNAL_EVAL=1` to run live-LLM evals.

## Pre-start decisions
- [ ] BYOK, not the bundled relay, for this test. (Recommended.)
- [ ] Defer the signed/notarized build to Phase 7; run the dev binary for
  weeks 1–2. (Recommended.)
- [ ] Test all live surfaces, in the phase order below (cheapest/highest-leverage
  first). (Recommended.)
- [ ] `[YOU]` Provide Anthropic API key and OpenAI API key.

---

## Phase 0 — Pre-flight: prove the real product boots
Goal: baseline. Green gates alone mean nothing until the built binary opens the
real DB with no panic.

- [ ] `[CLAUDE]` Run `scripts/apex_e7_check.sh` and `scripts/apex_eval.sh all`;
  confirm backend/frontend/relay tests green on this machine today.
- [ ] `[CLAUDE]` Snapshot the production DB to scratchpad, then
  `cd desktop && npm run tauri build` and launch the built binary against the
  real DB; watch for migration panics.
- [ ] `[YOU]` Confirm it is OK to run migrations on the live DB (snapshotted
  first).

Pass bar: every gate green today; built app opens the real DB with no panic;
overlay appears on the hotkey. Any red gate or panic stops the plan here.

---

## Phase 1 — The Brain, live (highest leverage; resolves the failed A/B)
Goal: turn on frontier models and honestly re-run A1's blind A/B, which failed
8/20 (needs >=16/20) and was waived as "model-confounded."

- [ ] `[YOU]` Provide both API keys (Anthropic: Judgment/Craft/Conversation;
  OpenAI: Reflex fallback, embeddings fallback, realtime voice).
- [ ] `[CLAUDE]` Export keys for the launching shell; confirm the router hits
  Anthropic at runtime (debug-log assertion, same as `apex_a1_check.sh`).
- [ ] `[CLAUDE]` Confirm `claude-haiku-4-5` / `claude-sonnet-5` are live ids;
  `set_tier_model_map` Craft -> `claude-opus-4-8`.
- [ ] `[CLAUDE]` Generate 20 blind revision pairs (Apex vs v2 baseline).
- [ ] `[YOU]` Judge the 20 pairs (this bar is defined as your preference).
  ~30 min.
- [ ] `[CLAUDE]` Run `JEFF_RUN_EXTERNAL_EVAL=1 scripts/character_eval.sh` and the
  goal eval with a real model in the loop.
- [ ] `[CLAUDE]` Verify A2 cached-token ratio >70% on a 20-turn conversation.
- [ ] `[CLAUDE]` Force a budget-degradation test; confirm Judgment falls back to
  Conversation, spend visible in Privacy Center.

Pass bar: A/B preferred >=16/20 by you; character eval >=90%; cached ratio >70%;
budget degradation graceful. If the A/B fails again with a frontier model, that
is the top finding: the prompt/character, not the model, is the ceiling.

---

## Phase 2 — Perception and memory, live (the on-device sidecar)
Goal: prove semantic mode (real embeddings, comprehension, recall), not the
lexical fallback.

- [ ] `[YOU]` Install llama.cpp (`brew install llama.cpp` -> `llama-server`), or
  authorize Claude to run brew.
- [ ] `[YOU]` Grant Accessibility permission when prompted (System Settings ->
  Privacy & Security -> Accessibility).
- [ ] `[YOU]` Have a real document open (Google Doc, Pages, or workspace draft).
- [ ] `[CLAUDE]` Set `JEFF_LOCAL_LLAMACPP_SERVER`; download curated embedding +
  Reflex models; confirm `health_check()` and `semantic_embedding_available()`.
- [ ] `[CLAUDE]` "Where is my argument weakest?" returns a specific, defensible
  section with no paste.
- [ ] `[CLAUDE]` Rewrite one paragraph 5x; churn localizes to it; comprehension
  pass fires on the 5-min cadence.
- [ ] `[CLAUDE]` Re-run `phase31_check.sh` raw-text audit with real content
  flowing.

Pass bar: semantic embeddings available; weakest-argument answer correct on a
doc you know; churn localizes; recall <30ms; raw-text audit clean.

---

## Phase 3 — Voice, live (never run before)
Goal: prove Pillar 5 realtime end to end; highest "looks done, never executed"
risk.

- [ ] `[YOU]` Grant Microphone permission when prompted.
- [ ] `[YOU]` Ensure OpenAI Realtime is enabled on the key's org (separate
  entitlement/billing).
- [ ] `[CLAUDE]` Confirm ephemeral session mints (`gpt-realtime-2.1`) and WebRTC
  connects.
- [ ] `[YOU]` Run a 3-minute spoken session, recorded: discuss a task, apply one
  revision by voice ("fix it"), interrupt Jeff once, let Jeff interject once.
- [ ] `[CLAUDE]` Measure latency: p50 <800ms, p95 <1.5s, barge-in cut <100ms.
- [ ] `[CLAUDE]` Pull network mid-session; confirm pipeline fallback next turn,
  spoken notice, no crash, no dead air >3s.
- [ ] `[CLAUDE]` (Optional) wire wake word via `JEFF_WAKE_WORD_COMMAND`.

Pass bar: recorded session feels conversational; latency meets the bar;
network-pull fallback graceful.

---

## Phase 4 — The hands, live (real edits in real docs)
Goal: prove Pillar 6 — Jeff acts, not just proposes.

- [ ] `[YOU]` Load the unpacked extension in Chrome (chrome://extensions ->
  Developer mode -> Load unpacked -> `browser-extension/selection-capture/`).
- [ ] `[YOU]` Grant macOS Automation permission when Pages/Word prompts.
- [ ] `[YOU]` Have a real Google Doc and a real Pages doc open.
- [ ] `[CLAUDE]` "Fix that transition" in Google Docs -> tracked change at the
  right location; reject reverts natively.
- [ ] `[CLAUDE]` Approve 10 consecutive `doc.insert` -> graduation offer; accept
  -> next insert L2 with revert receipt; revert -> byte-identical restore.
- [ ] `[CLAUDE]` Try to raise `email.send` / `file.delete` above L1 by editing
  settings; confirm the hard cap holds.
- [ ] `[CLAUDE]` `doc.insert` into Pages via AppleScript; unsupported app ->
  guided-apply copy button, no error state.

Pass bar: tracked change lands and reverts; graduation fires at 10 and revert is
byte-identical; irreversible classes cannot graduate.

---

## Phase 5 — The connected world, live (MCP: Gmail, Calendar, Drive, web)
Goal: prove Pillar 8. No bundled Google servers exist — every connection is
bring-your-own MCP server + OAuth token.

- [ ] `[YOU]` Stand up and OAuth-authorize MCP servers for Gmail, Google
  Calendar, Google Drive, and web search against your Google account.
- [ ] `[YOU]` Provide OAuth tokens / complete each consent flow.
- [ ] `[CLAUDE]` `add_tool_connection` each; set
  `JEFF_MCP_OAUTH_TOKEN_<NAME>`; `discover_connection_tools`.
- [ ] `[CLAUDE]` Gmail (E3): triage the real inbox; draft a reply in-thread;
  confirm `email.send` stays user-only.
- [ ] `[YOU]` Confirm triage "matters vs noise" quality on your real inbox.
- [ ] `[CLAUDE]` Calendar (E4): full-day awareness + pre-meeting prep offer from
  the document delta; event creation as L1 proposal.
- [ ] `[CLAUDE]` Drive/Docs (E5): ingest a remote Doc (grounds retrieval with
  provenance); remove it (chunks purge).
- [ ] `[CLAUDE]` Web (E2): "find three sources supporting X and draft the
  paragraph" -> one job, real resolvable citations.
- [ ] `[CLAUDE]` Disconnect an integration in Privacy Center; confirm calls stop
  (call log) and cached data purges.

Pass bar: briefing flags a genuinely important email and skips noise from your
inbox; reply draft lands in the right thread (send requires you); research
citations resolve; disconnect halts + purges. Gmail is highest-value if setup is
heavy.

---

## Phase 6 — The agent runtime, live (one real 20-minute job)
Goal: prove Pillar 7 with a real Craft model and real tools.

- [ ] `[YOU]` Give a real task on real material you can judge (a section of your
  actual writing + your notes).
- [ ] `[CLAUDE]` "Draft the counterargument section from my notes" -> plan,
  retrieve, draft, fresh-context self-verify, deliver assessment + draft +
  placement proposal; capture the job event stream + verification transcript.
- [ ] `[CLAUDE]` Kill network mid-job -> checkpoints, resumes on reconnect.
- [ ] `[CLAUDE]` Steer a running job ("make it two paragraphs") -> reflected in
  deliverable.
- [ ] `[CLAUDE]` Impossible task (verify claim vs a missing PDF) -> honest
  "couldn't verify" + capability request, no fabrication.
- [ ] `[YOU]` Leave a standing evening citation-check job running overnight (app
  closed) so the daemon + morning-readiness path runs for real.

Pass bar: a deliverable you'd actually use with an honest view; resume works;
steering works; impossible task fails honestly; overnight standing job posts a
receipt and the morning briefing folds it in.

---

## Phase 7 — Signed build + real onboarding test
Goal: the two manual E7 acceptance gates.

- [ ] `[YOU]` Provide an Apple Developer ID cert, signing identity, and
  app-specific password for notarization.
- [ ] `[CLAUDE]` Set `signingIdentity` + notarization in `tauri.conf.json`; run
  the release build; confirm Gatekeeper passes on a clean machine.
- [ ] `[YOU]` Recruit one real non-technical person (not you, not Claude);
  record them installing and reaching first useful Jeff observation in <5 min,
  no coaching.

Pass bar: signed app launches past Gatekeeper on a machine that never saw the
dev build; recruited tester hits first value in <5 min on video.

---

## Phase 8 — The week (the real acceptance test)
Goal: the E7 criterion that supersedes all: a full week of daily self-use, and
the Part II day-in-the-life performed live.

- [ ] `[CLAUDE]` Stand up opt-in local crash-free-session telemetry (>99.5%
  target) + a week-long data-integrity / migration check.
- [ ] `[YOU]` Use Jeff as your coworker for five working days — real work, real
  inbox, real writing. Note every dumb moment, every mistimed interruption,
  every delight.
- [ ] `[YOU]` At week's end, perform the Part II day-in-the-life live on the
  signed build (8:40 briefing, 10:00 observation, 1:25 pre-meeting summary,
  4:30 citation check) in one day.

Pass bar: <2 crashes, zero data loss across the week; day-in-the-life
performable end to end. This is the final go/no-go for "deployable today."

---

## Findings tracker
`[CLAUDE]` maintains `docs/HARD_TEST_FINDINGS.md`: one row per issue with phase,
symptom, severity, and type (bug vs ceiling). At week's end this document is the
real roadmap — grounded in Jeff meeting reality, worth more than any speculative
Epoch G/H work.

## Go / No-Go
Deployable-today requires: Phase 1 green (brain A/B), Phase 8 green (week +
day-in-the-life), and no open `blocker` from Phases 3–6. Voice, hands, and
connected-world each get an independent verdict — any one can ship off-by-default
if not ready, without blocking the core.

## Explicitly out of scope for this test
Epoch G (code sandbox, artifacts, entity model, research radar, negotiated
capability), Epoch H (wit register, screen VLM, Windows port), F3c-client
(phone), F4 (cloud continuation). These wait until the base is validated.
