# Jeff — Synthesis Layer Architecture

This document specifies the awareness core and synthesis layer: the architectural
component that reads all available signals simultaneously and produces a coherent
answer to "what is actually going on with this person right now, and what would
be genuinely valuable to say or do?"

This is the component that is most architecturally absent from Jeff v1. The
signals exist. The handlers exist. Nobody is reading the whole picture.

---

## The problem this solves

Jeff v1 has the following signal sources, each with its own handler:

- Active window title → context_observer.rs → prepended to chat prompts
- Calendar events → calendar.rs → companion header badge
- Workspace files → watcher.rs → context pack retrieval
- Task focus log → proactive.rs → reorientation trigger
- Drift detection → proactive.rs → drift banner
- Subtask status → subtask.rs → companion subtask row
- User model → user_model.rs → profile injection
- Conversation history → store.rs → context pack

Each of these signals feeds its own handler. None of them reads the others.
The result: three separate timers might fire three separate notifications on the
same 60-second tick, each unaware of the others, when what the user needs is
one synthesized observation.

The synthesis layer reads all signals simultaneously. It holds a persistent
`SituationalSnapshot` — updated incrementally, not assembled per-request — and
produces two things: a structured summary of current state that enriches all
LLM calls, and a judgment about whether something is worth saying proactively.

---

## Where it lives

`desktop/src-tauri/src/awareness_core.rs`

This is a new module, not an extension of an existing one. It depends on:
- `store.rs` (recent messages, focus log, subtask state)
- `state.rs` (AmbientState for window context, calendar state)
- `user_model.rs` (profile for trigger weight adjustments)
- `proactive.rs` (for the proactive delivery path, after judgment)
- `character.rs` (for snapshot summary injection into system prompts)

The `AwarenessCore` struct is added to `AppState` alongside existing state.

---

## Data structures

```rust
// the persistent situational model
pub struct SituationalSnapshot {
    // what the user is working toward (explicit statement or inferred from recent turns)
    pub current_goal: Option<String>,

    // what was accomplished recently (last accepted revision, last completed subtask)
    pub recent_progress: Option<String>,

    // what appears to be blocking progress
    pub current_blockers: Vec<String>,

    // current attention state
    pub attention_state: AttentionState,

    // pending items requiring user action
    pub pending_work: Vec<PendingItem>,

    // time pressure from calendar or stated deadlines
    pub time_pressure: Option<TimePressure>,

    // when the last substantive exchange happened (unix timestamp, seconds)
    pub last_meaningful_turn: Option<i64>,

    // 0.0 = no signal, 1.0 = full signal. used to gate proactive speech
    pub snapshot_confidence: f32,

    // when this snapshot was last updated (unix timestamp)
    pub updated_at: i64,

    // which trigger caused the most recent update (for logging)
    pub trigger: String,
}

pub enum AttentionState {
    Focused,    // recent messages + window matches task + no drift signal
    Drifting,   // drift flag within cooldown window, or window doesn't match task
    Returning,  // absence gap > REORIENTATION_MIN_ABSENCE_SECONDS (300)
    Idle,       // no signals in last 30+ minutes, no active turn
}

pub struct PendingItem {
    pub item_type: String,      // "file_write_proposal" | "subtask_result" | "live_edit"
    pub description: String,
    pub created_at: i64,
}

pub struct TimePressure {
    pub source: String,         // "calendar" | "stated_deadline"
    pub description: String,    // e.g. "Design review in 45 min" or "wants done by midnight"
    pub minutes_until: Option<i64>,
}

pub enum SnapshotTrigger {
    NewTurn,            // after each conversation turn completes
    FocusEvent,         // on task focus log write (user returns to work)
    WindowSwitch,       // on active window title change
    SubtaskCompleted,   // on subtask chain completion
    CalendarEvent,      // on calendar state change (new event within threshold)
    TimeTick,           // ambient monitor 60-second tick
}
```

---

## The update function

```rust
pub async fn update_snapshot(
    trigger: SnapshotTrigger,
    task_id: i64,
    state: &AppState,
    ambient: &AmbientState,
) -> SituationalSnapshot
```

