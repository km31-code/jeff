# Jeff — Transformation Plan (Phases 25–30)

This document is the authoritative execution plan for transforming Jeff from a
well-built ambient interface layer into the entity described in VISION.md.

Read VISION.md, CHARACTER.md, and SYNTHESIS_ARCHITECTURE.md before executing
any phase. These documents are the specification. The phases serve them.

---

## What is being transformed and why

Jeff Phases 0–24 built a genuine technical foundation: tray presence, streaming
pipeline, workspace awareness, proactive triggers, subtask engine, user model,
live app actions, distribution. That foundation is real and must be preserved.

What is missing is the center of the system. Jeff has capabilities and no
character. Jeff assembles context and holds no continuous model of the user's
situation. Jeff fires notifications and has no judgment about when something is
worth saying. Jeff delivers outputs and has no view on them. Jeff observes
behavioral patterns and builds no relational understanding.

These six gaps are addressed by the first six phases (25–30). Two additional gaps — live content observation and character consistency enforcement — are addressed by Phases 31 and 32, appended below.

---

## Non-negotiable constraints (inherited from Phases 17–24)

- Local-first. No user data leaves the device unless the user explicitly connects
  an external service.
- No silent writes. All file writes remain explicitly approval-gated.
- New sensing must be opt-in, scoped to active task, and surfaced in the
  Privacy Center.
- Every phase ships with a `scripts/phaseN_check.sh` that verifies runtime
  behavior, not only symbol presence.
- Preserve the five felt properties from VISION.md as the north star for every
  decision.

---

## Sequencing rationale

```
25 → 26 → 27 → 28 → 29 → 30 → 31 → 32 → 33 → 34
```

- Phase 25 (Character) first: character is the center of the system. Every
  subsequent phase produces outputs that must express it. Building proactivity
  or synthesis without character first produces the wrong outputs.

- Phase 26 (Awareness Core) before synthesis: the synthesis layer consumes the
  `SituationalSnapshot`. The snapshot must exist before judgment can be applied.

- Phase 27 (Synthesis) before proactivity redesign: the conversation-shaped
  proactivity in Phase 28 depends on the synthesis judgment — it delivers what
  the synthesis layer decides to say.

- Phase 28 (Conversation-shaped proactivity) before stakes in outputs: once
  Jeff speaks from judgment rather than triggers, the stakes design (Phase 29)
  can be built consistently.

- Phase 29 (Opinionated output) before relational model: the relational model
  (Phase 30) adjusts how Jeff expresses opinions based on trust signals. The
  opinion system must exist before the adjustment layer can be built.

- Phase 30 (Relational model) before content observation: the relational model
  is the highest interpretation layer. Phase 31 adds new sensing capability; the
  non-negotiable constraint is that new sensing comes after interpretation layers
  are in place, not before. Phase 30's struggle pattern detection also benefits
  directly from Phase 31 signals — "stuck on the same paragraph" becomes
  detectable from content observation, which feeds struggle pattern recording.

- Phase 31 (Live Content Observation) after Phase 30: extends the
  `SituationalSnapshot` from Phase 26 with content-level signals. All
  interpretation layers — character, synthesis judgment, relational model —
  must exist before a new sensing layer is introduced. Phase 31 also activates
  the `WorkQualityObservation` proactive speech reason reserved in Phase 27.

- Phase 32 (Character Consistency Enforcement) after Phase 31: the eval harness
  is most valuable when all output surfaces are wired, including the content-
  observation-informed surfaces from Phase 31. Running it earlier would validate
  fewer paths. Phase 32 retroactively extends Phase 25's exit criteria and becomes
  the quality gate before write-back and retrieval are added.

- Phase 33 (Native Document Write-Back) before retrieval: write-back must exist
  before retrieval because the primary value of retrieved content — sources,
  research — is incorporating it into the document. If retrieval lands without
  write-back, the user is still manually copying sourced content into their
  document. The full loop — find it, draft with it, place it — requires both,
  and write-back is the harder infrastructure problem.

- Phase 34 (Web Retrieval) after Phase 33: with write-back working, retrieved-
  and-drafted content can flow directly from the subtask agent into the document
  with a single approval. That is the complete version of "find evidence for this
  paragraph and handle it while I keep going."

---

## Phase 25: Character Operationalization

**Gap addressed:** Gap 1 — Character is entirely unspecified.

**Why this phase exists:** Everything Jeff says is wrong if there is no consistent
character behind it. Proactivity, synthesis, opinionated output — all three
require a defined voice to express them. This phase creates that voice as a
first-class code artifact and wires it into every output path.

**This phase is two things:** a design document (CHARACTER.md, already written)
and a code change that enforces the character across all prompt assembly sites.

---

### Scope

1. Create `desktop/src-tauri/src/character.rs` — the single module responsible
   for assembling all system prompts. No other module assembles a system prompt
   from scratch after this phase.

2. `base_character_prompt() -> &'static str`: returns the character instruction
   block. Hardcoded, not LLM-generated. Under 300 tokens. Contains:
   - Direct voice instruction (no filler phrases, first person, terse)
   - Opinion-before-result instruction (see Phase 29 for full surface, but the
     foundation prompt is here)
   - Uncertainty handling (one clause, keep moving)
   - Disagreement style (direct, once, then defer)

3. System prompt builders (all take a context struct, call `base_character_prompt()`
   internally, return `String`):
   - `build_chat_system_prompt(ctx: &ChatContext) -> String`
   - `build_revision_system_prompt(ctx: &RevisionContext) -> String`
   - `build_reorientation_system_prompt(ctx: &ReorientationContext) -> String`
   - `build_subtask_system_prompt(ctx: &SubtaskContext) -> String`
   Context structs are thin wrappers over existing fields already available
   in the respective modules.

4. Update all prompt assembly sites:
   - `chat.rs`: replace inline system prompt with `character::build_chat_system_prompt()`
   - `revision.rs`: replace inline system prompt with `character::build_revision_system_prompt()`
   - `proactive.rs` `REORIENTATION_SYSTEM_PROMPT`: replace with
     `character::build_reorientation_system_prompt()`
   - `subtask.rs` planning and execution prompts: replace with
     `character::build_subtask_system_prompt()`

5. `strip_filler_phrases(text: &str) -> String` in `character.rs`:
   Removes filler phrases from all text output (not just TTS — extends Phase 22).
   Phrases stripped: "Certainly,", "Absolutely,", "Of course,", "Great question!",
   "Sure thing,", "Happy to help!", "I'd be happy to", "I'll go ahead and",
   "I've gone ahead and". String replacement only — no LLM call.
   Called before any text response reaches the frontend (in `chat_streaming.rs`
   on final token assembly and in `subtask.rs` on result storage).

6. Assessment instruction as part of `base_character_prompt()`: "Before presenting
   a result (revision, draft, subtask completion), write one sentence about the
   judgment you made — the tradeoff, what's stronger, what's weaker. First person.
   No hedging. Example: 'Moved the argument to the front — loses the setup but
   lands faster.' Then the result."

---

### Implementation checklist

- [ ] Create `desktop/src-tauri/src/character.rs`
- [ ] Implement `base_character_prompt() -> &'static str` — hardcoded character
      instruction block, ≤ 300 tokens. Include: voice rules, no-filler-phrases,
      assessment-before-result instruction, uncertainty handling, disagreement style
- [ ] Define `ChatContext` struct: `task_summary: String`, `active_window: Option<String>`,
      `profile_injection: Option<String>`, `recent_transcript: Vec<String>`
- [ ] Define `RevisionContext` struct: `task_summary: String`, `target_description: String`,
      `instruction: String`, `profile_injection: Option<String>`
- [ ] Define `ReorientationContext` struct: `task_summary: String`, `last_active: String`,
      `profile_injection: Option<String>`
- [ ] Define `SubtaskContext` struct: `task_summary: String`, `subtask_title: String`,
      `execution_type: String`, `profile_injection: Option<String>`
- [ ] Implement `build_chat_system_prompt(ctx: &ChatContext) -> String`
- [ ] Implement `build_revision_system_prompt(ctx: &RevisionContext) -> String`
- [ ] Implement `build_reorientation_system_prompt(ctx: &ReorientationContext) -> String`
- [ ] Implement `build_subtask_system_prompt(ctx: &SubtaskContext) -> String`
- [ ] Implement `strip_filler_phrases(text: &str) -> String` with all six phrase
      patterns listed in scope above
- [ ] Add `character` module to `lib.rs`
- [ ] Update `chat.rs`: replace inline `system_prompt` string with call to
      `character::build_chat_system_prompt(ChatContext { ... })`
- [ ] Update `chat_streaming.rs`: call `strip_filler_phrases()` on final assembled
      text before it is stored and returned
- [ ] Update `revision.rs`: replace inline system prompt with
      `character::build_revision_system_prompt(RevisionContext { ... })`
- [ ] Update `proactive.rs` `generate_reorientation`: replace
      `REORIENTATION_SYSTEM_PROMPT` with
      `character::build_reorientation_system_prompt(ReorientationContext { ... })`
- [ ] Update `subtask.rs` chain planning prompt: replace with
      `character::build_subtask_system_prompt(SubtaskContext { ... })`
- [ ] Update `subtask.rs` result storage: call `strip_filler_phrases()` on
      `result_summary` before storing
- [ ] Write unit test `base_character_prompt_under_300_tokens`: token count
      heuristic (≤ 1200 chars as proxy) passes
- [ ] Write unit test `strip_filler_phrases_removes_all_patterns`: each phrase
      pattern is removed, surrounding text preserved
- [ ] Write unit test `chat_system_prompt_contains_character_block`: output of
      `build_chat_system_prompt` contains the base character prompt text
- [ ] Write `scripts/phase25_check.sh`:
      - `character.rs` file exists
      - `base_character_prompt` function exported
      - `strip_filler_phrases` function exported
      - `chat.rs` no longer contains a hardcoded system prompt string
        (grep for old prompt constant — should be absent)
      - `revision.rs` calls `character::build_revision_system_prompt`
      - behavioral assertion: send a test message and verify the response
        does not begin with "Certainly" (requires a live API key in CI)
- [ ] Run `scripts/character_eval.sh` after all prompt paths are wired; verify
      at least 13 of 15 sampled cases pass before marking Phase 25 complete

---

### Exit criteria (all behavioral)

1. Jeff responds to "summarize my notes" with the summary, not "Certainly!
   Here's a summary of your notes:". The response begins with the content.

2. Jeff proposes a revision to a paragraph. The revision card begins with
   Jeff's assessment sentence before the proposed text. The word "Certainly"
   or "Absolutely" does not appear anywhere in the output.

3. Jeff is asked "is my argument clear?" about a circular paragraph. Jeff
   says something like "No — the second sentence restates the first without
   adding anything." Not "It's a good start, but you might consider..."

4. Two different engineers given CHARACTER.md and asked to write Jeff's response
   to "I'm not sure how to start" independently produce responses that are
   recognizably the same entity: direct, brief, no flattery.

5. The `base_character_prompt()` text is verifiably under 300 tokens (checked
   by the unit test proxy).

6. Run `scripts/character_eval.sh`. At least 13 of 15 sampled cases pass
   (grader verdict agrees with labeled ground truth).

---

## Phase 26: Awareness Core — Persistent Situational Model

**Gap addressed:** Gap 3 — Context is assembled per-turn, not held continuously.

**Why this phase exists:** Jeff currently has no component that maintains a
coherent model of where the user is in their work across time. Retrieval answers
"what files are relevant?" The snapshot answers "what is happening right now?"
These are different questions. The snapshot is what enables genuine situational
awareness — and it is the foundation for every subsequent phase.

The full specification of the data structures and assembly logic is in
`docs/SYNTHESIS_ARCHITECTURE.md`. This phase builds exactly what is described
there.

---

### Scope

1. Create `desktop/src-tauri/src/awareness_core.rs` with all structs and enums
   specified in SYNTHESIS_ARCHITECTURE.md:
   - `SituationalSnapshot` (all 9 fields)
   - `AttentionState` enum (Focused, Drifting, Returning, Idle)
   - `PendingItem` struct
   - `TimePressure` struct
   - `SnapshotTrigger` enum (6 variants)
   - `AwarenessCore` struct: holds `tokio::sync::Mutex<SituationalSnapshot>`

2. Implement `AwarenessCore::update(trigger, task_id, state, ambient)`:
   - Deterministic assembly — no LLM call
   - All assembly logic per SYNTHESIS_ARCHITECTURE.md
   - Acquires mutex, updates snapshot, releases

3. Implement `snapshot_summary(snapshot) -> String`:
   - Under 150 tokens
   - Returns empty string when `snapshot_confidence < 0.3`

4. Add `awareness_core: Arc<AwarenessCore>` to `AppState` in `state.rs`.
   Initialize in `main.rs` with a default empty snapshot.

5. Wire trigger calls at all five trigger points:
   - `chat.rs` after each streaming turn completes: `SnapshotTrigger::NewTurn`
   - `proactive.rs` in `record_task_focus` command: `SnapshotTrigger::FocusEvent`
   - `context_observer.rs` when document title changes: `SnapshotTrigger::WindowSwitch`
   - `subtask.rs` when chain reaches terminal state: `SnapshotTrigger::SubtaskCompleted`
   - `main.rs` ambient monitor tick (60s): `SnapshotTrigger::TimeTick`

