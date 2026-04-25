# Jeff Build Phases

## Status

- Phase 0: complete
- Phase 1: complete
- Phase 2: complete
- Phase 3: complete
- Phase 4: complete
- Phase 5: complete
- Phase 6: complete
- Phase 7: complete
- Phase 8: complete
- Phase 9: complete
- Phase 10: complete (companion-first interaction layer)
- Phase 11: complete (ambient presence)
- Phase 12: complete (streaming everywhere)
- Phase 13: complete (workspace awareness)
- Phase 14: complete (real intent understanding)
- Phase 15: complete (proactive initiation)
- Phase 16: complete (richer parallel work)
- Phase 17: complete (reliability and productization gate)
- Phase 18: complete (first-run onboarding + secure key management)
- Phase 19: complete (presence completion: launch at login + session restore)
- Phase 20: complete (active window context, title-level)
- Phase 21: complete (privacy and trust control center)
- Phase 22: complete (selection capture and voice naturalness)
- Phase 23: complete (live app actions, personalization, workload awareness, calendar context)
- **Phase 24: complete (distribution + auto-update)**

## Phase 10 Exit Criteria

- default entry is minimal companion UI
- user can interact immediately by voice/text
- revision/subtask/suggestion flows are reachable conversationally
- full workspace remains accessible via toggle
- no backend capability expansion beyond Phases 1-9

---

## Phase 11: Ambient Presence

Make Jeff feel already-there rather than launched. Jeff lives in the
system tray, surfaces via global hotkey as an overlay that does not
steal focus, and uses native notifications instead of in-app banners.

Scope:
- system tray icon with status (idle / listening / working) and menu
- global hotkey to summon/dismiss the overlay from anywhere
- frameless, always-on-top overlay window distinct from full workspace
- collapsed state (compact bar) and expanded state (companion view)
- focus preservation: summoning Jeff does not take focus from the
  user's active app unless the user explicitly clicks into Jeff
- native OS notifications for proactive nudges and completions
- single-instance enforcement; closing the window hides to tray

Exit criteria:
- launching the app puts Jeff in the tray with no window stealing focus
- pressing the global hotkey toggles the overlay in under 200ms
  without the user's active app losing focus
- collapsed and expanded states are both reachable and persist across
  hotkey toggles within a session
- a proactive nudge fires a native OS notification, not an in-app
  toast, and clicking it expands Jeff to the relevant context
- closing the overlay window does not exit the process; quit is
  reachable only via the tray menu
- phase11_check.sh verifies tray, hotkey registration, overlay
  window flags, and notification permission state

---

## Phase 12: Streaming Everywhere

Eliminate turn-taking latency. LLM, STT, and TTS all stream, and
either side can interrupt mid-sentence without waiting for an
utterance boundary.

Scope:
- streaming LLM responses rendered token-by-token in chat and spoken
  by TTS as tokens arrive
- streaming STT producing partial transcripts the router can act on
  before the user finishes speaking
- streaming TTS that can be cut off mid-word on user speech onset
- true bidirectional barge-in: user can interrupt Jeff, Jeff can
  interrupt user (when proactive engine fires) without dropping state
- backpressure and cancellation tokens threaded through the pipeline
  so partial work is cleanly abandoned

Exit criteria:
- first audible TTS token within 400ms of first LLM token
- user speech onset cuts TTS within 150ms and preserves prior
  transcript context for the next turn
- partial STT transcripts trigger intent routing before final
  transcript when confidence threshold is met
- cancellation of an in-flight LLM stream leaves no orphaned
  reasoning, audio, or UI state
- phase12_check.sh runs automated streaming contract checks
  (reason-tagged cancellation, overlay streaming path, first-audio metrics,
  and no leaked turns via unit/integration tests)

---

## Phase 13: Workspace Awareness

Jeff knows what you are working on without being told. A designated
task workspace folder is watched; new and changed files are
auto-ingested into the context pack. Optional clipboard capture
extends awareness beyond the folder.