**This function does not make an LLM call.** It is deterministic assembly from
structured signals. The LLM reads the snapshot summary — the snapshot itself is
computed without inference.

Rationale: if the update function requires an LLM call, it cannot run on every
turn and every tick without significant latency and cost. The snapshot is the
structured state; the LLM is the consumer of it.

**Assembly logic:**

`current_goal`:
1. Scan the last 10 messages for explicit goal statements (heuristic: message
   contains "I'm working on", "I need to", "I'm trying to", "my goal is",
   "I want to"). Take the most recent match.
2. Fall back to the task title if no explicit statement found.

`recent_progress`:
1. If the last assistant message has `message_kind = "subtask_result"` or
   `"revision_accepted"`, extract a 1-sentence summary from `result_summary`.
2. Otherwise: None.

`attention_state`:
- `Returning` if `now - last_focus_at > 300` seconds
- `Drifting` if last drift flag was within the drift cooldown window (900s)
- `Focused` if last message was within 120 seconds AND active window app_name
  is not empty (user is in a document app)
- `Idle` otherwise

`current_blockers`:
- If `attention_state == Drifting`, include the drift flag reason as a blocker
- If any `PendingItem` has been waiting > 300 seconds, note it as a blocker
  ("waiting on your decision about [item]")
- Otherwise: empty vec

`pending_work`:
- Query `subtask_file_write_proposals` for `status = "pending_approval"`, active task
- Query `subtasks` for `result_review_status = "unreviewed"`, active task
- Build `PendingItem` for each

`time_pressure`:
1. Check `CalendarState` for the next event — if within 120 minutes, populate
   `TimePressure { source: "calendar", ... }`.
2. Scan recent messages for deadline statements (heuristic: message contains
   "by midnight", "by tomorrow", "due at", "deadline is") — if found, populate
   `TimePressure { source: "stated_deadline", ... }`.
3. First match wins.

`last_meaningful_turn`:
- Most recent message timestamp from `store.get_recent_messages(task_id, 1)`.

`snapshot_confidence`:
- Start at 0.0
- Active task exists: +0.20
- `current_goal` is Some: +0.20
- `last_meaningful_turn` within 3600 seconds: +0.20
- Active window context is available: +0.20
- `recent_progress` is Some or `pending_work` is non-empty: +0.20
- Max: 1.0

---

## The snapshot summary

```rust
pub fn snapshot_summary(snapshot: &SituationalSnapshot) -> String
```

Produces a natural-language block under 150 tokens. This is injected into all
LLM system prompts via `character::build_chat_system_prompt()`.

Format (omit lines where data is None or empty):

```
current situation: [attention_state description]
working toward: [current_goal]
recent progress: [recent_progress]
blockers: [current_blockers joined by "; "]
pending decisions: [pending_work descriptions]
time pressure: [time_pressure.description]
```

Example output:
```
current situation: returning after 8 minutes away
working toward: finish the introduction before the 10pm deadline
recent progress: completed the background section draft
pending decisions: file write proposal waiting (outline.md)
time pressure: Design review meeting in 42 minutes
```

If `snapshot_confidence < 0.3`, returns an empty string — nothing is injected.

---

## The synthesis judgment

```rust
pub fn should_speak_proactively(
    snapshot: &SituationalSnapshot,
    profile: &UserProfile,
    last_proactive_at: Option<i64>,
    now: i64,
) -> Option<ProactiveSpeechReason>
```

**This function also does not make an LLM call.** It is a decision function.
The content of what Jeff says is determined by `synthesize_proactive_message()`,
not here.

```rust
pub enum ProactiveSpeechReason {
    TaskReturn { idle_minutes: u64 },
    DeadlinePressure { event: String, minutes_until: i64 },
    BlockerDetected { blocker: String },
    WorkQualityObservation { observation: String },  // currently reserved for future use
}
```

**Decision logic (in order):**

1. If `snapshot.snapshot_confidence < 0.3`: return None. Not enough signal to
   say anything useful.

2. If `last_proactive_at` is Some AND `now - last_proactive_at < PROACTIVE_COOLDOWN (600)`:
   return None. Cooldown prevents rapid-fire interruption.