6. Update `character::build_chat_system_prompt()` to call
   `awareness_core.snapshot_summary()` and inject the result as a context block
   after the character prompt and before the task summary. Only inject when
   `snapshot_confidence >= 0.3`.

7. Add tauri command `get_situational_snapshot` → returns serialized
   `SituationalSnapshot` for debugging. Gated by debug mode.

---

### Implementation checklist

- [ ] Create `desktop/src-tauri/src/awareness_core.rs`
- [ ] Define `SituationalSnapshot` struct with all 9 fields (types per
      SYNTHESIS_ARCHITECTURE.md)
- [ ] Implement `SituationalSnapshot::default()` — all None/empty, confidence 0.0
- [ ] Define `AttentionState` enum with 4 variants, derive Serialize/Deserialize
- [ ] Define `PendingItem` struct: `item_type: String, description: String, created_at: i64`
- [ ] Define `TimePressure` struct: `source: String, description: String, minutes_until: Option<i64>`
- [ ] Define `SnapshotTrigger` enum with 6 variants
- [ ] Define `AwarenessCore` struct: `snapshot: tokio::sync::Mutex<SituationalSnapshot>`
- [ ] Implement `AwarenessCore::new() -> Self`
- [ ] Implement `AwarenessCore::update(trigger, task_id, state, ambient)`:
      - `current_goal`: scan last 10 messages for goal-statement patterns using
        string matching on these prefixes: "i'm working on", "i need to", "i'm
        trying to", "my goal is", "i want to". take most recent match. fall back
        to task title.
      - `recent_progress`: if last assistant message `message_kind` is
        "subtask_result" or "revision_accepted", use its `content` field truncated
        to 80 chars
      - `attention_state`: Returning if `now - last_focus_at > 300`; Drifting if
        last `proactive_trigger_log` entry of type "drift" is within 900s; Focused
        if last message within 120s; Idle otherwise
      - `pending_work`: query `subtask_file_write_proposals WHERE status =
        'pending_approval' AND task_id = ?` and `subtasks WHERE
        result_review_status = 'unreviewed' AND task_id = ?`
      - `time_pressure`: check CalendarState for event within 120 minutes; scan
        last 20 messages for deadline patterns "by midnight", "by tomorrow",
        "deadline is", "due at"
      - `last_meaningful_turn`: most recent message `created_at` as unix timestamp
      - `snapshot_confidence`: additive scoring per SYNTHESIS_ARCHITECTURE.md
- [ ] Implement `snapshot_summary(snapshot: &SituationalSnapshot) -> String` —
      returns empty string when confidence < 0.3, otherwise natural language block
      under 150 tokens
- [ ] Add `awareness_core: Arc<AwarenessCore>` to `AppState` struct in `state.rs`
- [ ] Initialize `AwarenessCore::new()` in `main.rs` before app setup
- [ ] Wire `SnapshotTrigger::NewTurn` in `chat.rs`: after streaming completes and
      message is stored, call `app_state.awareness_core.update(NewTurn, task_id, ...)`
      via `spawn` (non-blocking, does not gate the response)
- [ ] Wire `SnapshotTrigger::FocusEvent` in `proactive.rs` `record_task_focus`:
      after writing to `task_focus_log`, spawn snapshot update
- [ ] Wire `SnapshotTrigger::WindowSwitch` in `context_observer.rs` when
      `document_title` changes between poll intervals
- [ ] Wire `SnapshotTrigger::SubtaskCompleted` in `subtask.rs` when chain enters
      a terminal state (`completed` or `cancelled`)
- [ ] Wire `SnapshotTrigger::TimeTick` in `main.rs` ambient monitor (the existing
      60s loop from VISION_ALIGNMENT section C)
- [ ] Update `character.rs` `build_chat_system_prompt()`: add
      `awareness_core.snapshot_summary()` call; inject after character block,
      before task summary; only when confidence >= 0.3
- [ ] Update `character.rs` `build_reorientation_system_prompt()`: inject
      snapshot summary
- [ ] Add debug tauri command `get_situational_snapshot(task_id)` behind
      `#[cfg(debug_assertions)]` gate
- [ ] Write unit test `snapshot_confidence_zero_with_no_signals`: default
      snapshot has confidence 0.0
- [ ] Write unit test `attention_state_returning_after_five_minutes`: mock
      last_focus_at = now - 360, assert Returning
- [ ] Write unit test `attention_state_focused_with_recent_message`: mock
      last message within 60s, assert Focused
- [ ] Write unit test `snapshot_summary_empty_when_low_confidence`: confidence
      0.2 → empty string returned
- [ ] Write unit test `snapshot_summary_under_150_tokens`: full snapshot with
      all fields populated produces string under 600 chars (token proxy)
- [ ] Write unit test `goal_extracted_from_im_working_on_message`: message
      "i'm working on the introduction" → current_goal = "the introduction"
- [ ] Write `scripts/phase26_check.sh`:
      - `awareness_core.rs` exists
      - `SituationalSnapshot`, `AttentionState`, `AwarenessCore` symbols present
      - `AppState` struct in `state.rs` contains `awareness_core` field
      - `chat.rs` contains a call to `awareness_core` (update trigger)
      - `proactive.rs` contains a call to `awareness_core`
      - `character.rs` `build_chat_system_prompt` calls `snapshot_summary`
      - behavioral assertion: after 3 turns about "writing the introduction",
        `get_situational_snapshot` returns `current_goal` containing "introduction"

---

### Exit criteria (all behavioral)

1. After a conversation turn where the user says "I'm trying to finish the
   introduction," calling `get_situational_snapshot` shows `current_goal`
   containing "introduction." Response latency is not perceptibly increased
   (update is non-blocking).

2. Jeff's response to "what should I focus on?" in the middle of an active
   session correctly references the current goal and any pending items — drawn
   from the snapshot injected into the system prompt, not only from retrieval.

3. After 7 minutes without opening Jeff, the snapshot shows
   `attention_state: Returning`. Jeff's next response reflects this
   ("you've been away for a bit") without being told.

4. With no active task and no prior messages, `snapshot_confidence` is < 0.3
   and no snapshot summary is injected into the system prompt (verified by
   inspecting the prompt assembly in a debug run).

5. The snapshot update on a new conversation turn adds < 50ms to total turn
   latency (it runs as a background spawn, not blocking the response stream).

---

## Phase 27: Synthesis Layer — Situational Intelligence

**Gap addressed:** Gap 6 — No synthesis layer exists.

**Why this phase exists:** The snapshot (Phase 26) is the structured model of
current state. The synthesis layer is the judgment function that reads it and
decides: is there something worth saying? This phase replaces three independent
trigger evaluators with a single synthesis function and adds the integration
layer that makes multi-signal reasoning possible.

---

### Scope

1. Add to `awareness_core.rs`:
   - `ProactiveSpeechReason` enum (4 variants per SYNTHESIS_ARCHITECTURE.md)
   - `fn should_speak_proactively(snapshot, profile, last_proactive_at, now) -> Option<ProactiveSpeechReason>`
     Full decision logic per SYNTHESIS_ARCHITECTURE.md.
   - `async fn synthesize_proactive_message(reason, snapshot, api_key) -> Result<String>`
     LLM call, 1-2 sentence budget, character reorientation prompt.

2. Create `synthesis_log` DB table (per SYNTHESIS_ARCHITECTURE.md schema).

3. Add `log_synthesis_decision(store, task_id, reason, snapshot, message, delivered)`.

4. Update the ambient monitor in `main.rs`: replace the three separate calls
   (`check_reorientation_from_background`, `check_stuck_from_background`,
   `check_drift_from_background`) with a single `run_synthesis_check(state, ambient, app)`
   function that:
   a. Gets the current snapshot from `awareness_core`
   b. Calls `should_speak_proactively()`
   c. If Some reason returned: calls `synthesize_proactive_message()`
   d. Delivers via `proactive::deliver_proactive_as_chat_message()` (built in Phase 28)
      or via native notification if overlay is hidden
   e. Logs to `synthesis_log`
   f. All within the quiet mode check

---

### Implementation checklist

- [ ] Define `ProactiveSpeechReason` enum in `awareness_core.rs`:
      `TaskReturn { idle_minutes: u64 }`,
      `DeadlinePressure { event: String, minutes_until: i64 }`,
      `BlockerDetected { blocker: String }`,
      `WorkQualityObservation { observation: String }` (reserved, not triggered yet)
- [ ] Implement `should_speak_proactively(snapshot, profile, last_proactive_at, now) -> Option<ProactiveSpeechReason>`:
      - Return None if confidence < 0.3
      - Return None if within cooldown (600s default; read from profile trigger weights)
      - Check trigger_weight_reorientation from user_model: if < 0.5 (repeated
        dismissals), raise idle threshold from 300s to 600s
      - Return TaskReturn if attention_state == Returning AND idle gap > threshold
      - Return DeadlinePressure if time_pressure.minutes_until < 90
      - Return BlockerDetected if current_blockers non-empty AND last turn > 600s ago
      - Return None otherwise
- [ ] Implement `async fn synthesize_proactive_message(reason, snapshot, api_key) -> Result<String>`:
      - Build system prompt: `character::build_reorientation_system_prompt()` with
        snapshot summary injected
      - Build user prompt: brief description of reason + key snapshot fields
      - Instruction: "In 1-2 sentences, speak as a coworker who has been watching.
        Reference the specific situation. Do not be a notification. Start a
        conversation. Maximum 40 words."
      - Call gpt-4o-mini with request timeout 5s
      - Strip filler phrases from result
- [ ] Create `synthesis_log` table migration in `store.rs`:
      `id, task_id, reason_type, reason_detail, snapshot_confidence,
       snapshot_attention_state, message, delivered INT, delivered_at, created_at`
- [ ] Implement `log_synthesis_decision(store, task_id, Option<ProactiveSpeechReason>,
      snapshot_confidence, attention_state, Option<String> message, delivered: bool)`
- [ ] Add `get_last_synthesis_at(store, task_id) -> Option<i64>` — reads most recent
      `delivered = 1` row from `synthesis_log` for the cooldown check
- [ ] Create `fn run_synthesis_check(state: Arc<AppState>, ambient: Arc<AmbientState>,
      app: AppHandle)` in `main.rs` or a new `synthesis.rs`:
      a. Get current snapshot via `state.awareness_core.snapshot()`
      b. Read user profile from store
      c. Get last synthesis timestamp via `get_last_synthesis_at`
      d. Call `should_speak_proactively`; log to synthesis_log regardless of result
      e. If None: return
      f. Call `synthesize_proactive_message()`; if Err: log failure, return
      g. Deliver: if overlay visible, emit proactive event; if hidden, dispatch
         notification with synthesized text as body
      h. Log `delivered = true` in synthesis_log
- [ ] Update `main.rs` ambient monitor: replace the three separate check calls
      with a single `run_synthesis_check()` call
- [ ] Remove `check_reorientation_from_background`, `check_stuck_from_background`,
      `check_drift_from_background` from the ambient monitor loop (they are now
      consolidated into `run_synthesis_check`)
- [ ] Add `JEFF_DISABLE_AMBIENT_MONITOR=1` env var check remains for CI
- [ ] Add tauri command `get_synthesis_log(task_id)` for debug/Privacy Center audit
- [ ] Update Privacy Center audit view to show synthesis_log entries (kind and
      timestamp of each proactive decision, including suppressed ones)
- [ ] Write unit test `should_speak_returns_none_when_low_confidence`: confidence
      0.2 → None
- [ ] Write unit test `should_speak_returns_task_return_after_5min_idle`:
      attention_state = Returning, idle_minutes = 6 → TaskReturn { idle_minutes: 6 }
- [ ] Write unit test `should_speak_returns_none_within_cooldown`:
      last_proactive_at = now - 300 → None
- [ ] Write unit test `should_speak_deadline_pressure_at_89_minutes`:
      time_pressure.minutes_until = 89 → DeadlinePressure
- [ ] Write unit test `should_speak_raises_threshold_after_dismissals`:
      trigger_weight_reorientation = 0.3 → idle threshold becomes 600s;
      idle_minutes = 6 → None (below raised threshold)
- [ ] Write `scripts/phase27_check.sh`:
      - `ProactiveSpeechReason` enum in `awareness_core.rs`
      - `should_speak_proactively` function
      - `synthesize_proactive_message` function
      - `synthesis_log` table in a store.rs migration
      - `run_synthesis_check` in `main.rs` or `synthesis.rs`
      - The three old check functions are absent from the ambient monitor loop
      - behavioral assertion: with quiet mode on, `synthesis_log` records the
        decision but `delivered = 0`

---

### Exit criteria (all behavioral)

1. User has been away 8 minutes, has a calendar event in 40 minutes, and has
   an open file write proposal. Jeff produces one message: "You've been away
   for a bit — you have a meeting in 40 minutes and the outline.md write is
   still waiting for your decision." This is not three notifications.

2. With `snapshot_confidence < 0.3` (no active task, no recent messages): no
   proactive message is generated, and `synthesis_log` records the suppression.

3. Within 10 minutes of a delivered proactive message: no new proactive message
   fires, regardless of state. `synthesis_log` records the cooldown suppression.

4. After user dismisses 5 proactive messages: `trigger_weight_reorientation`
   is down-weighted in the user model, and the idle threshold for TaskReturn
   raises from 5 to 10 minutes (verified: user must be away 10 minutes before
   Jeff speaks proactively).