Scope:
- task workspace folder concept: one active folder per task
- filesystem watcher (debounced) for create/modify/delete events
- auto-ingest pipeline that chunks, embeds, and updates the context
  pack without user action
- optional clipboard capture (off by default, opt-in per task) that
  ingests copied text snippets with provenance
- visible indicator of what Jeff has recently ingested
- per-task ignore rules (size, extension, paths)

Exit criteria:
- dropping a file into the task workspace makes it queryable in
  context within 5 seconds without any user command
- editing a watched file updates the embedding within the same
  window without duplicating prior chunks
- clipboard capture toggle is off by default, persists per task,
  and never ingests when off
- ingested items appear in a "recently learned" list reachable from
  the companion view
- phase13_check.sh verifies watcher debouncing, ingest idempotency,
  clipboard opt-in default, and ignore-rule enforcement

---

## Phase 14: Real Intent Understanding

Replace the TypeScript keyword-based intent router with a
model-based classifier. Routing decisions become semantic, not
lexical, and can carry structured arguments to downstream systems.

Scope:
- model-based intent classifier (small, fast model) producing
  intent label plus structured slots
- replaces keyword routing in `desktop/src/App.tsx` companion flow
- supports existing intents (answer / revision / subtask /
  suggestion) plus a clear unknown/clarify path
- evaluation harness with a labeled set of real user turns
- fallback to current keyword router if classifier unavailable
- latency budget that does not regress perceived response start

Exit criteria:
- classifier achieves agreed accuracy threshold on the eval set
  (threshold defined when eval set is built)
- routing decisions include slots usable by downstream commands
  without re-parsing the raw text
- median classifier latency under 150ms on local hardware
- keyword-router fallback engages cleanly on classifier failure
  and is logged
- phase14_check.sh runs the eval set and prints intent accuracy,
  slot quality, and latency percentiles

---

## Phase 15: Proactive Initiation

Jeff stops being purely reactive. On return to a task Jeff
re-orients you, when an argument drifts Jeff flags it, when you
have been stuck Jeff suggests a next move, and important events
fire push notifications even when the overlay is hidden.

Scope:
- re-orientation: on task resume, Jeff produces a short "where you
  left off" summary unprompted
- drift detection: when current work diverges from stated task
  goal, Jeff surfaces it
- speculative subtask kickoff: when a likely-needed subtask is
  inferred with high confidence, Jeff starts it in the background
  and offers the result
- push notifications for completions, drift flags, and orientation
  prompts via the Phase 11 notification path
- per-trigger throttling and a global "quiet" mode

Exit criteria:
- returning to a paused task produces an unprompted re-orientation
  message within 3 seconds of focus
- drift detection fires on a curated set of drift scenarios and
  does not fire on a curated set of on-track scenarios
- speculative subtasks never apply changes; results are always
  offered, never auto-merged
- quiet mode suppresses all proactive surfaces (audio, overlay,
  notifications) until disabled
- phase15_check.sh verifies throttling, quiet-mode suppression,
  and the drift true/false-positive scenario suite

---

## Phase 16: Richer Parallel Work

Subtasks become real parallel work units: multi-step chains, the
ability to write to files (bounded, with approval), and tool use
inside the subtask sandbox.

Scope:
- multi-step subtask chains with intermediate state visible to user
- bounded file writes from inside a subtask, gated by explicit
  approval card in companion view
- tool use inside subtasks (retrieval, structured generation, the
  small set of tools already present in the runtime)
- per-subtask resource and step limits
- cancellation and rollback semantics for partial chains
- audit log of every file write proposed and every approval decision

Exit criteria:
- a multi-step subtask chain runs to completion with intermediate
  steps visible and individually cancellable
- no file write reaches disk without an explicit approval action;
  ignore/dismiss leaves the filesystem unchanged
- cancellation mid-chain rolls back to the last clean checkpoint
  with no partial artifacts left behind
- tool use inside a subtask honors the same safety boundaries as
  the top-level runtime (no external browsing, single-task scope)
- phase16_check.sh verifies approval-gating of writes, rollback
  integrity, and step/resource limit enforcement