3. Read `profile.trigger_weight_reorientation` (from user_model.rs). If the user
   has down-weighted this (repeated dismissals), raise the idle threshold from
   5 to 10 minutes.

4. If `snapshot.attention_state == Returning` AND idle gap > threshold:
   return `TaskReturn { idle_minutes }`.

5. If `snapshot.time_pressure` is Some AND `minutes_until < 90`:
   return `DeadlinePressure`.

6. If `snapshot.current_blockers` is non-empty AND last message > 600 seconds ago:
   return `BlockerDetected { blocker: blockers[0] }`.

7. Return None.

**The signal integration that makes this different from v1:**

In v1, each trigger evaluator runs independently. If all three fire, three
notifications go out. Here, `should_speak_proactively` returns at most one reason,
by priority. But the snapshot fed to `synthesize_proactive_message()` contains
all three signals. The synthesized message can reference all of them in one
sentence: "You've been away for a while and have a meeting in 40 minutes — your
introduction draft is still open."

---

## The synthesis call

```rust
pub async fn synthesize_proactive_message(
    reason: &ProactiveSpeechReason,
    snapshot: &SituationalSnapshot,
    api_key: &str,
) -> Result<String>
```

This IS an LLM call. It generates the message Jeff speaks.

- System prompt: `character::build_reorientation_system_prompt()`, which includes
  the character base prompt + the snapshot summary