5. `synthesis_log` shows all decisions for the session: delivered and suppressed.
   The Privacy Center audit view shows this log to the user.

---

## Phase 28: Conversation-Shaped Proactivity

**Gap addressed:** Gap 2 — Proactivity is notification-shaped, not conversation-shaped.

**Why this phase exists:** The synthesis judgment (Phase 27) produces a message
worth saying. This phase changes how that message is delivered. A chat bubble that
the user can reply to is fundamentally different from a dismissible banner or a
push notification. One is a coworker saying something; the other is a system pinging
you. The mechanic is the same; the experience is completely different.

---

### Scope

1. In `proactive.rs`: add `deliver_proactive_as_chat_message(store, app_handle, task_id, message, kind)`:
   - Inserts a message into the conversation via `store.insert_chat_message(task_id, "assistant", kind, message)`
   - Emits `proactive://message_inserted` event to the frontend
   - `kind` values: `"proactive_reorientation"`, `"proactive_drift"`,
     `"proactive_blocker"`, `"proactive_deadline"`

2. Update `run_synthesis_check` (Phase 27) to call `deliver_proactive_as_chat_message`
   when overlay is visible, and `ambient::dispatch_notification` with the synthesized
   message text when overlay is hidden.

3. In `Overlay.tsx`: subscribe to `proactive://message_inserted`. On receive, call
   `loadMessages()` to refresh the message list. The proactive message appears as
   a standard assistant message bubble. The user can reply to it by sending a new
   message — the conversation continues naturally.

4. Message kind styling in `Overlay.tsx`: for assistant messages with a `proactive_*`
   kind, render the bubble with a dim-white left border instead of the normal accent-
   purple border. This gives the user a visual cue that Jeff initiated, without making
   it feel like a separate UI component.

5. Remove the `reorientation-banner` component from `Overlay.tsx`. Remove the
   `drift-banner` component from `Overlay.tsx`. These surfaces are replaced by chat
   bubbles. The banner state variables (`showReorientationBanner`, `driftFlag`, etc.)
   are removed.

6. The speculative subtask offer card: already in the companion as a message-stream
   card (from VISION_ALIGNMENT section E4). Keep it. Deliver it via the same
   `deliver_proactive_as_chat_message` path with `kind = "proactive_speculative_subtask"`.
   Accept/dismiss buttons remain in the card.

7. Native notification path: the body of the notification is the synthesized message
   text. Not a canned template. The title remains "jeff" (lowercase).

8. Notification click: on `ambient://notification-click` with proactive kind, load
   messages and scroll to the most recent proactive message in the chat list.
   Remove the "opened from notification" context banner.

---

### Implementation checklist

- [x] In `proactive.rs`: implement `pub async fn deliver_proactive_as_chat_message(store, app, task_id: i64, message: &str, kind: &str) -> Result<()>`:
      calls `store.insert_chat_message(task_id, "assistant", kind, message)`,
      emits `proactive://message_inserted` via `app.emit_all()`
- [x] Update `run_synthesis_check` (from Phase 27): when overlay is visible, call
      `deliver_proactive_as_chat_message`; when hidden, call `ambient::dispatch_notification`
      with synthesized text as body
- [x] Ensure `store.insert_chat_message` accepts `message_kind` as a parameter
      (it currently does per models.rs — verify no schema change needed)
- [x] In `Overlay.tsx`: add `proactive://message_inserted` event listener in
      `useEffect`; on receive, call `loadMessages(activeTask.id)`
- [x] In `Overlay.tsx`: add conditional styling for `message.message_kind.startsWith("proactive_")`:
      use `border-left: 2px solid rgba(255,255,255,0.18)` instead of accent-purple
      for the left border on the message bubble. No other visual change.
- [x] In `Overlay.tsx`: remove `showReorientationBanner` state and the banner
      component that uses it (approximately lines rendering `overlay-banner-info`
      for reorientation context)
- [x] In `Overlay.tsx`: remove `driftFlag` state and the banner component that
      uses it
- [x] In `Overlay.tsx`: remove the call to `triggerTaskResume()` on
      `ambient://overlay-shown` (this was the remaining hook into the old banner
      path). `recordTaskFocus()` still runs on overlay-shown.
- [x] In `Overlay.tsx`: on `ambient://notification-click` with kind containing
      "reorientation" or "proactive": call `loadMessages()` and scroll to last
      proactive message. Remove the "opened from notification" banner display.
- [x] In `ambient.rs` `dispatch_notification`: when called from `run_synthesis_check`,
      the message text is passed as the notification body directly (not a canned
      template)
- [x] Update `TESTING_GUIDE.md` Scenario E: step E3 now says "A native macOS
      notification appears" and step E4 says "The companion expands and the most
      recent message in the chat is Jeff's reorientation" (not "shows the
      reorientation context card")
- [x] Write unit test `deliver_proactive_inserts_message_with_correct_kind`:
      mock store, call `deliver_proactive_as_chat_message`, assert message_kind
      is stored correctly
- [x] Write `scripts/phase28_check.sh`:
      - `deliver_proactive_as_chat_message` in `proactive.rs`
      - `proactive://message_inserted` event emitted in `proactive.rs`
      - No `showReorientationBanner` state in `Overlay.tsx`
      - No `driftFlag` state in `Overlay.tsx`
      - `ambient://notification-click` handler does not render a banner
      - behavioral assertion: after a proactive message is delivered, loading
        messages returns an assistant message with `message_kind` starting with
        "proactive_"

---

### Exit criteria (all behavioral)

1. User has been away 6 minutes. Opens the companion. The most recent item in
   the message list is a chat bubble from Jeff. It is styled with a dim-white
   left border. The user can reply to it directly and the conversation continues.

2. No banner appears — not for reorientation, not for drift. All Jeff-initiated
   content arrives as chat bubbles.

3. A native macOS notification fires after 6 minutes away. The notification body
   is the exact synthesized message text ("You left off mid-argument in paragraph 3
   — want to pick up from there?"). Not "jeff noticed you were away."

4. Clicking the notification expands the companion and the chat list is scrolled
   to the proactive message. No additional banner appears.

5. The user ignores a proactive chat bubble and sends a new message. The
   conversation continues normally from the new message. Jeff does not reference
   the ignored message or ask for a response.

---

## Phase 29: Opinionated Output

**Gap addressed:** Gap 4 — Jeff has no stakes in the outcome.

**Why this phase exists:** Jeff currently delivers results and disappears. The
character spec (Phase 25) establishes the voice. This phase applies that character
specifically to output surfaces — every place where Jeff delivers a result must
include Jeff's view on that result. This is not about adding more words; it is
about changing the orientation of every output from "task completed" to "here is
what I produced and what I think of it."

---

### Scope

1. `base_character_prompt()` already contains the assessment instruction (Phase 25).
   This phase ensures every output surface actually renders the assessment correctly.

2. In `revision.rs`: parse the LLM output to detect and extract the assessment
   sentence. The assessment is the first sentence if: it does not contain a revision
   marker (no "original:", no "proposed:"), is 1-2 sentences, is first person.
   Store in `rationale` field of `RevisionProposalDto`.

3. In `Overlay.tsx` revision proposal card: move `proposal.rationale` to the top
   of the card — rendered before the proposed text, styled as dim text
   (`var(--text-dim)`, italic). Not below it.

4. In `Overlay.tsx` subtask offer card: render `subtask.result_summary` (the
   assessment sentence Jeff produced) as the first line of the card, before the
   result excerpt. Same dim text styling.

5. `generate_revision_alternative(revision_id)` command in `revision.rs`:
   re-runs the revision with a modified instruction "Generate the alternative
   approach to what you described in your assessment. The prior revision was: [prior
   rationale]. Now take the other path." Creates a new `RevisionProposalDto` linked
   to the original via a `parent_revision_id` field. (Add `parent_revision_id
   INTEGER` column to `revisions` table via idempotent `ALTER TABLE`.)

6. In `Overlay.tsx` revision card: add "see alternative" button when `proposal.rationale`
   is non-empty and does not already have a linked alternative. Calls
   `generateRevisionAlternative(proposalId)`. When complete, shows the alternative
   card inline below the original.

7. In `chat.rs` / `streaming.rs`: for responses that are suggestions (intent
   `Suggestion`), the character prompt already instructs assessment-first. No
   additional parsing needed — the assessment appears as the first sentence of
   the response. The frontend renders this naturally.

---

### Implementation checklist

- [x] In `revision.rs`: after receiving LLM output, call `extract_assessment_sentence(output: &str) -> Option<String>`:
      heuristic: if the first sentence (up to first `.` or `?` or `!`) is
      under 120 chars, does not contain markdown formatting, and contains
      first-person pronouns ("I ", "my "), extract it as assessment.
      Store as `rationale` on the RevisionProposalDto.
- [x] In `revision.rs`: store the result of `extract_assessment_sentence` in the
      `rationale` column of the revisions table (this column already exists per
      models.rs `RevisionProposalDto.rationale: Option<String>`)
- [x] In `Overlay.tsx` revision proposal card: render `proposal.rationale` as
      first element in the card, above the diff/proposed text. Style: `font-style:
      italic; color: var(--text-dim); font-size: 12px; margin-bottom: 6px;`. Only
      render when `proposal.rationale` is non-null and non-empty.
- [x] In `Overlay.tsx` subtask offer card: render `subtask.result_summary` as
      first element before the result excerpt. Same dim italic styling. Only
      when non-null.
- [x] Add `parent_revision_id INTEGER` column to `revisions` table via idempotent
      `ALTER TABLE IF NOT EXISTS revisions ADD COLUMN parent_revision_id INTEGER;`
      migration in `store.rs`
- [x] Add `parent_revision_id: Option<i64>` field to `RevisionProposalDto` in `models.rs`
- [x] Implement `generate_revision_alternative(revision_id: i64, store, api_key) -> Result<RevisionProposalDto>`:
      loads original revision's rationale and proposed text; calls revision LLM
      with instruction to take the alternative approach described in the rationale;
      stores new proposal with `parent_revision_id = original.id`
- [x] Add tauri command `generate_revision_alternative(task_id, revision_id)`
- [x] In `Overlay.tsx` revision card: add "see alternative" button (ghost style,
      small) when `proposal.rationale` is non-empty AND no sibling alternative
      exists yet. Clicking calls `generateRevisionAlternative()`. While loading,
      show spinner in button. On complete, render new card inline below.
- [x] Write unit test `extract_assessment_sentence_extracts_first_person_sentence`:
      input "I moved the argument to the front. The conclusion now:\n\nYour new
      argument..." → extracted: "I moved the argument to the front."
- [x] Write unit test `extract_assessment_sentence_returns_none_for_no_first_person`:
      input "The revision restructures the paragraph." → None (no first person)
- [x] Write unit test `revision_system_prompt_includes_assessment_instruction`:
      `build_revision_system_prompt()` output contains the assessment instruction
      from `base_character_prompt()`
- [x] Write `scripts/phase29_check.sh`:
      - `extract_assessment_sentence` function in `revision.rs`
      - `parent_revision_id` column in revisions migration
      - `generate_revision_alternative` in commands list
      - `Overlay.tsx` renders `proposal.rationale` before proposed text
        (grep for rationale render before proposed-text render)
      - behavioral assertion: request a revision, verify returned proposal has
        non-empty `rationale` field

---

### Exit criteria (all behavioral)

1. Jeff proposes a revision to a paragraph. The revision card shows Jeff's
   assessment first: "Moved the lead claim to the front — loses the setup but
   the argument lands faster." Then below it: the proposed text. Never the
   proposed text with the assessment below or absent.

2. Jeff completes a subtask draft. The offer card shows the assessment sentence
   first: "Went direct — no setup, straight to the argument." Then the draft
   excerpt.

3. Jeff's response to a suggestion request ("how should I open this essay?")
   begins with a direct statement: "Start with the claim, not the context." Not
   "There are several approaches you might consider."

4. Click "see alternative" on a revision card. A second card appears with a
   different approach. The second card also has its own assessment sentence
   describing what's different about the approach.

5. Jeff produces a subtask result with zero assessment. (This should not happen.)
   If it does, verify the subtask system prompt was assembled via `character.rs`
   and includes the assessment instruction. Fix the system prompt if not.

---

## Phase 30: Relational Understanding

**Gap addressed:** Gap 5 — The user model is observational, not relational.

**Why this phase exists:** The behavioral user model (Phase 23) tells Jeff how
the user communicates. The relational model tells Jeff what the user is trying to
accomplish and how they like to collaborate. These are different layers. Without
the relational model, Jeff is calibrated to your style but does not know you. With
it, Jeff tracks your actual goals across sessions, recognizes when you keep hitting
the same wall, and adjusts how it expresses opinions based on whether you want to
hear them.

---

### Scope

1. Create `desktop/src-tauri/src/relational_model.rs`:
   - `StatedGoal` struct and `GoalStatus` enum
   - `StrugglePattern` struct
   - `CollaborationStyle` struct (prefers_opinions, wants_explanations,
     delegation_comfort, interruption_tolerance — all f32 scores 0.0–1.0)
   - `TrustMetrics` struct (counter fields)
   - `RelationalProfile` struct wrapping all of the above

2. New DB tables (all migrations via idempotent `CREATE TABLE IF NOT EXISTS`
   in `store.rs`):
   - `stated_goals`: `id, task_id, goal_text, stated_at, status, updated_at`
   - `struggle_patterns`: `id, pattern_text, task_ids_json, first_seen, last_seen, occurrence_count`
   - `collaboration_style_signals`: key-value table, initialized with defaults
     on first access. Keys: `prefers_opinions` (default 0.5), `wants_explanations`
     (default 0.5), `delegation_comfort` (default 0.5), `interruption_tolerance`
     (default 0.5)
   - `trust_metrics`: single-row table with columns `times_accepted_opinion`,
     `times_pushed_back`, `times_asked_for_more`. Upserted on each signal.

3. Signal writers (all called from existing signal points — no new user-facing
   entry points):
   - `record_goal_stated(store, task_id, goal_text)`: called from `chat.rs` when
     a user message matches goal-statement patterns. Same patterns as awareness_core
     goal detection. Does not call an LLM.
   - `update_goal_status(store, goal_id, GoalStatus)`: called when task is marked
     complete, or when user says "I finished" or "done with" (string match).
   - `record_struggle(store, task_id, description)`: called from ambient monitor
     once per 24h per task when drift check has fired 3+ times in the last 7 days
     on the same task (indicating a recurring difficulty). Description is the drift
     flag reason.
   - `record_opinion_accepted(store)`: called from `user_model.rs`
     `record_revision_accepted` (already tracks acceptances). Add a call here.
   - `record_opinion_pushback(store)`: called from `user_model.rs`
     `record_revision_rewrite` (already tracks rewrites). Add a call here.
   - `record_asked_for_more(store)`: called from `chat.rs` when user message
     contains "tell me more", "explain", "why?", "can you elaborate".

4. Collaboration style updates (EMA with alpha = 0.1):
   - `prefers_opinions`: increases when user accepts Jeff's assessment without
     modification; decreases when user pushes back on an assessment
   - `wants_explanations`: increases when `record_asked_for_more` fires
   - `delegation_comfort`: use existing delegation pattern signal from user_model.rs
   - `interruption_tolerance`: increases when user engages with proactive messages;
     decreases when user dismisses them without replying

5. `build_relational_context(store) -> Option<String>`:
   - Returns None if: no stated goals AND no struggle patterns AND collaboration
     scores all at 0.5 (defaults). New user → no injection.
   - Returns a compact text block (< 80 tokens) when signals exist:
     ```
     stated goal: [most recent active goal]
     recurring struggle: [most recent struggle pattern if present]
     collaboration note: [e.g. "prefers direct opinions" or "prefers guidance
       over assertions" based on prefers_opinions score]
     ```

6. Update `character::build_chat_system_prompt()` to call `build_relational_context()`
   and inject the result after the snapshot summary and before the task summary.

7. Adjust Jeff's opinion expression based on `collaboration_style.prefers_opinions`:
   In `build_revision_system_prompt()` and `build_subtask_system_prompt()`:
   if `prefers_opinions < 0.3`: replace ASSESSMENT_INSTRUCTION with a softer
   version: "Note the key tradeoff you made in one clause. Example: '(went direct
   here)'. Do not lead with a strong opinion."
   If `prefers_opinions > 0.7`: keep the standard assertive ASSESSMENT_INSTRUCTION.

8. "Jeff knows you" panel update in `Overlay.tsx` (in the existing profile signals
   section or Privacy Center):
   - Add a "goals" section above the behavioral signals: shows active stated goals
     with their status. Each goal has a delete button.
   - Add a "patterns" section: shows struggle patterns in plain language. Each has
     a delete button.
   - "Clear all" wipes both tables and resets collaboration style to defaults.
   - The existing behavioral signals (sentence length, formality, etc.) remain
     below, now labeled "communication style" to distinguish from relational signals.

---

### Implementation checklist

Implementation status: completed in the Codex follow-up pass; verified by
`scripts/phase30_check.sh`.

- [ ] Create `desktop/src-tauri/src/relational_model.rs`
- [ ] Define `StatedGoal` struct: `id: i64, task_id: i64, goal_text: String, stated_at: String, status: GoalStatus, updated_at: String`
- [ ] Define `GoalStatus` enum: `Active`, `Completed`, `Abandoned`; derive Serialize/Deserialize with serde rename_all = "lowercase"
- [ ] Define `StrugglePattern` struct: `id: i64, pattern_text: String, task_ids_json: String, first_seen: String, last_seen: String, occurrence_count: i64`
- [ ] Define `CollaborationStyle` struct: 4 f32 fields all defaulting to 0.5
- [ ] Define `TrustMetrics` struct: `times_accepted_opinion: i64, times_pushed_back: i64, times_asked_for_more: i64`
- [ ] Define `RelationalProfile` struct wrapping StatedGoal vec, StrugglePattern vec, CollaborationStyle, TrustMetrics
- [ ] Add `stated_goals` table migration in `store.rs` (idempotent)
- [ ] Add `struggle_patterns` table migration in `store.rs`
- [ ] Add `collaboration_style_signals` table migration in `store.rs` (key-value, insert defaults on first access)
- [ ] Add `trust_metrics` table migration in `store.rs` (upsert pattern)
- [ ] Add `relational_model` module to `lib.rs`
- [ ] Implement `record_goal_stated(store, task_id, goal_text) -> Result<()>`:
      inserts to stated_goals with status Active; deduplicates: if very similar
      goal exists (simple text contains check), update updated_at instead of
      inserting
- [ ] Implement `update_goal_status(store, goal_id, status) -> Result<()>`
- [ ] Implement `record_struggle(store, task_id, description) -> Result<()>`:
      upsert: if pattern_text similar to existing (simple text contains), increment
      occurrence_count and update last_seen; else insert new row.
      Throttle: check last_seen < 24h ago → skip
- [ ] Implement `record_opinion_accepted(store) -> Result<()>`: increment
      trust_metrics.times_accepted_opinion; update collaboration_style_signals
      prefers_opinions via EMA (alpha=0.1, target=0.8)
- [ ] Implement `record_opinion_pushback(store) -> Result<()>`: increment
      times_pushed_back; update prefers_opinions via EMA (alpha=0.1, target=0.2)
- [ ] Implement `record_asked_for_more(store) -> Result<()>`: increment
      times_asked_for_more; update wants_explanations via EMA (alpha=0.1, target=0.8)
- [ ] In `chat.rs`: after parsing user message, check for goal-statement patterns
      (same list as awareness_core). If match found, call `record_goal_stated`.
      Also check for "I finished", "done with", "completed" — call `update_goal_status`
      on the most recent Active goal if found.
- [ ] In `chat.rs`: check for "tell me more", "explain", "why?", "can you elaborate".
      If match, call `record_asked_for_more`.
- [ ] In `user_model.rs` `record_revision_accepted`: add call to
      `relational_model::record_opinion_accepted(store)`
- [ ] In `user_model.rs` `record_revision_rewrite`: add call to
      `relational_model::record_opinion_pushback(store)`
- [ ] In `main.rs` ambient monitor: once per monitor tick, for the active task,
      check if drift trigger has fired 3+ times in last 7 days (query
      `proactive_trigger_log WHERE trigger_type = 'drift' AND task_id = ? AND
      fired_at > datetime('now', '-7 days') GROUP BY ... HAVING count(*) >= 3`).
      If so, call `record_struggle(store, task_id, last_drift_reason)`.
- [ ] Implement `build_relational_context(store) -> Result<Option<String>>`:
      - Returns None if stated_goals is empty AND struggle_patterns is empty
        AND all collaboration_style_signals are at 0.5 defaults
      - Otherwise returns compact block with most recent active goal, most
        recent struggle pattern if any, and a collaboration note derived from
        prefers_opinions score (< 0.3: "prefers options over opinions";
        > 0.7: "values direct assessments"; 0.3-0.7: omit the note)
- [ ] Update `character::build_chat_system_prompt()`: call `build_relational_context()`;
      inject after snapshot summary and before task summary when Some
- [ ] Update `character::build_revision_system_prompt()`: read `prefers_opinions`
      from collaboration_style; if < 0.3, use softer assessment instruction;
      if > 0.7, use standard assertive instruction
- [ ] Update `character::build_subtask_system_prompt()` same conditional assessment
      instruction logic
- [ ] Add tauri commands: `get_relational_profile()` returning `RelationalProfile`,
      `delete_stated_goal(id)`, `delete_struggle_pattern(id)`,
      `clear_relational_profile()` (wipes all 4 tables, resets to defaults)
- [ ] In `Overlay.tsx` (or Privacy Center / App.tsx): add "goals" section above
      behavioral signals showing active stated_goals with delete buttons
- [ ] In `Overlay.tsx` (or Privacy Center): add "patterns" section showing
      struggle_patterns with delete buttons
- [ ] Update "clear all" in the Jeff remembers panel to call `clear_relational_profile`
      in addition to `clear_user_profile`
- [ ] Write unit test `goal_detected_in_im_working_on_pattern`: message
      "I'm working on the introduction" → `record_goal_stated` called with
      "the introduction"
- [ ] Write unit test `build_relational_context_returns_none_with_no_signals`:
      empty DB → None
- [ ] Write unit test `prefers_opinions_decreases_after_pushback`:
      initial 0.5, call `record_opinion_pushback` 5 times → value < 0.4
- [ ] Write unit test `collaboration_style_initialized_at_defaults`: fresh DB,
      read `prefers_opinions` → 0.5
- [ ] Write `scripts/phase30_check.sh`:
      - `relational_model.rs` exists
      - All 4 table migrations in `store.rs`
      - `record_goal_stated` called from `chat.rs`
      - `build_relational_context` in `relational_model.rs`
      - `character::build_chat_system_prompt` calls `build_relational_context`
      - `character::build_revision_system_prompt` has conditional assessment
        instruction based on `prefers_opinions`
      - `get_relational_profile` in tauri commands
      - behavioral assertion: after sending "I'm trying to finish the intro",
        `get_relational_profile` returns a stated_goal with "intro" in goal_text

---

### Exit criteria (all behavioral)

1. User says "I'm trying to finish this essay before midnight" in session 1.
   Three sessions later, Jeff's reorientation message (which reads the relational
   context) references the deadline without being told again: "You mentioned
   wanting this done by midnight — how's the progress?"

2. After user pushes back on Jeff's opinions 5 times (rewrites accepted revisions
   significantly): `prefers_opinions` score is below 0.3, and Jeff's next revision
   proposal uses the softer assessment instruction. The assessment sentence reads
   "tried a shorter approach here" not "this is cleaner."

3. After 3+ sessions where drift detection fires on the same task: a struggle
   pattern appears in the "Jeff knows you" panel: plain language, not a metric.
   ("You've hit some friction on this task's direction a few times.")

4. "Jeff knows you" panel shows stated goals (not behavioral metrics) at the top.
   Below: struggle patterns. Below: behavioral/communication style signals.

5. `build_relational_context()` returns None for a new user with no goals, no
   patterns, and default collaboration scores. Nothing is injected into the
   system prompt for a new user.

---

## Phase 31: Live Content Observation

**Gap addressed:** Gap A — Jeff's awareness is capped at window titles and workspace
files. The synthesis layer works from metadata and conversation history; it never
sees what the user is actively writing.

**Why this phase exists:** Phase 20 captures the document title. Phase 22 captures
selected text when the user presses a hotkey. Neither captures what the user is
writing right now, passively and continuously. Without seeing the content, Jeff
cannot notice that the user has been rewriting the same sentence for 20 minutes,
that 300 words were just deleted, or that the current paragraph contradicts the
thesis. The "already knows your task" felt property is partially fulfilled but
capped at the file name. This phase closes that gap with opt-in, passive, content-
level polling of the active document — distinct from Phase 20 (title only) and
Phase 22 (hotkey-triggered selection) in that it is continuous, not triggered,
and reads content rather than metadata.

---

### Scope

1. Extend `desktop/src-tauri/src/context_observer.rs` with a second polling path
   for document text content, independent of the existing 3-second title poll.
   Constant: `CONTENT_OBSERVATION_POLL_INTERVAL_SECONDS: u64 = 10`.
   The content poll runs only when `privacy_content_observation_enabled` is true
   for the active task. Polling stops when quiet mode is active or no task is active.

2. New structs in `context_observer.rs`:

   `ContentObservationState` (held in `AppState` as
   `pub content_observation: Arc<Mutex<ContentObservationState>>`; never persisted
   to SQLite):
   - `raw_text: Option<String>` — current captured document text, memory only
   - `prior_text: Option<String>` — previous captured text for change detection
   - `observation: Option<ContentObservation>` — most recent computed summary
   - `last_captured_at: Option<i64>` — unix timestamp of most recent successful
     capture
   - `capture_attempt_count: u32` — total attempts this session
   - `capture_failed_count: u32` — consecutive failed attempts for current app

   `ContentObservation`:
   - `word_count: usize`
   - `draft_state: DraftState`
   - `content_changed: bool` — true if content differs from prior capture
   - `change_magnitude: ChangeMagnitude`
   - `stable_for_ticks: u32` — consecutive polls with no content change
   - `captured_at: i64`

   `DraftState` enum (derive Serialize, Deserialize, Debug, Clone, PartialEq):
   - `Early` — word_count < 200
   - `Mid` — word_count 200–1000
   - `Late` — word_count > 1000

   `ChangeMagnitude` enum (derive Serialize, Deserialize, Debug, Clone, PartialEq):
   - `None` — content identical to prior
   - `Minor` — absolute word count difference < 10% of prior word count
   - `Major` — absolute word count difference >= 10% of prior word count