- User prompt: a brief description of the reason (e.g. "The user has been away for
  8 minutes. Their next meeting is in 42 minutes. They have an open file write
  proposal.")
- Instruction: "In 1-2 sentences, speak as a coworker who has been watching.
  Reference the specific situation. Do not be a system status message. Start a
  conversation."
- Output budget: 1-2 sentences, maximum 40 words

---

## The synthesis log

Every proactive speech decision is logged, including suppressed ones.

```sql
CREATE TABLE IF NOT EXISTS synthesis_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id INTEGER,
    reason_type TEXT NOT NULL,           -- "task_return" | "deadline_pressure" | "blocker" | null (suppressed)
    reason_detail TEXT,
    snapshot_confidence REAL NOT NULL,
    snapshot_attention_state TEXT NOT NULL,
    message TEXT,                        -- null if suppressed
    delivered INTEGER NOT NULL DEFAULT 0,
    delivered_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

This serves two purposes:
1. It feeds the `last_proactive_at` cooldown check.
2. It is surfaced in the Privacy Center audit view so users can see every time
   Jeff decided to speak (or decided not to).

---

## How it integrates with existing systems

**Replaces:**
- The three separate condition checks in the ambient monitor
  (`check_reorientation_from_background`, `check_stuck_from_background`,
  `check_drift_from_background`) are replaced by a single
  `check_synthesis(state, ambient, app)` call that uses
  `should_speak_proactively()` + `synthesize_proactive_message()`.

**Extends:**
- `character::build_chat_system_prompt()` gains a `snapshot_summary()` call —
  the snapshot is injected into every LLM chat turn, making the model aware of
  current state without per-turn retrieval.
- The proactive delivery path in `proactive.rs` gains
  `deliver_proactive_as_chat_message()` — proactive messages land in the
  conversation, not in a separate banner.

**Does not replace:**
- The context pack (RAG retrieval). The snapshot summary and the retrieved chunks
  are both injected — they answer different questions. Retrieval answers "what is
  relevant from my files?" The snapshot answers "what is happening right now?"
- The user model (user_model.rs). The relational model (Phase 30) sits above both.

---

## Failure modes and mitigations

**Low confidence / no active task:**
`snapshot_confidence < 0.3` → `should_speak_proactively` returns None. Jeff
stays silent. This is the correct behavior when the user hasn't given Jeff
enough context yet.

**Stale signals:**
The `updated_at` field guards against stale data. If the snapshot has not been
updated in > 120 seconds and the trigger was not `TimeTick`, treat confidence
as 0 until next update.

**Proactive spam:**
The 600-second cooldown in `should_speak_proactively` plus the user model's
`trigger_weight_reorientation` combine to prevent repeated interruptions. If
the user dismisses proactive messages repeatedly, the threshold raises to
10 minutes.

**LLM failure in synthesis call:**
If `synthesize_proactive_message()` fails (timeout, API error), log the failure
in `synthesis_log` with `delivered = false` and suppress silently. Do not fall
back to a canned message — a bad synthesis is worse than silence.

**Thread safety:**
`AwarenessCore` holds a `tokio::sync::Mutex<SituationalSnapshot>`. Updates are
async and non-blocking. The ambient monitor holds the lock for the update, then
releases it before dispatching. Chat turns that read the snapshot via
`snapshot_summary()` take a short read lock.

---

## Phase 31 addendum: live content observation inputs

This section describes how Phase 31's active document capture integrates with
the snapshot assembly and summary functions specified above. The original sections
are not modified — this addendum extends them.

---

### New snapshot fields (Phase 31 addition)

`SituationalSnapshot` gains two fields after Phase 31. The original 9-field
struct becomes 11 fields:

```rust
// compact natural-language description of the active document's content state.
// ≤ 100 tokens. format: "~[word_count] words, [draft_state] draft, [change phrase]"
// example: "~840 words, mid-draft, content changed recently"
// None when content observation is disabled or no capture has occurred.
pub active_document_excerpt: Option<String>,

// seconds the document content has been unchanged across consecutive polls.
// computed as stable_for_ticks * CONTENT_OBSERVATION_POLL_INTERVAL_SECONDS (10).
// None when content observation is disabled.
pub content_idle_seconds: Option<u32>,
```

Both fields default to `None`. They are populated only via the
`SnapshotTrigger::ContentObservation` trigger path.

---

### New trigger variant (Phase 31 addition)

`SnapshotTrigger` gains a seventh variant:

```rust
ContentObservation,  // fired after each content observation poll (every 10s)
```

This trigger is fired by `run_content_observation_poll` in `context_observer.rs`
after each successful or unsuccessful poll, via a non-blocking `spawn`. It is
the only trigger that populates `active_document_excerpt` and `content_idle_seconds`.

---

### How `AwarenessCore::update()` handles `ContentObservation`

When `trigger == SnapshotTrigger::ContentObservation`, the update function
acquires a read lock on `AppState.content_observation` (a new field of type
`Arc<Mutex<ContentObservationState>>` added in Phase 31) and reads the most
recent `ContentObservation` struct. It does not re-run the content poll — it
reads the result that was already computed by `summarize_content_observation()`
in `context_observer.rs`.

Assembly logic for the two new fields:

`active_document_excerpt`:
- If `ContentObservationState.observation` is Some:
  - Format: `"~{word_count} words, {draft_state} draft, {change_phrase}"` where:
    - `draft_state` is "early" / "mid" / "late" from `DraftState` enum
    - `change_phrase` is "content changed recently" if `content_changed == true`
      and `stable_for_ticks == 0`; "no recent changes" if `stable_for_ticks >= 3`;
      otherwise omit the phrase (brief stable, ambiguous)
  - Truncate to 100 chars
- If `ContentObservationState.observation` is None: set to None

`content_idle_seconds`:
- If observation is Some: `stable_for_ticks * 10` (where 10 is
  `CONTENT_OBSERVATION_POLL_INTERVAL_SECONDS`)
- If observation is None: None

**Confidence bonus:** when `active_document_excerpt` is Some after assembly,
add `+0.10` to `snapshot_confidence`, capped at `1.0`.

**Privacy boundary:** `context_observer.rs` holds the raw captured text in
`ContentObservationState.raw_text` (memory only, never written to SQLite).
`AwarenessCore::update()` receives only the pre-computed `ContentObservation`
struct — never the raw text. The raw text crosses no boundary into the awareness
core, the LLM prompt, or any log table.

---

### How `snapshot_summary()` renders the new field

When `active_document_excerpt` is Some, `snapshot_summary()` appends one line:

```
active document: [active_document_excerpt]
```

The 600-character budget (150-token proxy) already in place governs the total.
If the summary is near budget, `active_document_excerpt` is truncated to 60
characters before inclusion.

The line is omitted entirely when `active_document_excerpt` is None.

Example full summary with content observation enabled:

```
current situation: returning after 8 minutes away
working toward: finish the introduction before the 10pm deadline
recent progress: completed the background section draft
pending decisions: file write proposal waiting (outline.md)
time pressure: Design review meeting in 42 minutes
active document: ~840 words, mid-draft, no recent changes
```

---

### How `should_speak_proactively()` uses content observation

Phase 31 activates the `WorkQualityObservation` variant that was reserved in
Phase 27's `ProactiveSpeechReason` enum. It is added as the final (lowest-
priority) check in `should_speak_proactively()`, evaluated only after all Phase
27 checks have returned `None`:

```rust
// lowest priority — only fires when content observation is active and content
// has been idle long enough to suggest the user may be stuck.
if snapshot.content_idle_seconds >= Some(60)
    && snapshot.attention_state == AttentionState::Focused
    && snapshot.last_meaningful_turn
        .map(|turn| now.saturating_sub(turn) > 300)
        .unwrap_or(false)
    && snapshot.snapshot_confidence >= 0.3
{
    return Some(ProactiveSpeechReason::WorkQualityObservation {
        observation: "content unchanged for a while".to_string(),
    });
}
```

This check does not fire if:
- `content_idle_seconds` is None (feature disabled)
- Attention state is not Focused (returning / idle / drifting users should get
  the TaskReturn or BlockerDetected path instead)
- The user has had a recent conversation turn (< 300s) — they may be reading
  rather than stuck
- Confidence is below 0.3 (the base gate for all proactive speech)

The 60-second idle threshold corresponds to 6 consecutive content polls with no
detected change. `synthesize_proactive_message()` for this reason type produces
a message that references the document state: "You've been looking at this
without changes for a bit — stuck somewhere?" The snapshot summary (which now
includes the active document excerpt) gives the LLM sufficient context to make
the message specific rather than generic.

---

## Phase 33 addendum: insertion point inference

This section describes how Phase 33's document write-back integrates with the
snapshot and synthesis layer. The original sections and the Phase 31 addendum
are not modified — this addendum extends them.

---

### Insertion point inference from `active_document_excerpt`

When a subtask chain completes with a content result and `active_document_excerpt`
is Some (i.e., Phase 31's content observation is enabled and has run at least
once), the synthesis layer informs the write-back path with a best-guess
insertion point.

This inference is **not** a new LLM call. It is a deterministic heuristic
function `infer_insertion_point(content: &str, excerpt: &str) -> Option<String>`
in `subtask.rs` that reads the `active_document_excerpt` string and returns a
plain-language suggestion:

```rust
// heuristic: excerpt signals "introduction" territory → suggest after intro
if excerpt.contains("introduction") || excerpt.contains("intro") || excerpt.contains("opening") {
    return Some("after your introduction paragraph".to_string());
}
// heuristic: large word count → content is late-draft, append to end
if let Some(word_count) = parse_word_count_from_excerpt(excerpt) {
    if word_count > 800 {
        return Some("at the end of the document".to_string());
    }
}
// no clear signal — no suggestion
None
```

The result, when Some, is included in `DocumentWriteRequest.anchor_context`
prefixed with `"inferred: "` to distinguish it from an explicit anchor provided
by the user or the insertion point selected via the AX read path.

---

### What the approval card shows

The approval card for a write-back action (`document_write://approval_requested`)
shows the inferred insertion point as a suggestion, not a guaranteed location.

When `anchor_context` starts with `"inferred: "`, the card renders:

```
Suggested: after your introduction paragraph
(This is Jeff's best guess based on your document — adjust if needed)
```

When `anchor_context` is a confirmed 50-char surrounding context match from the
AX read path, the card renders:

```
After: "...the argument rests on three assumptions..."
```

The distinction matters: an inferred anchor is a suggestion for the user to
evaluate; a confirmed anchor is a precise match that the write path will seek.
If the confirmed anchor is not found at write time (the document changed), the
system falls back to guided-apply regardless of whether the anchor was inferred
or confirmed.

---

### What the synthesis layer does not do

The synthesis judgment (`should_speak_proactively`) is not modified by Phase 33.
The insertion point inference happens only on the write-back delivery path
(when a subtask result is ready to be placed), not as a proactive speech trigger.
Jeff does not speak proactively to announce that it has inferred an insertion
point. The inference is surfaced only in the approval card, initiated by user
action (approving the subtask result).