3. AX text read function in `context_observer.rs`:
   `fn read_ax_document_text(pid: i32) -> Option<String>`.
   - macOS-only; non-macOS stub returns `None`.
   - Does not call `AXIsProcessTrustedWithOptions` — permission already asserted
     by the Phase 20 title polling path.
   - Gets the frontmost application element via `AXUIElement::application(pid)`.
   - Traverses children of the focused window element (max depth 4) looking for
     the first element with role `kAXTextAreaRole`. If not found, tries
     `kAXWebAreaRole`.
   - Reads `kAXValueAttribute` on the found element.
   - Returns `None` silently on any failure (permission denied, no matching
     element, attribute unreadable).
   - Truncates result to 50,000 characters (memory guard) before returning.

4. Deterministic content summarizer (no LLM call, no I/O, pure computation):
   `fn summarize_content_observation(text: &str, prior: Option<&str>, prior_word_count: usize, stable_for_ticks: u32) -> ContentObservation`:
   - `word_count`: `text.split_whitespace().count()`
   - `draft_state`: Early/Mid/Late per thresholds above
   - `content_changed`: if prior is Some, compare first 80 chars; changed if they
     differ
   - `change_magnitude`: if not changed → `None`; if changed AND
     `abs(word_count as i64 - prior_word_count as i64) < (prior_word_count / 10) as i64`
     → `Minor`; else → `Major`
   - `stable_for_ticks`: reset to 0 if `content_changed`, increment by 1 otherwise
   - `captured_at`: `unix_now()` at call time

5. Add `ContentObservationState` to `AppState` in `state.rs`:
   `pub content_observation: Arc<Mutex<ContentObservationState>>`
   Initialize in `main.rs` as `Arc::new(Mutex::new(ContentObservationState::default()))`.

6. Add `SnapshotTrigger::ContentObservation` variant to the `SnapshotTrigger` enum
   in `awareness_core.rs`. This is the 7th trigger variant. Wire: after each content
   poll completes (regardless of whether content changed), call
   `state.awareness_core.update(SnapshotTrigger::ContentObservation, task_id, ...)`
   via `spawn` (non-blocking).

7. Extend `SituationalSnapshot` in `awareness_core.rs` with two new fields (Phase 31
   addition; the 9-field Phase 26 struct becomes 11 fields):
   - `pub active_document_excerpt: Option<String>` — compact natural-language
     description, ≤ 100 tokens. Format:
     `"~[word_count] words, [draft_state] draft, [change phrase]"`.
     Example: `"~840 words, mid-draft, content changed recently"`.
     `None` when content observation is disabled or no capture has occurred.
   - `pub content_idle_seconds: Option<u32>` — computed as
     `stable_for_ticks * CONTENT_OBSERVATION_POLL_INTERVAL_SECONDS`.
     `None` when content observation is disabled.

8. In `AwarenessCore::update()`: when trigger is `ContentObservation`, acquire a
   read lock on `state.content_observation`, read the current `ContentObservation`,
   and populate `active_document_excerpt` and `content_idle_seconds`. If
   `active_document_excerpt` is Some, add `+0.10` to `snapshot_confidence`
   (capped at 1.0).

9. In `snapshot_summary()`: when `active_document_excerpt` is Some, append the
   line `active document: [active_document_excerpt]`. The 150-token budget still
   applies; truncate `active_document_excerpt` to 60 characters if the summary
   is near budget.

10. In `should_speak_proactively()` in `awareness_core.rs`: activate the
    `WorkQualityObservation` variant (reserved in Phase 27) as the lowest-priority
    check, added after all existing checks:
    If `snapshot.content_idle_seconds >= Some(60)` AND
    `snapshot.attention_state == AttentionState::Focused` AND
    `last_meaningful_turn` is more than 300 seconds ago AND
    `snapshot_confidence >= 0.3`:
    → return `ProactiveSpeechReason::WorkQualityObservation { observation: "content unchanged for a while".to_string() }`.

11. Privacy Center additions:
    - New app_settings key per task: `privacy_content_observation_task_[task_id]`
      (value "true" / "false", default "false")
    - Toggle label: "Active document reading"
    - Explanation text (exact; shown before first use and in Privacy Center):
      "Jeff will periodically read the text in your active document window to give
      you better feedback. This text never leaves your device."
    - Privacy Center status row: "Last read: [timestamp]" when `last_captured_at`
      is Some; "Could not read text from [app name] — this app may restrict
      accessibility access." when `capture_failed_count >= 3`
    - "Clear" button: calls `clear_content_observation` tauri command, which sets
      `raw_text = None` and `prior_text = None` in the `ContentObservationState`
      mutex. Does not change `capture_attempt_count` or `last_captured_at`.

12. Graceful degradation:
    - If `read_ax_document_text` returns `None` for 3 consecutive polls for the
      same frontmost app: stop attempting for that app session (until frontmost app
      changes). Show the failure message in Privacy Center.
    - No error appears in the companion chat. No empty states. Behavior is
      identical to feature-off.

13. The raw captured text never reaches SQLite, any log table, or any API request.
    The only thing that reaches the LLM is `active_document_excerpt`. This is
    enforced at the boundary: `context_observer.rs` holds raw text in memory;
    `awareness_core.rs` receives only the pre-computed `ContentObservation` struct.

---

### Implementation checklist

- [ ] Add `CONTENT_OBSERVATION_POLL_INTERVAL_SECONDS: u64 = 10` constant to
      `context_observer.rs`
- [ ] Define `DraftState` enum in `context_observer.rs`: `Early`, `Mid`, `Late`;
      derive `Serialize, Deserialize, Debug, Clone, PartialEq`
- [ ] Define `ChangeMagnitude` enum in `context_observer.rs`: `None`, `Minor`,
      `Major`; derive same traits
- [ ] Define `ContentObservation` struct in `context_observer.rs` with 6 fields:
      `word_count: usize`, `draft_state: DraftState`, `content_changed: bool`,
      `change_magnitude: ChangeMagnitude`, `stable_for_ticks: u32`,
      `captured_at: i64`
- [ ] Define `ContentObservationState` struct in `context_observer.rs` with 6 fields:
      `raw_text: Option<String>`, `prior_text: Option<String>`,
      `observation: Option<ContentObservation>`, `last_captured_at: Option<i64>`,
      `capture_attempt_count: u32`, `capture_failed_count: u32`
- [ ] Implement `ContentObservationState::default()`: all None/0
- [ ] Add `pub content_observation: Arc<Mutex<ContentObservationState>>` to
      `AppState` in `state.rs`
- [ ] Initialize `Arc::new(Mutex::new(ContentObservationState::default()))` in
      `main.rs` AppState construction
- [ ] Implement `fn read_ax_document_text(pid: i32) -> Option<String>` in
      `context_observer.rs`:
      - `#[cfg(target_os = "macos")]` gate; non-macOS returns None
      - `AXUIElement::application(pid)` to get app element
      - Traverse children (max depth 4) for `kAXTextAreaRole`; fall back to
        `kAXWebAreaRole`
      - Read `kAXValueAttribute` on found element; return None on any failure
      - Truncate to 50,000 chars before returning
- [ ] Implement `fn summarize_content_observation(text: &str, prior: Option<&str>, prior_word_count: usize, stable_for_ticks: u32) -> ContentObservation`:
      - word_count via `split_whitespace().count()`
      - draft_state per thresholds (< 200 Early, > 1000 Late, else Mid)
      - content_changed via first-80-chars comparison
      - change_magnitude via word count ratio
      - stable_for_ticks: reset on change, increment on no change
      - captured_at: current unix timestamp
- [ ] Implement content observation polling task:
      `pub async fn run_content_observation_poll(state: Arc<AppState>, ambient: Arc<AmbientState>, app: AppHandle)`:
      - Loop with `tokio::time::sleep(Duration::from_secs(CONTENT_OBSERVATION_POLL_INTERVAL_SECONDS))`
      - Check `JEFF_DISABLE_CONTENT_OBSERVATION` env var; skip if set
      - Check quiet mode via `ambient.is_quiet_mode()`; skip if active
      - Read active task_id from state; skip if None
      - Read `privacy_content_observation_task_[task_id]` from app_settings; skip
        if "false" or absent
      - If `capture_failed_count >= 3`: skip
      - Call `read_ax_document_text(pid)` with current frontmost app PID from
        `AmbientState.context_state`; increment `capture_attempt_count`
      - On None: increment `capture_failed_count`; if now >= 3, log to Privacy
        Center state
      - On Some(text): reset `capture_failed_count` to 0; call
        `summarize_content_observation`; update `ContentObservationState`;
        set `last_captured_at`; spawn `awareness_core.update(ContentObservation, ...)`
- [ ] Spawn `run_content_observation_poll` in `main.rs` at startup, alongside the
      ambient monitor
- [ ] Add `SnapshotTrigger::ContentObservation` variant to `SnapshotTrigger` enum
      in `awareness_core.rs`
- [ ] Add `pub active_document_excerpt: Option<String>` field to `SituationalSnapshot`
      in `awareness_core.rs`
- [ ] Add `pub content_idle_seconds: Option<u32>` field to `SituationalSnapshot`
      in `awareness_core.rs`
- [ ] Update `SituationalSnapshot::default()` to set both new fields to `None`
- [ ] Update `AwarenessCore::update()`: when trigger is `ContentObservation`, read
      `state.content_observation` lock, compute `active_document_excerpt` string
      from observation fields, compute `content_idle_seconds` from
      `stable_for_ticks * 10`, add +0.10 confidence bonus when excerpt is Some
- [ ] Update `snapshot_summary()` to include `active document: [excerpt]` line
      when `active_document_excerpt` is Some; truncate to 60 chars if near token
      budget
- [ ] Update `should_speak_proactively()` in `awareness_core.rs`: add
      `WorkQualityObservation` check as final (lowest priority) check per scope
      item 10
- [ ] Add tauri command `set_content_observation_enabled(task_id: i64, enabled: bool)`:
      writes `privacy_content_observation_task_[task_id]` to app_settings
- [ ] Add tauri command `get_content_observation_enabled(task_id: i64) -> bool`:
      reads the same key; returns false if absent
- [ ] Add tauri command `clear_content_observation()`: acquires
      `content_observation` mutex, sets `raw_text = None` and `prior_text = None`
- [ ] Add `content_observation_enabled: bool`, `content_observation_last_captured_at: Option<String>`,
      `content_observation_capture_failed: bool`, `content_observation_failed_app: Option<String>`
      to `PrivacyCenterDashboardDto` in `models.rs`
- [ ] Update Privacy Center dashboard command in `commands.rs` to populate the new
      DTO fields from `ContentObservationState`
- [ ] Update Privacy Center in `App.tsx` or `Overlay.tsx`:
      - Add "Active document reading" toggle row
      - Show exact explanation text from scope item 11
      - Show "Last read: [timestamp]" or "Not yet captured" when enabled
      - Show capture failure message when `content_observation_capture_failed` is
        true
      - Show "Clear" button calling `clearContentObservation()`
- [ ] Add `JEFF_DISABLE_CONTENT_OBSERVATION=1` env var guard in
      `run_content_observation_poll` (CI safety)
- [ ] Write unit test `summarize_empty_text_is_early_draft`: empty string →
      word_count 0, DraftState::Early, content_changed false
- [ ] Write unit test `summarize_detects_major_change`: prior 100 words, new 200
      words → ChangeMagnitude::Major, content_changed true, stable_for_ticks 0
- [ ] Write unit test `summarize_minor_change`: prior 100 words, new 105 words →
      ChangeMagnitude::Minor
- [ ] Write unit test `stable_for_ticks_resets_on_change`: stable_for_ticks = 5,
      content changes → stable_for_ticks = 0 in result
- [ ] Write unit test `content_idle_seconds_from_ticks`: stable_for_ticks = 6 →
      content_idle_seconds = Some(60)
- [ ] Write unit test `snapshot_confidence_bonus_when_excerpt_present`: base
      confidence 0.6 + excerpt present → confidence 0.7 (capped at 1.0)
- [ ] Write `scripts/phase31_check.sh`:
      - `ContentObservationState` and `ContentObservation` structs in
        `context_observer.rs`
      - `read_ax_document_text` function in `context_observer.rs`
      - `active_document_excerpt` field in `SituationalSnapshot`
      - `content_idle_seconds` field in `SituationalSnapshot`
      - `SnapshotTrigger::ContentObservation` variant in `awareness_core.rs`
      - `set_content_observation_enabled` and `clear_content_observation` in
        `commands.rs`
      - `JEFF_DISABLE_CONTENT_OBSERVATION` env var guard present
      - behavioral assertion: with feature enabled and TextEdit open containing
        text, `get_situational_snapshot` returns non-null `active_document_excerpt`
        within 15 seconds

---

### Exit criteria (all behavioral)

1. Content observation is off by default. Privacy Center shows "Active document
   reading: off." `get_situational_snapshot` returns `active_document_excerpt: null`.
   No content polling activity occurs.

2. Enable the toggle. Open a TextEdit document containing 300 words. Within 12
   seconds, `get_situational_snapshot` shows `active_document_excerpt: "~300 words,
   mid-draft, no recent changes"`. The raw document text is not present in the
   snapshot debug output, in any SQLite table, or in any API request log.

3. Jeff's response to "how am I doing on my draft?" references the approximate word
   count or draft state ("you're about 300 words in, solidly mid-draft") without
   the user mentioning it. The content observation summary was injected via the
   snapshot into the system prompt.

4. Keep a document open without typing for 60 seconds. Jeff sends a proactive chat
   bubble via the synthesis path: "You've been looking at this for a bit without
   changes — stuck somewhere?" This is the `WorkQualityObservation` path.

5. Disable the toggle. Jeff stops capturing. `get_situational_snapshot` returns
   `active_document_excerpt: null`. "Clear" button has no visible effect (nothing
   to clear).

6. Open an app where AX text access fails. After 3 failed polls (within 30
   seconds), Privacy Center shows the capture failure message for that app. No
   error appears in the companion chat.

7. Raw text audit: after a session with content observation enabled, run
   `sqlite3 ~/Library/Application\ Support/com.jeff.desktop/jeff.db
   "SELECT count(*) FROM chat_messages WHERE content LIKE '%[first 10 words of doc]%'"`.
   Result: 0. The raw text was never persisted.

---

## Phase 32: Character Consistency Enforcement

**Gap addressed:** Gap B — Character is installed via system prompt and string
filter (Phase 25) but not verified. Weak assessments, hedged disagreements,
result-without-assessment outputs, and near-violations not in the phrase blocklist
can occur silently without detection. Under model updates or new prompt paths,
character quality can degrade without any signal.

**Why this phase exists:** Phase 14 (intent classification) demonstrated that
accuracy requires measurement: a classifier that is 70% accurate is not the same
as one at 95%, and the difference is invisible without an eval. Character quality
needs the same treatment. This phase introduces a labeled eval harness that makes
character consistency checkable, runnable in CI, and extensible as new output
surfaces are added. It also retroactively extends Phase 25's exit criteria with
a mandatory eval pass.

---

### Scope

1. Create `eval/character_eval.json` — minimum 30 labeled evaluation cases.
   Each case:
   ```json
   {
     "id": "c001",
     "context": "optional scenario description for human readers",
     "input": "the user message or scenario prompt",
     "jeff_output": "a Jeff response (correct or incorrect character)",
     "violations": []
   }
   ```
   Distribution requirements: at minimum 18 cases with `violations: []` (correct
   character) and at minimum 12 cases with one or more violations. Every violation
   type in the taxonomy below must appear in at least 2 negative cases. Cases must
   cover these output surfaces: chat answer, revision proposal, subtask draft
   result, reorientation message, direct disagreement, uncertainty acknowledgment,
   short reply (< 15 words).

2. Violation taxonomy — 8 types. These are the authoritative identifiers. Use
   these exact strings in the eval set and the grading prompt.

   `FillerPhrase` — response starts with or contains a phrase from the banned
   list: "Certainly", "Absolutely", "Great question", "Of course", "Sure thing",
   "Happy to help", "I'd be happy to", "I'll go ahead and", "I've gone ahead and"

   `PermissionSeeking` — response seeks permission to state an opinion:
   "Would it be okay if I", "If you'd like I could", "I might suggest",
   "Perhaps I could", "Would you like me to"

   `DisagreementAsQuestion` — a direct disagreement is framed as a question
   rather than a statement: "Have you considered", "You might want to think
   about whether", "Wouldn't it be better to"

   `TrailingSummary` — response ends by summarizing what Jeff just did:
   "So I've gone ahead and", "I've now revised the paragraph to",
   "In summary, I have", "To recap what I did"

   `ResultWithoutAssessment` — delivers a revision, draft, or completed task
   result without a first-person assessment sentence before the result. Applies
   only when the output is presenting a result artifact; not to conversational
   replies.

   `ExcessiveHedge` — states more than one hedge clause for a single opinion:
   "This might possibly be one approach that could potentially be helpful in some
   cases"

   `NonAnswer` — states "it depends" or an equivalent vague statement without
   following with an actual answer

   `SelfNarration` — describes its own process before delivering the result:
   "I'll now analyze your paragraph", "First, let me examine",
   "Let me take a look at this"

3. Create `scripts/character_eval.sh`:
   - Requires `OPENAI_API_KEY` env var; exits 2 with an error message if absent
   - Reads `eval/character_eval.json`
   - Samples 10 cases randomly:
     ```sh
     SAMPLE=$(python3 -c "
     import random,json,sys
     d=json.load(sys.stdin)
     print(json.dumps(random.sample(d,10)))
     " < eval/character_eval.json)
     ```
   - For each of the 10 cases: sends `jeff_output` to gpt-4o-mini with the
     character-grading system prompt; parses the JSON response
   - Pass logic:
     - Positive case (labeled `violations: []`): passes if grader returns
       `violations: []`
     - Negative case (labeled violations non-empty): passes if grader returns
       any non-empty `violations` list (exact match on type not required)
   - Prints each result: `[PASS] c001` or `[FAIL] c001 — expected clean, got violations` /
     `[FAIL] c001 — expected violations, got clean`
   - Prints summary: `N/10 passed`
   - Exits 0 if N >= 8; exits 1 otherwise; exits 2 on env/parse error

4. Character-grading system prompt embedded verbatim in `character_eval.sh`:
   ```
   You are a character consistency grader for Jeff, an AI companion.
   Check the following Jeff response for violations of Jeff's character spec.

   Check for exactly these violation types:
   - FillerPhrase: contains "Certainly", "Absolutely", "Great question",
     "Of course", "Sure thing", "Happy to help", "I'd be happy to",
     "I'll go ahead and", "I've gone ahead and"
   - PermissionSeeking: seeks permission to state an opinion ("Would it be
     okay if I", "If you'd like I could", "I might suggest", "Perhaps I could")
   - DisagreementAsQuestion: frames a disagreement as a question ("Have you
     considered", "You might want to think about whether", "Wouldn't it be
     better to")
   - TrailingSummary: ends by summarizing what Jeff just did ("So I've gone
     ahead and", "I've now revised the paragraph to", "In summary, I have")
   - ResultWithoutAssessment: delivers a revision, draft, or task result without
     a first-person assessment sentence before the result (not applicable to
     conversational replies)
   - ExcessiveHedge: uses more than one hedge clause for a single opinion
   - NonAnswer: says "it depends" or equivalent without providing an actual
     answer
   - SelfNarration: narrates its own process before delivering ("I'll now
     analyze", "First, let me examine", "Let me take a look at")

   Respond only with JSON. No other text.
   {"violations": ["ViolationType", ...], "explanation": "one sentence"}
   If no violations: {"violations": [], "explanation": "clean"}
   ```

5. Create `eval/character_eval_guide.md`:
   - The violation taxonomy: all 8 types with one concrete violating example and
     one passing example each
   - JSON format for adding new cases (schema above)
   - How to run: `OPENAI_API_KEY=sk-... bash scripts/character_eval.sh`
   - Policy: every new Jeff output surface ships with at least 3 new eval cases
     covering that surface's specific output format (2 positive, 1 negative
     `ResultWithoutAssessment` for result-producing surfaces)
   - How to run against a specific violation type:
     ```sh
     python3 -c "
     import json, sys
     d = json.load(open('eval/character_eval.json'))
     t = sys.argv[1]
     print(json.dumps([c for c in d if t in c['violations'] or not c['violations']]))
     " ResultWithoutAssessment | bash scripts/character_eval.sh /dev/stdin
     ```

6. Phase 25 update (to be applied when executing Phase 32): add the following item
   to Phase 25's implementation checklist: "[ ] Run `scripts/character_eval.sh`
   after all prompt paths are wired; verify ≥ 8/10 pass before marking Phase 25
   complete." Add the following exit criterion to Phase 25: "6. Run
   `scripts/character_eval.sh`. At least 8 of 10 sampled cases pass (grader
   verdict agrees with labeled ground truth)." These additions are applied as an
   edit to this document when Phase 32 is executed, not before.

7. Write `scripts/phase32_check.sh` per implementation checklist item below.

---

### Implementation checklist

- [ ] Create `eval/` directory in project root
- [ ] Write `eval/character_eval.json` with ≥ 30 cases:
      Positive cases (violations: []) covering: chat answer with direct opening,
      revision with first-person assessment before proposed text, subtask result
      with assessment before excerpt, reorientation as conversation opener (not
      notification), direct disagreement stated as fact, uncertainty acknowledged
      in one clause and answer given, short question answered in one sentence,
      Jeff deferring after stating view, Jeff correcting itself without apology,
      adaptive register (quick question quick answer), revision alternative with
      different assessment, Jeff stating it lacks information and providing what
      it knows, post-proactive message reply without asking why user ignored it,
      assessment that names a tradeoff explicitly, first-person subtask result
      summary, statement of draft state observation, response to "tell me more"
      that adds content, response using "I" not "one" or "we"
      Negative cases covering all 8 violation types at ≥ 2 cases each:
      FillerPhrase × 2 (one starting with "Certainly!", one with "Great question!"),
      PermissionSeeking × 2 (one "Would it be okay", one "If you'd like"),
      DisagreementAsQuestion × 2, TrailingSummary × 1, ResultWithoutAssessment × 2
      (one revision, one subtask), ExcessiveHedge × 1, NonAnswer × 1,
      SelfNarration × 1
- [ ] Create `scripts/character_eval.sh` with:
      - `OPENAI_API_KEY` guard (exit 2 if absent)
      - python3 10-case random sampling
      - per-case gpt-4o-mini call with grading system prompt from scope item 4
        embedded verbatim
      - pass/fail logic per scope item 3
      - exit code: 0 if ≥ 8/10, 1 if < 8/10, 2 on error
- [ ] Make `scripts/character_eval.sh` executable: `chmod +x scripts/character_eval.sh`
- [ ] Create `eval/character_eval_guide.md` covering all items in scope item 5,
      including: taxonomy with examples, JSON schema, how to run, policy for new
      surfaces, how to filter by violation type
- [ ] Write unit test `character_eval_json_has_minimum_cases`:
      parse `eval/character_eval.json`, assert `len >= 30`,
      assert each of 8 violation types appears in >= 2 cases,
      assert >= 18 cases have `violations: []`
- [ ] Write `scripts/phase32_check.sh`:
      - `eval/character_eval.json` exists and parses: `python3 -c "import json; json.load(open('eval/character_eval.json'))" && echo ok`
      - Case count >= 30: `python3 -c "import json; d=json.load(open('eval/character_eval.json')); assert len(d)>=30,f'{len(d)} cases'"`
      - Each of 8 violation types in >= 2 cases: `python3` assertion script
      - Positive cases >= 18: assertion
      - `scripts/character_eval.sh` is executable: `test -x scripts/character_eval.sh`
      - `eval/character_eval_guide.md` exists
      - behavioral assertion: if `OPENAI_API_KEY` is set, run
        `bash scripts/character_eval.sh` and assert exit code 0

---

### Exit criteria (all behavioral)

1. `eval/character_eval.json` has at minimum 30 cases. Every violation type in the
   taxonomy appears in at least 2 negative cases. At least 18 cases have
   `violations: []`. The JSON parses without error.

2. `bash scripts/character_eval.sh` exits 0. At least 8 of 10 sampled cases pass
   (grader verdict matches labeled ground truth).

3. At least 2 of the sampled 10 cases are negative cases with labeled violations.
   The grader correctly flags both as having violations. (Probabilistic: with 12
   negative cases in a 30-case set, sampling 10 produces ≥ 2 negatives with high
   probability.)

4. A new engineer adds a case to `eval/character_eval.json` following
   `eval/character_eval_guide.md`. The file remains valid JSON and
   `character_eval.sh` runs successfully with the updated set.

5. Phase 25's exit criteria are updated in this document (as part of Phase 32
   execution) to add: "Run `scripts/character_eval.sh`. At least 8 of 10 sampled
   cases pass." Phase 25 cannot be marked complete without this passing.

---

## Phase 33: Native Document Write-Back

**Gap addressed:** Jeff produces results into the companion bar. The user still
has to manually take that output and put it into their document. Every single
interaction has this friction. Cursor that couldn't write to your files would
not be Cursor. Jeff that can't write to your document is a smarter clipboard.

**Why this phase exists:** The subtask engine (Phases 15–16) already runs
parallel work and produces file write proposals gated behind explicit approval.
The same approval-gated model applies here — but instead of writing to workspace
files, Jeff writes directly into the active document the user is working in.
The felt property "does parts of the work in parallel while you keep going"
is incomplete until the output lands in the document without the user touching it.

---

### Scope

1. **Target surfaces in priority order:** Google Docs (browser, via existing
   extension infrastructure from Phase 23), Apple Pages (via Accessibility API
   write path), Microsoft Word for Mac (via Accessibility API write path). Other
   apps degrade gracefully — Jeff shows output in the companion bar with a copy
   button. No error state.

2. **The write-back flow** follows the exact approval pattern as Phase 16 file
   writes. Jeff produces a result, surfaces an approval card showing: the target
   document name, the insertion point description ("after paragraph 2"), and a
   before/after excerpt. The user approves or rejects. Approval triggers the
   write. Rejection leaves the document unchanged. No write ever reaches the
   document without explicit approval.

3. **Google Docs path:** extend the existing browser extension (Phase 23).
   The extension's `content.js` already has anchor-hash validation. Add an
   insertion mode: instead of patching selected text, insert new content at a
   specified anchor position. The anchor is identified by surrounding context
   (50 characters before the insertion point), not by cursor position, so it
   survives minor edits between approval and application. The extension reports
   success or failure back via the existing `/apply-result` HTTP bridge endpoint.

4. **Pages and Word path:** use the macOS Accessibility API write path —
   `AXValue` setAttribute on the target `AXTextArea`. The same accessibility
   permission from Phase 20 and Phase 31 covers this. Identify the insertion
   anchor by character offset from a context match (same 50-char surrounding
   context approach). If the anchor cannot be found (document changed
   significantly between approval and application), route to guided-apply
   fallback: show the content in the companion bar with a "copy to clipboard"
   button and instruction text saying exactly where to paste.

5. **Insertion types Jeff must support:**
   - `InsertAfterParagraph`: inserts content after the paragraph containing the
     anchor context
   - `ReplaceSelection`: replaces a specified range (used for revision write-back)
   - `AppendToDocument`: inserts at the end, used for subtask output when no
     specific anchor exists

6. **Audit table:** all three insertion types produce a record in
   `document_write_receipts`: `app_name`, `document_title`, `insertion_type`,
   `anchor_context`, `content_excerpt` (first 100 chars), `status`
   (pending/approved/rejected/failed/guided), `failure_reason`, `created_at`,
   `resolved_at`.

7. **On failure** (anchor drift, accessibility write rejected, extension error):
   never silently fail. Always surface the guided-apply fallback with the content
   available to copy. Log the failure in `document_write_receipts` with status
   `"failed"` and a `failure_reason`.

8. **Insertion point inference:** when `active_document_excerpt` is available
   (Phase 31) and a subtask result is being delivered, the synthesis judgment
   includes a best-guess insertion point derived from the document excerpt. This
   inference is included in the approval card as a suggestion (e.g. "after your
   introduction paragraph"), not applied automatically. See SYNTHESIS_ARCHITECTURE.md
   Phase 33 addendum.

---

### Implementation checklist

- [ ] Create `desktop/src-tauri/src/document_write.rs`
- [ ] Define `InsertionType` enum: `InsertAfterParagraph`, `ReplaceSelection`,
      `AppendToDocument`; derive `Serialize, Deserialize, Debug, Clone`
- [ ] Define `DocumentWriteRequest` struct: `app_name: String`,
      `document_title: String`, `insertion_type: InsertionType`,
      `anchor_context: Option<String>`, `content: String`, `task_id: i64`
- [ ] Define `DocumentWriteReceipt` struct: `id: i64`, `app_name: String`,
      `document_title: String`, `insertion_type: String`,
      `anchor_context: Option<String>`, `content_excerpt: String`,
      `status: String`, `failure_reason: Option<String>`,
      `created_at: String`, `resolved_at: Option<String>`
- [ ] Define `WriteResult` enum: `Success`, `AnchorNotFound`, `AccessibilityDenied`
- [ ] Add `document_write_receipts` table migration in `store.rs` (idempotent):
      `id INTEGER PRIMARY KEY AUTOINCREMENT`, `task_id INTEGER`, `app_name TEXT`,
      `document_title TEXT`, `insertion_type TEXT NOT NULL`, `anchor_context TEXT`,
      `content_excerpt TEXT NOT NULL`, `status TEXT NOT NULL DEFAULT 'pending'`,
      `failure_reason TEXT`, `created_at TEXT NOT NULL DEFAULT (datetime('now'))`,
      `resolved_at TEXT`
- [ ] Implement `store.create_write_receipt(task_id, request) -> Result<i64>`:
      inserts record with status "pending", returns new id
- [ ] Implement `store.resolve_write_receipt(id, status, failure_reason) -> Result<()>`:
      updates status and sets resolved_at to current datetime
- [ ] Implement `store.list_write_receipts(task_id) -> Result<Vec<DocumentWriteReceipt>>`
- [ ] Implement `store.get_pending_write_receipts(task_id) -> Result<Vec<DocumentWriteReceipt>>`:
      WHERE status = 'pending'
- [ ] Implement `attempt_pages_write(request: &DocumentWriteRequest) -> Result<WriteResult>`
      in `document_write.rs`:
      - `#[cfg(target_os = "macos")]` gate
      - Gets frontmost Pages `AXUIElement` via `AXUIElementCreateApplication(pid)`
      - Traverses to `AXTextArea` element (max depth 4)
      - Reads `kAXValueAttribute` to get current content string
      - Finds anchor by searching for `anchor_context` (50-char string) in content
      - If not found: return `WriteResult::AnchorNotFound`
      - Builds modified content string per `insertion_type`
      - Calls `AXUIElementSetAttributeValue` with `kAXValueAttribute` on the element
      - On AX error: return `WriteResult::AccessibilityDenied`
      - On success: return `WriteResult::Success`
- [ ] Implement `attempt_word_write(request: &DocumentWriteRequest) -> Result<WriteResult>`:
      same pattern as `attempt_pages_write`, targeting Word's `AXTextArea` hierarchy
- [ ] Implement `attempt_guided_fallback(app: &AppHandle, request: &DocumentWriteRequest)`:
      emits `document_write://fallback_triggered` event with full content, insertion
      instruction text, and `receipt_id` for frontend to display
- [ ] Implement `dispatch_document_write(store, app, request) -> Result<()>`:
      - Creates receipt via `store.create_write_receipt` (status "pending")
      - Checks `request.app_name` against supported list (`"Pages"`, `"Microsoft Word"`,
        Google Docs detected via extension path)
      - Routes to `attempt_pages_write`, `attempt_word_write`, or guided fallback
        for unsupported apps
      - Resolves receipt with correct status and failure reason
      - For Google Docs: emits `document_write://apply_requested` event to extension
        bridge; awaits `document_write://apply_result` event (10-second timeout);
        resolves receipt on result
- [ ] Add tauri command `approve_document_write(state, app, receipt_id: i64) -> Result<()>`:
      reads pending receipt, calls `dispatch_document_write`
- [ ] Add tauri command `reject_document_write(state, receipt_id: i64) -> Result<()>`:
      calls `store.resolve_write_receipt(id, "rejected", None)`
- [ ] Add tauri command `list_document_write_receipts(state, task_id: i64) -> Result<Vec<DocumentWriteReceipt>>`
- [ ] Add tauri command `get_pending_document_writes(state, task_id: i64) -> Result<Vec<DocumentWriteReceipt>>`
- [ ] Register all four commands in `main.rs` invoke handler
- [ ] Extend browser extension `content.js`: implement
      `insertAfterAnchor(anchorContext, content)` — finds anchor by 50-char context
      string search in `document.body.innerText`, walks up DOM to find containing
      paragraph element, inserts `content` as a new `<p>` after that element.
      Returns `{ success: boolean, reason?: string }` to `background.js`
- [ ] Extend browser extension `background.js`: add polling for
      `document_write://apply_requested` events (same pattern as existing live-edit
      polling). On receive: dispatches to `content.js` `insertAfterAnchor`. Reports
      result via existing `/apply-result` endpoint with `receipt_id` in payload and
      `status: "applied" | "failed"`
- [ ] Extend HTTP bridge server in `selection_capture.rs` (or equivalent bridge
      module): add `POST /document-write-result` route receiving
      `{ receipt_id, status: "applied" | "failed", reason? }` — calls
      `store.resolve_write_receipt`
- [ ] In `subtask.rs`: implement `infer_insertion_point(content: &str, excerpt: &str) -> Option<String>`:
      simple heuristic — if excerpt mentions "introduction", "intro", "opening",
      suggest "after your introduction paragraph"; if excerpt mentions word count
      > 800, suggest "at the end of the document"; otherwise return None.
      No LLM call.
- [ ] In `subtask.rs`: when subtask chain completes with a content result, check
      if `awareness_core.snapshot().active_document_excerpt` is Some. If yes, call
      `infer_insertion_point` and include result in `DocumentWriteRequest.anchor_context`
      marked as inferred (prefix: `"inferred: "`)
- [ ] In `subtask.rs`: for supported apps (Pages, Word, Google Docs detected via
      active window context), replace existing companion bar result display with
      `create_write_receipt` + emit `document_write://approval_requested` event.
      Unsupported apps retain existing companion bar display with copy button
- [ ] In `Overlay.tsx`: add document write approval card component. Renders on
      `document_write://approval_requested` event. Shows: document name, insertion
      type label ("Adding after paragraph 2" / "Replacing selection" / "Adding to
      end"), before/after excerpt (before: last 80 chars before anchor; after: new
      content first 120 chars), Approve and Reject buttons. On Approve: calls
      `approveDocumentWrite(receiptId)`. On Reject: calls `rejectDocumentWrite(receiptId)`
- [ ] In `Overlay.tsx`: add guided fallback card component. Renders on
      `document_write://fallback_triggered` event. Shows: content in a read-only
      `<textarea>`, "Copy to clipboard" button, insertion instruction text (from
      event payload). No error label.
- [ ] Add Privacy Center section "Document write history": table of
      `document_write_receipts` for active task showing app, insertion type, status,
      timestamp. "Clear" button calls new `clear_document_write_receipts(task_id)`
      tauri command that deletes all receipts for the active task
- [ ] Add tauri command `clear_document_write_receipts(state, task_id: i64) -> Result<()>`
- [ ] Write unit test `anchor_match_finds_correct_position`: document with 3
      paragraphs, anchor context = last 50 chars of paragraph 1 → content inserted
      after paragraph 1 at correct character offset
- [ ] Write unit test `anchor_not_found_routes_to_fallback`: anchor context string
      not present in document → `WriteResult::AnchorNotFound` → guided fallback
      event fires
- [ ] Write unit test `receipt_created_before_write_attempted`: call
      `dispatch_document_write` → receipt exists with status "pending" before write
      attempt resolves
- [ ] Write unit test `reject_leaves_document_unchanged`: receipt shown, reject
      called → `resolve_write_receipt` called with status "rejected", no AX write
      attempted
- [ ] Write `scripts/phase33_check.sh`:
      - `document_write.rs` exists
      - `document_write_receipts` table migration in `store.rs`
      - `approve_document_write` and `reject_document_write` in `commands.rs`
      - `insertAfterAnchor` present in extension `content.js`
      - `document_write://approval_requested` emitted in `subtask.rs` or
        `document_write.rs`
      - behavioral assertion: trigger a subtask result delivery for a Pages session
        → `get_pending_document_writes` returns one record with status "pending"
        before any approval action

---

### Exit criteria (all behavioral)

1. User is in Google Docs. Says "draft a conclusion paragraph." Jeff runs the
   subtask. When complete, an approval card appears in the companion bar showing
   the content and "Adding to end of document." User approves. The paragraph
   appears in Google Docs without the user touching the document.

2. User is in Pages. Same flow. Paragraph appears in Pages on approval.

3. User is in an unsupported app (e.g. Notion desktop). Same flow. Guided
   fallback card appears with content and copy button. No error state, no
   explanation required beyond the implicit behavior.

4. Anchor drifts — user edits the document between the approval card appearing
   and approving. Jeff cannot find the anchor. Guided fallback triggers
   automatically. Receipt status is "failed" with reason "anchor not found."
   Document is unchanged.

5. User rejects a write. Receipt status is "rejected." Document is unchanged.
   No retry.

---

## Phase 34: Web Retrieval Inside the Subtask Agent

**Gap addressed:** Jeff is entirely closed-world. It knows the watched folder
and what the user has said in conversation. For document-heavy and research-heavy
work — the target user — Jeff cannot go get anything. A researcher needs sources.
A writer needs facts. A student needs material they haven't pre-loaded. Without
outbound retrieval, Jeff is a smart editor, not a research partner.

**Why this phase exists:** The subtask chain already supports step types:
`retrieval` (local RAG), `llm_call`, `file_write_proposal`. Adding `web_search`
as a fourth step type completes the loop: find it (Phase 34), draft with it
(existing `llm_call` step), place it in the document (Phase 33). Phase 33 must
exist first because retrieved content's primary value is landing in the document.

---

### Scope

1. **`web_search` is a new step type** in the existing subtask chain runner
   (`subtask.rs`). The chain planner LLM call must be updated to know this step
   type exists and when to use it.

2. **Search API:** Brave Search API or Serper (clean REST, no OAuth). API key
   stored in macOS Keychain under `com.jeff.desktop.search_api_key`. If no key
   is present, web search steps are skipped and the companion bar shows: "web
   search unavailable: add a search API key in settings." No silent failure.

3. **A `web_search` step produces a `WebSearchResult`:** query used, list of up
   to 5 results each with `title`, `url`, `excerpt` (first 500 chars of page
   body, fetched via HTTP GET with 5-second timeout). Results stored in
   `subtask_web_results` table. Never written to disk without an explicit
   `file_write_proposal` step following them.

4. **Search is scoped.** The chain planner is instructed: web search steps must
   have a specific query derived from the user's actual request, not a broad
   topic query. "evidence for the argument that containment was economically
   motivated" not "cold war." The query appears in the approval surface.

5. **Results surface as a web results card** in the companion bar showing: the
   query, up to 5 result titles with URLs, one-sentence excerpt per result,
   checkbox per result (all checked by default). User actions: Approve Selected,
   or Reject. No result is used in a draft without this approval.

6. **After approval,** selected results are injected as context into the next
   `llm_call` step. The LLM call is instructed to cite which source informed
   which part of the output using inline references (URL + title). If the LLM
   produces a claim without a source in its context, it must not fabricate a
   citation — it omits the citation.

7. **Rate limiting:** maximum 3 web search steps per subtask chain. Maximum 10
   API calls per hour across all chains (enforced via counter in `app_settings`).
   If the hourly limit is reached, remaining web search steps are skipped with a
   companion bar note. No search is performed for queries that contain the user's
   name from their profile (simple case-insensitive string check).

8. **Privacy Center:** new "Web search" section — whether a search API key is
   configured, current hour's call count against the limit, log of the last 10
   queries executed, Clear log button.

9. **Chain planner system prompt update** in `subtask.rs`: include `web_search`
   in the available step type list; describe when to use it ("when the request
   requires external information not available in workspace files — provide a
   specific topic query"); include current tool availability status
   (`[web_search: available|unavailable]`).

---

### Implementation checklist

- [ ] Add `WebSearch` variant to `SubtaskStepType` enum in `subtask.rs`
- [ ] Define `SearchResult` struct: `title: String`, `url: String`,
      `excerpt: String`; derive `Serialize, Deserialize, Clone, Debug`
- [ ] Define `WebSearchResult` struct: `query: String`,
      `results: Vec<SearchResult>`; derive same traits
- [ ] Define `WebSearchStepOutput` struct: `query: String`,
      `results: Vec<SearchResult>`, `approved_indices: Vec<usize>`
- [ ] Add `subtask_web_results` table migration in `store.rs` (idempotent):
      `id INTEGER PRIMARY KEY AUTOINCREMENT`, `step_id INTEGER NOT NULL`,
      `query TEXT NOT NULL`, `results_json TEXT NOT NULL`,
      `approved_indices_json TEXT`, `created_at TEXT NOT NULL DEFAULT (datetime('now'))`
- [ ] Add `web_search_query_log` table migration in `store.rs`:
      `id INTEGER PRIMARY KEY AUTOINCREMENT`, `query TEXT NOT NULL`,
      `step_id INTEGER`, `executed_at TEXT NOT NULL DEFAULT (datetime('now'))`.
      Insert trigger or post-insert cleanup: delete rows beyond the 50 most recent
- [ ] Implement `store.log_search_query(step_id, query) -> Result<()>`: inserts
      to `web_search_query_log`, deletes oldest rows beyond 50
- [ ] Implement `store.list_search_query_log() -> Result<Vec<String>>`:
      returns most recent 10 query strings
- [ ] Implement `store.clear_search_query_log() -> Result<()>`
- [ ] Add `search_api_call_count` and `search_api_call_window_start` to
      `app_settings` (string key-value, existing pattern)
- [ ] Implement `get_search_api_key() -> Option<String>`: reads from Keychain
      service `com.jeff.desktop.search_api_key`
- [ ] Implement `set_search_api_key(key: &str) -> Result<()>`: writes to Keychain
- [ ] Implement `check_and_increment_rate_limit(store) -> Result<bool>`:
      reads `search_api_call_count` and `search_api_call_window_start` from
      `app_settings`; if `window_start` is > 1 hour ago, reset count to 0 and
      update `window_start`; if count >= 10 return `false`; else increment count
      and return `true`
- [ ] Implement `execute_web_search_step(query: &str, api_key: &str) -> Result<Vec<SearchResult>>`:
      - HTTP GET to Brave Search API (or Serper) with query; 5-second timeout;
        parse top 5 results
      - For each result: HTTP GET page URL with 5-second timeout; strip HTML tags
        via simple regex `<[^>]+>`; take first 500 chars of plain text
      - If individual page fetch fails: include result with empty excerpt, do not
        fail the whole step
      - Return `Vec<SearchResult>`
- [ ] Implement `is_query_safe(query: &str, user_name: Option<&str>) -> bool`:
      returns `false` if `user_name` is Some AND query contains user_name
      (case-insensitive). Returns `true` otherwise.
- [ ] In `subtask.rs` `run_subtask_chain`: add `WebSearch` branch in step
      execution loop:
      - Check key: if None, log skip note to chain result, continue to next step
      - Check rate limit via `check_and_increment_rate_limit`: if false, log skip
        note, continue
      - Check max 3 web search steps per chain: if already 3 executed, skip
      - Check `is_query_safe`: if false, log skip, continue
      - Call `execute_web_search_step`
      - Store results in `subtask_web_results` and log query in
        `web_search_query_log`
      - Emit `subtask://web_results_pending` event with `step_id` and results
      - Pause chain execution: poll `subtask_web_results` for
        `approved_indices_json` not null OR status row marked skipped, every 2
        seconds, timeout 10 minutes → auto-reject on timeout
- [ ] Add tauri command `approve_web_search_results(state, step_id: i64, approved_indices: Vec<usize>) -> Result<()>`:
      updates `subtask_web_results.approved_indices_json`, unblocks chain
- [ ] Add tauri command `reject_web_search_results(state, step_id: i64) -> Result<()>`:
      marks step skipped in `subtask_web_results`, unblocks chain
- [ ] Add tauri command `get_pending_web_search_results(state, task_id: i64) -> Result<Vec<WebSearchStepOutput>>`
- [ ] Add tauri command `set_search_api_key(key: String) -> Result<()>`
- [ ] Add tauri command `get_search_api_configured() -> Result<bool>`
- [ ] Add tauri command `get_search_call_count() -> Result<i64>`
- [ ] Add tauri command `get_search_query_log(state) -> Result<Vec<String>>`
- [ ] Add tauri command `clear_search_query_log(state) -> Result<()>`
- [ ] Register all commands in `main.rs` invoke handler
- [ ] Update chain planner system prompt in `subtask.rs`:
      add `web_search` to available step types with description "use when the
      request requires information not available in workspace files — provide a
      specific topic query, not a broad subject"; add
      `[web_search: available|unavailable]` line to the tool availability section
      of the prompt based on key presence
- [ ] Update `llm_call` step execution in `subtask.rs`: when the prior step was
      an approved `web_search` step, inject the approved `SearchResult` list as
      additional context. Append to system prompt: "You have been given the
      following web sources. Cite each claim that comes from a source as 'Title
      (URL)' inline. Do not fabricate citations. If you cannot attribute a claim
      to a provided source, omit the citation."
- [ ] In `Overlay.tsx`: add web results card component. Renders on
      `subtask://web_results_pending` event. Shows: query string in a
      `<code>`-style label, list of up to 5 results each with title (linked),
      URL in gray text, excerpt. Checkbox per result (all checked by default).
      "Approve Selected" button calls `approveWebSearchResults(stepId, selectedIndices)`.
      "Reject" button calls `rejectWebSearchResults(stepId)`.
- [ ] In Privacy Center (`App.tsx` or `Overlay.tsx`): add "Web search" section
      with: password input field for API key with Save button calling
      `setSearchApiKey`; configured status indicator; call count display
      "N / 10 this hour"; query log list (last 10 queries, most recent first);
      "Clear log" button calling `clearSearchQueryLog`
- [ ] Write unit test `rate_limit_blocks_at_10`: call
      `check_and_increment_rate_limit` 10 times in same window → 11th call returns
      `false`
- [ ] Write unit test `rate_limit_resets_after_window`: set
      `search_api_call_window_start` to 2 hours ago, count to 10 →
      `check_and_increment_rate_limit` returns `true` (window reset)
- [ ] Write unit test `query_safety_blocks_user_name`:
      `is_query_safe("research on Krish Malik", Some("Krish Malik"))` → `false`
- [ ] Write unit test `chain_pauses_on_web_search_step`: run chain with a
      `WebSearch` step → chain status is "waiting_approval" after step executes,
      not "complete"
- [ ] Write unit test `approved_results_injected_into_next_llm_step`: approve
      web results → next `llm_call` step system prompt contains result excerpts
- [ ] Write `scripts/phase34_check.sh`:
      - `WebSearch` variant in `SubtaskStepType` in `subtask.rs`
      - `subtask_web_results` and `web_search_query_log` migrations in `store.rs`
      - `execute_web_search_step` in `subtask.rs`
      - `approve_web_search_results` in `commands.rs`
      - `get_search_api_configured` in `commands.rs`
      - behavioral assertion: with a valid search API key configured, trigger a
        subtask chain that includes a `WebSearch` step →
        `get_pending_web_search_results` returns one record with results before
        approval

---

### Exit criteria (all behavioral)

1. User says "find three sources that support the argument that containment was
   economically motivated." Jeff runs a subtask chain with a `web_search` step.
   A web results card appears in the companion bar showing the query and up to 5
   results with titles, URLs, and excerpts. User selects 3 and approves. The next
   step produces a draft paragraph incorporating those sources with inline
   citations. The citations are real URLs from the results, not fabricated.

2. No search API key configured. User triggers a web-search subtask. Companion
   bar shows "web search unavailable — add a search API key in settings" with a
   link to the Privacy Center field. No API call is attempted.

3. Rate limit reached (10 calls in the current hour). Next web search step is
   skipped. Companion bar shows "web search limit reached for this hour." Chain
   continues with remaining non-search steps.

4. User rejects web search results. Chain step is marked skipped. Next step
   proceeds without web content. No sources appear in the output.

5. Privacy Center shows the last 10 queries executed. Clear log wipes them. Call
   count reflects actual API calls made in the current hour window.

6. A subtask chain with a `web_search` step followed by an `llm_call` step
   produces output where every cited source URL matches one of the approved
   search results. No fabricated citations appear.

---

## v2 completion definition

Phases 25–34 are complete when all of the following are true:

- Jeff's voice is consistent and recognizable across all output surfaces (Phase 25)
- Jeff holds a continuous model of what the user is working on that persists
  across turns without reassembly (Phase 26)
- Jeff decides when to speak from judgment, producing one synthesized message
  that integrates all available signals (Phase 27)
- Jeff's proactive messages are conversation openers, not notifications — the user
  can reply to them (Phase 28)
- Every Jeff output — revision, draft, suggestion — leads with Jeff's own
  assessment of the tradeoff made (Phase 29)
- Jeff tracks the user's stated goals across sessions and adjusts how it expresses
  opinions based on observed collaboration preferences (Phase 30)
- Jeff can read the active document content when opt-in is enabled, and the
  synthesis layer uses this for richer situational awareness (Phase 31)
- Jeff's character consistency is verified by an automated eval harness that runs
  as part of the release check matrix (Phase 32)
- Jeff can write approved subtask output directly into the user's active document
  in Google Docs, Pages, and Word without the user manually copying anything
  (Phase 33)
- Jeff can search the web inside a subtask chain, surface results for user
  approval, and incorporate approved sources with real citations into drafted
  content (Phase 34)

The product test from VISION.md: "You should feel like someone smart has been in
the room the whole time, and when they say something, it's because something is
worth saying."

---

## Deferred after Phases 25–34

These are explicitly out of scope for this transformation plan. They are noted
here to prevent scope creep.

- **Emotion or affect detection**: Jeff does not attempt to read the user's emotional
  state from typing patterns, voice tone, or content.
- **Autonomous task initiation without user input**: Jeff does not start tasks the
  user has not asked for. Speculative subtasks (Phase 15) remain the boundary.
- **Cross-session goal completion tracking beyond patterns**: tracking multi-week
  goal arcs across many sessions is a more complex relational model feature deferred
  until Phase 30 signals are proven accurate in the wild.
- **Proactive learning requests**: Jeff does not ask the user to teach it about
  their work style. The relational model infers from observation, not interrogation.
- **Multi-user or shared awareness**: Jeff is a personal coworker. Team features
  require a backend service. Out of scope.
- **Cross-app content capture beyond Accessibility API**: Phase 31 uses the macOS
  AX API. Apps that restrict AX text access degrade gracefully to title-only. A
  different capture mechanism (e.g., screenshot OCR) is deferred — the AX path
  covers native document apps, which is the primary use case.
- **Automated character regression on every commit**: Phase 32 runs
  `character_eval.sh` as part of the release check matrix, not on every commit.
  Per-commit character regression would require a faster, cheaper eval path and
  is deferred until the eval harness is proven accurate over several release cycles.
- **Full document text injection into LLM context**: Phase 31 injects a compact
  content observation summary, not the raw document text. Injecting full document
  text raises token cost and privacy surface significantly. Deferred as an opt-in
  advanced mode once the compact summary path is validated.
- **Cross-app write-back beyond Google Docs, Pages, and Word**: app-specific
  variability in accessibility API write support makes broader coverage a post-v2
  problem. Phase 33 covers the apps that account for the majority of the target
  user's writing.
- **Full-page web retrieval beyond 500-char excerpts**: the excerpt is sufficient
  for citation and context-informed drafting. Deep document ingestion from the web
  is a separate capability with different token and latency tradeoffs.
- **Automated search API key provisioning**: the user must supply their own key
  in v2. Bundled search is a post-v2 commercial consideration.
- **Real-time citation verification**: confirming that cited URLs still resolve
  and content matches at time of use is deferred. Phase 34 citations are real
  at time of retrieval; freshness checking is a future feature.
