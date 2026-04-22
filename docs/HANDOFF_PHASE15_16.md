# Jeff — Phase 15 & 16 Handoff

## How to use this document

This is a planning handoff for the next Claude Code session. Read it in full before
writing any code. The session's job is to complete Phase 15 (Proactive Initiation) and
Phase 16 (Richer Parallel Work). Both phases have detailed scope and exit criteria in
`docs/PHASES.md`. This document provides the full technical context, existing code
patterns, and recommended milestone breakdown needed to build a thorough implementation
plan.

---

## 1. Mandatory session rules (from CLAUDE.md)

These are non-negotiable. Follow them exactly:

1. Read `docs/VISION.md` in full before starting. All technical decisions must serve the
   five felt properties.
2. Read `docs/PHASES.md` to confirm current phase status.
3. Read `docs/ARCHITECTURE.md` for current layering.
4. Verification-first: never claim a milestone complete without running the relevant
   phase check script (`scripts/phaseN_check.sh`), all tests passing, and a confirmed
   runtime proof.
5. Work one milestone at a time. Do not combine milestones.
6. Show the plan before writing code. Wait for explicit approval before editing.
7. Preserve existing code paths. Add minimal fallbacks or guards, do not do large rewrites
   unless the current phase plan explicitly calls for one.
8. Do not expand backend capability beyond the current phase's scope.
9. No emojis anywhere. All code comments in lowercase. TypeScript frontend, Rust backend.

---

## 2. Product vision (abridged)

Jeff is a coworker, not a tool. Five felt properties that every phase must serve:

1. **Already present** — lives in tray, one keypress away, never launched.
2. **Already knows your task** — no briefing, no context pasting.
3. **Can interrupt and be interrupted** — mid-sentence, either direction.
4. **Does parallel work** — subtasks run while the user keeps going.
5. **Initiates conversation** — orients on return, flags drift, suggests next moves.

Phases 15 and 16 together complete properties 4 and 5 at a level of fidelity that makes
Jeff feel genuinely like a coworker rather than a reactive chatbot.

---

## 3. Current state (phases 0–14 complete)

### Verified non-regression baseline from phases 13–14 (must stay green)

- `scripts/phase13_check.sh` currently enforces:
  - active-task watcher synchronization (`ensure_workspace_awareness_for_task`)
  - startup watcher restore (`restore_workspace_awareness_for_active_task`)
  - remove/rename event handling in watcher ingest (`EventKind::Remove`, `ModifyKind::Name`)
  - clipboard poll ownership and disable guards (`clipboard_polls`, `if !enabled`)
  - idempotent watcher ingest using high-resolution file versioning (`as_nanos` + size/hash)
- `scripts/phase14_check.sh` currently enforces:
  - model classifier + explicit keyword fallback path
  - unknown-intent clarification branch in `App.tsx`
  - slot passthrough including `slots?.target_description` for revision targeting
  - explicit timeout constants in frontend and backend (`INTENT_CLASSIFIER_TIMEOUT_MS`, `REQUEST_TIMEOUT_MS`)
- `desktop/src-tauri/tests/intent_eval.rs` budgets:
  - intent accuracy: `>= 90%`
  - latency: `p50 < 150ms`, `p95 < 450ms`
  - slot-quality checks for all four slots
- Phase 15/16 implementation must preserve all of the above (do not regress M13/M14 while adding new capability).

### Completed infrastructure relevant to phases 15–16

**Store (`desktop/src-tauri/src/store.rs`)**
- SQLite via `rusqlite` (bundled), WAL mode, FK enforcement
- 18 tables total: `tasks`, `sessions`, `task_summaries`, `artifacts`,
  `artifact_chunks`, `chat_messages`, `artifact_revisions`, `artifact_versions`,
  `subtasks`, `session_mode_state`, `suggestions`, `app_settings`,
  `open_resources`, `event_log`, `watched_folders`, `watched_file_registry`,
  `clipboard_capture_settings`, `recently_learned_log`
- Schema versioning via `CREATE TABLE IF NOT EXISTS` + idempotent `execute_batch`
- Key methods used by phases 15–16 planning:
  - `create_subtask(NewSubTaskInput)` → `SubTaskDto`
  - `transition_subtask_status(subtask_id, status, result_summary, result_payload, error)`
  - `get_subtask_by_id(subtask_id)` → `Option<SubTaskDto>`
  - `list_subtasks(task_id)` → `Vec<SubTaskDto>`
  - `list_recent_chat_messages(task_id, limit)` → `Vec<ChatMessageDto>`
  - `append_chat_message(task_id, role, source, kind, content)` → `ChatMessageDto`
  - `get_active_task()` → `Option<TaskDto>`
  - `get_app_setting(key)`, `set_app_setting(key, value)`, `delete_app_setting(key)`
  - `get_app_setting_bool(key)` → `Option<bool>`
  - `record_event(tx, task_id, event_type, payload_json)` (private, used in transactions)
  - `append_recently_learned(task_id, source, label, preview)`
  - `upsert_file_registry_entry(task_id, canonical_path, artifact_id, version_tag)`
  - `replace_artifact_chunks(task_id, artifact_id, chunks)`
- `TaskStore` is `Clone` (wraps `StorePaths` struct, reconnects per-call)
- Transaction pattern: `conn.transaction()` → `tx.execute(...)` → `tx.commit()`
- `ON CONFLICT ... DO UPDATE` used for upserts

**State (`desktop/src-tauri/src/state.rs`)**
```rust
pub struct JeffState {
    pub store: TaskStore,
    pub embeddings: Arc<dyn EmbeddingProvider>,
    pub reasoning: Arc<dyn ReasoningProvider>,
    pub voice: Arc<OpenAiVoiceProvider>,
    pub interaction_epoch: Arc<AtomicU64>,
    pub coworking: Arc<Mutex<CoworkingRuntime>>,
    pub subtasks: Arc<SubTaskRunner>,
    pub interactions: SharedRegistry,        // phase 12 streaming
    pub watcher: Arc<Mutex<WatcherState>>,   // phase 13 workspace watcher
}
```
Adding new state fields: follow the exact same pattern. Add to `JeffState`, initialize
in `JeffState::new`, add `use crate::module_name::TypeName` imports.

**CoworkingRuntime (`desktop/src-tauri/src/coworking.rs`)**
- Tracks `CoworkingState`: `Idle`, `Listening`, `Thinking`, `Speaking`,
  `SilentObserving`, `AwaitingUser`, `Suppressed`
- `CoworkingConfig`: `proactive_mode: bool`, `pause_threshold_seconds: u64`,
  `nudge_cooldown_seconds: u64`, `interruption_suppression_seconds: u64`,
  `low_confidence_suppression_seconds: u64`, `min_retrieval_confidence: f32`
- `evaluate_proactive_nudge_for_task(store, embeddings, reasoning, task_id, coworking_status)` → `ProactiveEvaluationDto`
- Existing nudge system: fires `NO_NUDGE` from LLM when context is weak, fires a short
  advisory sentence otherwise. Phase 15 extends this — do not break it.
- `CoworkingRuntime.status(unix_now)` → `CoworkingStatusDto`
- Quiet mode already exists as `session_mode` in store (`session_mode_state` table),
  but Phase 15 needs a global process-level quiet mode that suppresses ALL proactive
  surfaces. This is distinct from per-task session mode.

**SubTaskRunner (`desktop/src-tauri/src/subtask.rs`)**
- `SubTaskRunner { cancellation_flags: Arc<Mutex<HashMap<i64, Arc<AtomicBool>>>> }`
- `start_subtask(store, reasoning, subtask_id)` → spawns `thread::spawn`
- `request_cancel(subtask_id)` → sets AtomicBool
- `run_subtask_execution(store, reasoning, subtask_id, cancel_token)` (private):
  - loads snapshot from `parent_context_snapshot` JSON
  - calls LLM for `SubTaskOutputJson` (`result_summary`, `result_payload`,
    `grounding_notes`, `confidence`)
  - transitions subtask status to `completed` or `failed`
- `normalize_execution_type(s)` validates: `draft_generation`, `expansion`,
  `synthesis`, `targeted_research_synthesis`
- Subtask status lifecycle: `pending → running → completed | failed | cancelled`
- `result_review_status`: `unreviewed → accepted | rejected | converted`
- Phase 16 extends `run_subtask_execution` significantly — see M16.2 below.

**Retrieval (`desktop/src-tauri/src/retrieval.rs`)**
- `build_task_context_pack(store, embeddings, task_id, query)` → `TaskContextPackDto`
- `retrieve_relevant_chunks(store, embeddings, task_id, query)` → `Vec<RetrievedChunkDto>`
- `retrieve_relevant_chunks_with_top_k(store, embeddings, task_id, query, top_k)` → `Vec<RetrievedChunkDto>`
- `auto_ingest_file_for_task(store, embeddings, task_id, path)` → `Result<()>` (phase 13)
  - version tag now uses `modified().as_nanos + file_size + content_hash`
- `import_artifact_for_task(store, embeddings, task_id, path)` → `ArtifactDto`

**Ambient (`desktop/src-tauri/src/ambient.rs`)**
- `dispatch_notification(app_handle, NotificationPayload)` → fires native OS notification
- `AmbientState` (Mutex): `tray_status`, `overlay_mode`, `quiet_mode: bool`,
  `notification_permission`
- Events emitted: `ambient://notification-dispatched`, `ambient://notification-suppressed`,
  `ambient://state-changed`
- Phase 15 uses `dispatch_notification` for push notifications on drift/orientation/stuck.
- Quiet mode: when `AmbientState.quiet_mode == true`, `dispatch_notification` is
  suppressed → emits `ambient://notification-suppressed` instead.
- `ambient_set_quiet_mode` command already exists (Phase 11). Phase 15 must hook into
  this for the global quiet mode exit criterion.

**Classifier (`desktop/src-tauri/src/classifier.rs`)**
- `classify_intent(text, api_key)` → `IntentClassificationDto` (phase 14)
- Calls gpt-4o-mini with `response_format: { type: "json_object" }`
- Backend timeout constant is explicit (`REQUEST_TIMEOUT_MS = 300`)
- Unknown/unsupported labels map to `IntentLabel::Unknown`
- Pattern reusable for Phase 15 LLM calls (drift evaluation, re-orientation generation)

**Reasoning provider (`desktop/src-tauri/src/reasoning.rs`)**
- `OpenAiReasoningProvider.generate_response(system_prompt, user_prompt)` → `Result<String>`
- Uses `reqwest::blocking::Client`, model `gpt-4o-mini`, temp=0
- Pattern for any synchronous LLM call in Rust backend

**Frontend: `App.tsx` patterns for Phase 15/16**
- State via `useState`, side-effects via `useEffect`
- Tauri events subscribed with `listen("event://name", handler)` (from `@tauri-apps/api/event`)
- Streaming events via `onLlmToken`, `onTtsChunk` etc. from `streamClient.ts`
- Error surface: `setOperationError(label, error)` and `setErrorMessage(msg)`
- Companion view is the primary surface for all new UI (not the full workspace)
- Existing companion sections: recent messages, send input, recently-learned panel,
  watcher status, clipboard toggle
- `activeTask` drives all task-scoped state; `useEffect([activeTask?.id])` pattern
  for loading per-task data
- Intent routing is currently centralized in `classifyMessageIntentWithFallback(...)`
  and preserves:
  - fallback to `inferMessageIntentKeyword(...)` on timeout/error
  - explicit unknown-intent clarification copy
  - slot-driven routing (`slots.instruction`, `slots.draft_type`, `slots.target_description`)

---

## 4. File tree (relevant to phases 15–16)

```
desktop/
  src/
    App.tsx                    ← main frontend (companion + full workspace)
    Overlay.tsx                ← overlay window frontend
    tauriClient.ts             ← all Tauri invoke wrappers + TS types
    ambientClient.ts           ← ambient_* command wrappers
    streamClient.ts            ← stream:// event listeners
  src-tauri/
    src/
      main.rs                  ← mod declarations + invoke_handler
      state.rs                 ← JeffState
      store.rs                 ← TaskStore (SQLite)
      models.rs                ← all DTOs
      commands.rs              ← all #[tauri::command] fns
      subtask.rs               ← SubTaskRunner + execution logic  ← HEAVY EDIT phase 16
      coworking.rs             ← CoworkingRuntime                 ← LIGHT EDIT phase 15
      ambient.rs               ← tray, overlay, notifications     ← READ-ONLY phase 15
      classifier.rs            ← intent classifier (phase 14)     ← pattern reference
      reasoning.rs             ← OpenAiReasoningProvider
      retrieval.rs             ← context pack + chunked retrieval
      watcher.rs               ← filesystem watcher (phase 13)    ← pattern reference
      revision.rs              ← artifact revision workflow
      chat.rs                  ← send_message_for_task
      chat_streaming.rs        ← streaming variant
      streaming.rs             ← InteractionToken + registry
      artifact_parser.rs       ← file → text chunks
      workspace.rs             ← slugify_title, path helpers
      lib.rs                   ← re-exports for integration tests
    tests/
      intent_eval.rs           ← phase 14 eval harness (pattern for phase 15 tests)
    Cargo.toml
scripts/
  phase13_check.sh
  phase14_check.sh
  phase15_check.sh             ← TO CREATE
  phase16_check.sh             ← TO CREATE
tests/
  fixtures/
    intent_eval_set.json
docs/
  VISION.md
  PHASES.md
  ARCHITECTURE.md
```

---

## 5. Phase 15: Proactive Initiation — detailed plan

### What this phase means for the product

Phase 15 completes felt property 5: *Jeff initiates conversation rather than only
responding.* The user returns to a task and Jeff says "here's where you left off."
The user's argument starts drifting and Jeff flags it. The user has been stuck and
Jeff suggests the next move. This is not reactive — Jeff watches and speaks unprompted.

### Existing foundation to leverage

- `CoworkingRuntime` already handles proactive nudge evaluation and quiet mode per
  session. Phase 15 adds a new trigger surface (task resume) and new signal types
  (drift, stuck) on top of this.
- `ambient.rs` `dispatch_notification` already exists and respects quiet mode.
- `evaluate_proactive_nudge_for_task` in coworking.rs already fires gpt-4o-mini
  with a restraint-focused system prompt. The re-orientation and drift prompts
  are different in character but the same infrastructure pattern.
- `event_log` table already captures task lifecycle events, useful for determining
  "last active" timestamp for resume detection.

### New DB tables needed (M15.1)

```sql
CREATE TABLE IF NOT EXISTS proactive_trigger_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id INTEGER NOT NULL REFERENCES tasks(id),
    trigger_type TEXT NOT NULL,   -- 'resume' | 'drift' | 'stuck'
    fired_at TEXT NOT NULL DEFAULT (datetime('now')),
    suppressed INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (task_id) REFERENCES tasks(id)
);

CREATE TABLE IF NOT EXISTS task_focus_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id INTEGER NOT NULL REFERENCES tasks(id),
    focused_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (task_id) REFERENCES tasks(id)
);
```

**Why `task_focus_log`:** Re-orientation fires when the user returns to a task. The
focus event is recorded when the frontend detects window focus + active task. The
backend checks `task_focus_log` to know if this is a genuine "return" (last focus >
some threshold ago, e.g., > 5 minutes) vs. a spurious re-fire.

**Why `proactive_trigger_log`:** Throttling. Each trigger type has a per-task cooldown.
Before firing a trigger, query the log: if the same trigger_type for this task_id fired
within the cooldown window, suppress it. This prevents spam.

New store methods:
- `record_task_focus(task_id)` → records current timestamp
- `get_last_task_focus(task_id)` → `Option<String>` (datetime string)
- `record_proactive_trigger(task_id, trigger_type, suppressed: bool)` → `i64`
- `get_last_proactive_trigger(task_id, trigger_type)` → `Option<String>` (fired_at)

New DTOs in `models.rs`:
```rust
pub struct ReorientationDto {
    pub task_id: i64,
    pub summary: String,         // short "where you left off" message
    pub fired_at: String,
}

pub struct DriftFlagDto {
    pub task_id: i64,
    pub is_drifting: bool,
    pub flag_reason: String,     // empty string when not drifting
    pub confidence: f32,
}

pub struct ProactiveTriggerDto {
    pub task_id: i64,
    pub trigger_type: String,    // "resume" | "drift" | "stuck"
    pub fired: bool,             // false if suppressed by throttle or quiet mode
    pub suppressed_reason: Option<String>,
}
```

### New module: `proactive.rs` (M15.2)

This module holds the three trigger evaluators:

**1. Re-orientation (`generate_reorientation`)**
```rust
pub fn generate_reorientation(
    store: &TaskStore,
    reasoning: &dyn ReasoningProvider,
    task_id: i64,
) -> Result<ReorientationDto>
```
- Checks `task_focus_log`: if last focus < 5 minutes ago, return early with no summary
  (not a genuine return).
- Checks `proactive_trigger_log` for `trigger_type = "resume"`: cooldown is 5 minutes.
  If within cooldown, return suppressed.
- Builds context: last 4 chat messages + task summary + artifact count.
- System prompt (keep it tight, under 30 words output): "You are Jeff. The user just
  returned to this task. Write one short sentence (max 25 words) summarizing where they
  left off. Be specific to the content. No commands."
- Fires `generate_response(system, user_prompt_with_context)`.
- Records trigger in `proactive_trigger_log`.
- Returns `ReorientationDto`.

**2. Drift detection (`evaluate_drift`)**
```rust
pub fn evaluate_drift(
    store: &TaskStore,
    reasoning: &dyn ReasoningProvider,
    embeddings: &dyn EmbeddingProvider,
    task_id: i64,
    current_text: &str,
) -> Result<DriftFlagDto>
```
- Checks cooldown in `proactive_trigger_log` for `trigger_type = "drift"` (15 minutes).
- Loads task summary (the stated goal).
- Calls `retrieve_relevant_chunks` for `current_text` — if similarity is high, user is
  on-track (short-circuit to `is_drifting: false`).
- If similarity is low, fires LLM with: task summary, current text, and a prompt asking
  it to return JSON `{ "is_drifting": bool, "reason": string, "confidence": float }`.
- Returns `DriftFlagDto`.

**3. Speculative subtask (`propose_speculative_subtask`)**
```rust
pub fn propose_speculative_subtask(
    store: &TaskStore,
    reasoning: &dyn ReasoningProvider,
    embeddings: &dyn EmbeddingProvider,
    task_id: i64,
) -> Result<Option<SubTaskDto>>
```
- Reuses `suggest_subtask_for_task` from `subtask.rs`, then proactively starts one bounded
  subtask via existing `create_subtask_and_start` using `instruction_source = "system"`.
- This aligns with Phase 15 scope in `docs/PHASES.md` ("kickoff in background") while
  preserving safety: no auto-merge/apply behavior is introduced.
- Checks cooldown for `trigger_type = "stuck"` (20 minutes).
- Checks that the user has been "stuck": last chat message > 10 minutes ago OR no
  messages in last N minutes. Use `event_log` or `chat_messages` timestamp.
- Guardrail: do not auto-start if there is already a `pending` or `running` subtask on
  the task (avoid compounding work while the user is already parallelizing).
- Returns `None` if not stuck or cooldown active.

### New Tauri commands (M15.3)

In `commands.rs`:
```rust
pub fn trigger_task_resume(state: State<'_, JeffState>, ambient: State<'_, AmbientState>, task_id: i64) -> Result<ReorientationDto, String>
pub fn check_task_drift(state: State<'_, JeffState>, ambient: State<'_, AmbientState>, task_id: i64, current_text: String) -> Result<DriftFlagDto, String>
pub fn trigger_speculative_subtask(state: State<'_, JeffState>, ambient: State<'_, AmbientState>, task_id: i64) -> Result<Option<SubTaskDto>, String>
pub fn dismiss_proactive_trigger(state: State<'_, JeffState>, task_id: i64, trigger_type: String) -> Result<(), String>
pub fn record_task_focus(state: State<'_, JeffState>, task_id: i64) -> Result<(), String>
```

All commands check quiet mode from `AmbientState` before firing LLM calls. If
`ambient_state.quiet_mode == true`, commands return suppressed results immediately.

Register all in `main.rs` `invoke_handler`.

### Frontend changes (M15.4)

In `App.tsx`:

**Focus detection:**
```typescript
useEffect(() => {
  // on window focus: record focus, then trigger re-orientation
  const handler = async () => {
    if (!activeTask) return;
    await recordTaskFocus(activeTask.id);
    const result = await triggerTaskResume(activeTask.id);
    if (result.summary) setReorientationBanner(result.summary);
    // phase 15 kickoff: fire-and-forget proactive stuck check
    void triggerSpeculativeSubtask(activeTask.id);
  };
  window.addEventListener("focus", handler);
  return () => window.removeEventListener("focus", handler);
}, [activeTask?.id]);
```

**Re-orientation banner:** New state `reorientationBanner: string | null`. Shown as a
dismissible banner at the top of the companion chat view (not a blocking modal). Auto-
dismisses after 8 seconds or on user interaction.

**Drift detection:** Called when the user submits a message (at the end of
`submitRoutedMessage`, post-send, on a delay so it doesn't block response). If
`is_drifting: true` and `confidence > 0.6`, show a soft inline notice in companion.

**Speculative subtask card:** When `triggerSpeculativeSubtask` returns a non-null result,
render an offer card in the companion view below the message list:
```
"I started a bounded background subtask: [title]"
[View] [Cancel] [Dismiss]
```
`Cancel` calls existing `cancelSubtask`. `Dismiss` hides the card + calls
`dismissProactiveTrigger`.

**Quiet mode button:** Add to companion view header (small icon toggle). Calls
`setQuietMode(...)` from `ambientClient.ts`. State still comes from `AmbientState`.

**New TypeScript types/wrappers in `tauriClient.ts`:**
- `ReorientationDto`, `DriftFlagDto`, `ProactiveTriggerDto` interfaces
- `triggerTaskResume(taskId)`, `checkTaskDrift(taskId, currentText)`,
  `triggerSpeculativeSubtask(taskId)`, `dismissProactiveTrigger(taskId, triggerType)`,
  `recordTaskFocus(taskId)`

### Check script (M15.5): `scripts/phase15_check.sh`

Checks:
1. `proactive.rs` module exists with `generate_reorientation`, `evaluate_drift`,
   `propose_speculative_subtask` symbols
2. `proactive_trigger_log` and `task_focus_log` tables in `store.rs`
3. All new store methods present
4. All new DTOs in `models.rs`
5. All new commands in `commands.rs` and registered in `main.rs`
6. Throttling: `proactive_trigger_log` query present in `proactive.rs`
7. Quiet mode guard present in commands (ambient quiet_mode check)
8. Frontend: `reorientationBanner`, `triggerTaskResume`, `checkTaskDrift` in `App.tsx`
9. Frontend: `triggerSpeculativeSubtask` in `App.tsx` + wrapper
10. Speculative subtask offer card: `speculative-subtask-card` test id in `App.tsx`
11. Frontend: speculative card supports `Cancel` and `Dismiss` actions
12. Drift notice in companion view: `drift-flag-notice` test id in `App.tsx`
13. `dismissProactiveTrigger` in `App.tsx`
14. Cooldown constants documented in `proactive.rs`
15. Regression gates: `scripts/phase13_check.sh` and `scripts/phase14_check.sh` still pass
16. Full build + test suite

### Phase 15 exit criteria (from PHASES.md)

- Returning to a paused task produces an unprompted re-orientation message within 3
  seconds of focus.
- Drift detection fires on a curated set of drift scenarios and does not fire on a
  curated set of on-track scenarios.
- Speculative subtasks never apply changes; results are always offered, never auto-merged.
- Quiet mode suppresses all proactive surfaces (audio, overlay, notifications) until
  disabled.
- `phase15_check.sh` verifies throttling, quiet-mode suppression, and the drift
  true/false-positive scenario suite.

---

## 6. Phase 16: Richer Parallel Work — detailed plan

### What this phase means for the product

Phase 16 completes felt property 4: *does parts of the work in parallel while you keep
going.* Today subtasks are single-shot LLM calls. Phase 16 makes them real parallel work
units: multi-step chains where each step is visible and individually cancellable, the
ability to write files (bounded, gated by explicit approval), and tool use inside the
subtask sandbox. The user can say "handle the intro while I work on citations" and Jeff
actually handles a multi-step write process in parallel without the user stopping.

### Existing foundation to leverage

- `SubTaskRunner.start_subtask` spawns a `thread::spawn`. The Phase 16 chain executor
  replaces `run_subtask_execution` with a multi-step variant.
- `SubTaskDto` already has `result_summary`, `result_payload`, `status`,
  `result_review_status` fields. The `status` lifecycle already has `running/completed/
  failed/cancelled`.
- `retrieval.rs` `retrieve_relevant_chunks` is the retrieval tool.
- `reasoning.rs` `generate_response` is the structured generation tool.
- `revision.rs` `propose_artifact_revision` is already a bounded write (proposes to
  revision table, requires explicit apply). Phase 16 extends this for file writes
  (new files, not just artifact revisions).
- `artifact_parser.rs` `supported_artifact_type` validates extensions.
- Workspace paths are bounded to `store.paths.workspace_root / task_slug / ...`.

### New DB tables (M16.1)

```sql
CREATE TABLE IF NOT EXISTS subtask_steps (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    subtask_id INTEGER NOT NULL REFERENCES subtasks(id),
    step_index INTEGER NOT NULL,
    step_type TEXT NOT NULL,        -- 'llm_call' | 'retrieval' | 'file_write_proposal'
    status TEXT NOT NULL DEFAULT 'pending',  -- pending | running | completed | failed | skipped
    description TEXT NOT NULL DEFAULT '',
    result_summary TEXT,
    result_payload TEXT,
    error_message TEXT,
    started_at TEXT,
    completed_at TEXT,
    UNIQUE(subtask_id, step_index)
);

CREATE TABLE IF NOT EXISTS subtask_file_write_proposals (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    subtask_id INTEGER NOT NULL REFERENCES subtasks(id),
    step_id INTEGER REFERENCES subtask_steps(id),
    task_id INTEGER NOT NULL REFERENCES tasks(id),
    proposed_path TEXT NOT NULL,    -- relative to task workspace
    proposed_content TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending_approval',  -- pending_approval | approved | rejected
    proposed_at TEXT NOT NULL DEFAULT (datetime('now')),
    resolved_at TEXT,
    FOREIGN KEY (subtask_id) REFERENCES subtasks(id)
);

CREATE TABLE IF NOT EXISTS subtask_write_audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id INTEGER NOT NULL REFERENCES tasks(id),
    subtask_id INTEGER NOT NULL,
    proposal_id INTEGER NOT NULL,
    action TEXT NOT NULL,           -- 'approved' | 'rejected'
    proposed_path TEXT NOT NULL,
    resolved_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

**Why these three tables:**
- `subtask_steps`: makes the chain execution visible (frontend can poll/display each step).
- `subtask_file_write_proposals`: the approval-gating mechanism. No write touches disk
  until this is `approved`. Phase 16 exit criterion: "no file write reaches disk without
  an explicit approval action."
- `subtask_write_audit_log`: immutable audit record. A rejected proposal leaves no disk
  artifact but still appears in the log.

**Modified `subtasks` table:**

Add two columns via `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`:
```sql
ALTER TABLE subtasks ADD COLUMN max_steps INTEGER NOT NULL DEFAULT 5;
ALTER TABLE subtasks ADD COLUMN current_step INTEGER NOT NULL DEFAULT 0;
```
Rollback is handled at execution-state/proposal-state level (see M16.2), so no workspace
snapshot column is required.

New store methods:
- `create_subtask_step(subtask_id, step_index, step_type, description)` → `SubTaskStepDto`
- `update_subtask_step_status(step_id, status, result_summary, result_payload, error)`
- `list_subtask_steps(subtask_id)` → `Vec<SubTaskStepDto>`
- `get_subtask_step(step_id)` → `Option<SubTaskStepDto>`
- `create_file_write_proposal(subtask_id, step_id, task_id, proposed_path, proposed_content)` → `FileWriteProposalDto`
- `resolve_file_write_proposal(proposal_id, action)` → `FileWriteProposalDto`
- `list_pending_file_write_proposals(task_id)` → `Vec<FileWriteProposalDto>`
- `list_file_write_proposals_for_subtask(subtask_id)` → `Vec<FileWriteProposalDto>`
- `append_write_audit_entry(task_id, subtask_id, proposal_id, action, path)`

New DTOs in `models.rs`:
```rust
pub struct SubTaskStepDto {
    pub id: i64,
    pub subtask_id: i64,
    pub step_index: i64,
    pub step_type: String,
    pub status: String,
    pub description: String,
    pub result_summary: Option<String>,
    pub result_payload: Option<String>,
    pub error_message: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

pub struct FileWriteProposalDto {
    pub id: i64,
    pub subtask_id: i64,
    pub step_id: Option<i64>,
    pub task_id: i64,
    pub proposed_path: String,
    pub proposed_content: String,
    pub status: String,
    pub proposed_at: String,
    pub resolved_at: Option<String>,
}

pub struct WriteAuditEntryDto {
    pub id: i64,
    pub task_id: i64,
    pub subtask_id: i64,
    pub proposal_id: i64,
    pub action: String,
    pub proposed_path: String,
    pub resolved_at: String,
}
```

### Multi-step chain execution in `subtask.rs` (M16.2)

The existing `run_subtask_execution` becomes the single-step fallback. The new function:

```rust
pub fn run_subtask_chain(
    store: &TaskStore,
    reasoning: &dyn ReasoningProvider,
    embeddings: &dyn EmbeddingProvider,
    subtask_id: i64,
    cancel_token: &AtomicBool,
) -> Result<()>
```

Chain planning phase (runs before steps):
1. Load subtask + context snapshot.
2. Call LLM with a "chain planning" prompt to produce a JSON step list:
   ```json
   {
     "steps": [
       { "step_type": "retrieval", "description": "..." },
       { "step_type": "llm_call", "description": "..." },
       { "step_type": "file_write_proposal", "description": "...", "proposed_path": "..." }
     ]
   }
   ```
   Max 5 steps (enforced both in prompt and in chain runner; `max_steps` field on subtask).
3. Store each step via `create_subtask_step`.
4. Persist subtask planning metadata only (step list + `max_steps` + `current_step`).

Step execution loop:
```
for step in steps:
    if cancel_token.load(Relaxed) { mark_cancelled_and_cleanup; return cancelled }
    update_subtask_step_status(step_id, "running")
    match step.step_type:
        "retrieval" → call retrieve_relevant_chunks, store result in step payload
        "llm_call"  → call generate_response with context + previous step payloads
        "file_write_proposal" → create_file_write_proposal; do NOT write to disk
    update_subtask_step_status(step_id, "completed", result)
    update subtask.current_step
```

Key invariant: `file_write_proposal` step NEVER writes to disk. It only inserts into
`subtask_file_write_proposals`. Writing to disk happens only in `approve_subtask_file_write`.

Cancellation mid-chain:
- If cancel_token fires while a step is running: mark step as `failed`, set error to
  `"cancelled_mid_step"`.
- Mark any remaining pending steps as `skipped`.
- Mark unresolved file proposals from this subtask as `rejected`.
- Transition subtask status to `cancelled`.
- Because file writes are deferred to explicit approval commands, there are no partial
  on-disk writes from in-flight chain execution to roll back.

**Tool use:**
- `retrieval` step: calls `retrieve_relevant_chunks` — existing retrieval pipeline.
- `llm_call` step: calls `reasoning.generate_response` — existing reasoning provider.
- `file_write_proposal` step: creates proposal record — new, bounded to workspace.
- No external browsing. No tool outside this set. Safety boundary is unchanged from
  existing phase 1-9 contract.

**Step limits:** Hard cap at 5 steps (`MAX_SUBTASK_STEPS: usize = 5` constant). If the
LLM proposes more, truncate to 5. Log a warning but do not error.

**Resource limits:** Each `llm_call` step gets the same context budget as a regular
chat message (no expansion). Total token estimate per chain: bounded by step count × 2k.

**Backward compatibility:** The existing `start_subtask` → `run_subtask_execution` path
stays intact and is used for simple single-step subtasks. Phase 16 adds a new
`start_subtask_chain` entry point that uses `run_subtask_chain`. The frontend chooses
which to call based on task complexity hint (simple = single-step, multi = chain).
For now, all new subtask creations from the companion UI use the chain path.

### File write approval in `commands.rs` (M16.3)

```rust
pub fn list_subtask_steps(state, subtask_id: i64) -> Result<Vec<SubTaskStepDto>, String>
pub fn list_file_write_proposals(state, task_id: i64) -> Result<Vec<FileWriteProposalDto>, String>
pub fn approve_subtask_file_write(state, proposal_id: i64) -> Result<FileWriteProposalDto, String>
pub fn reject_subtask_file_write(state, proposal_id: i64) -> Result<FileWriteProposalDto, String>
pub fn list_write_audit_log(state, task_id: i64, limit: Option<usize>) -> Result<Vec<WriteAuditEntryDto>, String>
pub fn start_subtask_chain(state, task_id: i64, title: String, description: String, execution_type: String, instruction_source: Option<String>) -> Result<SubTaskDto, String>
```

`approve_subtask_file_write` implementation:
1. Load proposal, verify status is `pending_approval`.
2. Verify parent subtask is `completed` before allowing approval (disallow apply while
   chain is still running).
3. Verify `proposed_path` is safe and within the task workspace:
   - must be relative (not absolute)
   - must not contain parent traversal (`..`)
   - canonical parent path must remain under canonical workspace root
4. Write content to disk: `std::fs::write(full_path, content)` — wrapped in
   `create_dir_all` for nested paths.
5. Update proposal status to `approved`.
6. Append `subtask_write_audit_log` entry.
7. Auto-ingest via `auto_ingest_file_for_task` (so the new file immediately becomes
   retrievable context).
8. Return updated `FileWriteProposalDto`.

`reject_subtask_file_write`:
1. Update proposal status to `rejected`.
2. Append `subtask_write_audit_log` entry.
3. Nothing written to disk.
4. Return updated `FileWriteProposalDto`.

### Frontend approval card (M16.4)

In `App.tsx`:

**State:**
```typescript
const [fileWriteProposals, setFileWriteProposals] = useState<FileWriteProposalDto[]>([]);
const [subtaskSteps, setSubtaskSteps] = useState<Record<number, SubTaskStepDto[]>>({});
```

**File write approval card:** Renders for each `pending_approval` proposal:
```
Jeff wants to write a file:
  Path: [proposed_path]
  [Preview first 200 chars of proposed_content]
  [Approve] [Reject]
```
Cards appear in the companion view below the recently-learned section. Each is
individually approvable/rejectable. On approve: calls `approveSubtaskFileWrite(proposalId)`,
re-fetches proposals. On reject: calls `rejectSubtaskFileWrite(proposalId)`.

**Step progress indicator:** Each subtask card in the companion view shows step progress:
`Step 2 / 4` when a chain is running. Rendered from `subtaskSteps[subtask.subtask_id]`.

**Chain subtask creation:** `createSubtaskChain(taskId, title, description, executionType, source)` 
calls `start_subtask_chain` command. The companion send-message flow uses this when the
routing intent is `subtask`.

**Polling for pending proposals:** `useEffect` with a 3-second interval on `activeTask?.id`
calls `listFileWriteProposals(taskId)` when there are running chain subtasks.

**New TypeScript types/wrappers in `tauriClient.ts`:**
- `SubTaskStepDto`, `FileWriteProposalDto`, `WriteAuditEntryDto` interfaces
- `listSubtaskSteps(subtaskId)`, `listFileWriteProposals(taskId)`,
  `approveSubtaskFileWrite(proposalId)`, `rejectSubtaskFileWrite(proposalId)`,
  `listWriteAuditLog(taskId, limit?)`, `startSubtaskChain(taskId, title, description, executionType, source?)`

### Check script (M16.5): `scripts/phase16_check.sh`

Checks:
1. New DB tables in `store.rs`: `subtask_steps`, `subtask_file_write_proposals`, `subtask_write_audit_log`
2. ALTER TABLE columns for `subtasks`: `max_steps`, `current_step`
3. New store methods present (all 9 listed above)
4. New DTOs in `models.rs`: `SubTaskStepDto`, `FileWriteProposalDto`, `WriteAuditEntryDto`
5. `MAX_SUBTASK_STEPS` constant in `subtask.rs`
6. `run_subtask_chain` function in `subtask.rs`
7. `file_write_proposal` step type does not call `fs::write` (the write is deferred)
8. `approve_subtask_file_write` in `commands.rs` enforces parent subtask completed + workspace path safety
9. `approve_subtask_file_write` calls `auto_ingest_file_for_task` after write
10. New commands registered in `main.rs`
11. Frontend: `file-write-approval-card` test id in `App.tsx`
12. Frontend: `subtask-step-progress` test id in `App.tsx`
13. Frontend: `approveSubtaskFileWrite`, `rejectSubtaskFileWrite` in `tauriClient.ts`
14. Full build + test suite
15. Cargo tests: `subtask_chain_*`, `store::tests::subtask_steps_*`, `store::tests::file_write_proposal_*`

### Phase 16 exit criteria (from PHASES.md)

- A multi-step subtask chain runs to completion with intermediate steps visible and
  individually cancellable.
- No file write reaches disk without an explicit approval action; ignore/dismiss leaves
  the filesystem unchanged.
- Cancellation mid-chain rolls back to the last clean checkpoint with no partial
  artifacts left behind.
- Tool use inside a subtask honors the same safety boundaries as the top-level runtime
  (no external browsing, single-task scope).
- `phase16_check.sh` verifies approval-gating of writes, rollback integrity, and
  step/resource limit enforcement.

For this implementation plan, "rollback" means execution-state rollback plus auto-rejecting
unresolved proposals; because writes are approval-gated, there should be no unapproved
filesystem deltas to revert.

---

## 7. Technical patterns to follow

### Adding a new Rust module

1. Create `desktop/src-tauri/src/new_module.rs`
2. Add `mod new_module;` to `main.rs` (alphabetically)
3. If integration tests need it: add `pub mod new_module;` to `lib.rs`
4. Use existing modules as imports: `use crate::store::TaskStore;`, `use crate::models::SomeDto;`

### Adding new DB tables to `store.rs`

Find `fn initialize_schema` → the `execute_batch` SQL block. Add new `CREATE TABLE IF
NOT EXISTS` and `CREATE UNIQUE INDEX IF NOT EXISTS` statements. Update the schema table-
count test in `#[cfg(test)]` at the bottom of the file:
```rust
// update the count to include the new tables
assert_eq!(table_count, N);  // find this assertion and increment N
```

Add ALTER TABLE migrations for existing tables at the end of `initialize_schema`,
wrapped in `conn.execute_batch(...)`. Use `ADD COLUMN IF NOT EXISTS` syntax for safety
(SQLite ≥ 3.37, bundled version supports it — but check; if not, use `try_execute`
or wrap in `PRAGMA table_info` check).

### Adding a new command

1. Write `#[tauri::command] pub fn my_command(state: State<'_, JeffState>, ...) -> Result<ReturnDto, String>` in `commands.rs`
2. Import `ReturnDto` in the `use crate::models::{ ... }` block at the top of commands.rs
3. Add `commands::my_command,` to the `invoke_handler` in `main.rs`
4. Add typed wrapper to `tauriClient.ts`:
   ```typescript
   export async function myCommand(param: Type): Promise<ReturnDto> {
     return invoke<ReturnDto>("my_command", { param });
   }
   ```

### Rust LLM call pattern (from `reasoning.rs`, `classifier.rs`)

```rust
let result = reasoning.generate_response(SYSTEM_PROMPT, &user_prompt_string)?;
// or for JSON-forced output (classifier pattern):
let client = Client::new();
let response = client
    .post("https://api.openai.com/v1/chat/completions")
    .bearer_auth(api_key)
    .json(&serde_json::json!({
        "model": "gpt-4o-mini",
        "temperature": 0,
        "response_format": { "type": "json_object" },
        "messages": [...]
    }))
    .send()?;
```

### Phase check script pattern (from `phase13_check.sh`, `phase14_check.sh`)

```bash
fail() { echo "FAIL: $1" >&2; exit 1; }
pass() { echo "PASS: $1"; }
grep -q "symbol_name" "$FILE" || fail "symbol_name missing from file"
pass "all X symbols present"
# at end:
cd "$ROOT_DIR/desktop"
npm run lint
npm run test
cargo build --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml module_name
# for m15/m16, also keep prior phases green:
"$ROOT_DIR/scripts/phase13_check.sh"
"$ROOT_DIR/scripts/phase14_check.sh"
```

### Test patterns (from `store.rs` tests)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    fn new_test_store() -> (TempDir, TaskStore, PathBuf) {
        let dir = TempDir::new().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        let ws = dir.path().to_path_buf();
        (dir, store, ws)
    }
    
    #[test]
    fn my_feature_round_trips() {
        let (_dir, store, _ws) = new_test_store();
        let task = store.create_task("test").unwrap();
        // ... test assertions
    }
}
```

---

## 8. Known gotchas and decisions

**SQLite ALTER TABLE:** SQLite does not support `ADD COLUMN IF NOT EXISTS` before
version 3.37.0. The bundled `rusqlite` version in this project uses SQLite 3.43+.
Still wrap migrations defensively:
```rust
let _ = conn.execute_batch("ALTER TABLE subtasks ADD COLUMN max_steps INTEGER NOT NULL DEFAULT 5");
```
If the column already exists, this errors but we ignore the error. Add a comment
explaining this is intentional for idempotent migration.

**Thread safety for chain executor:** `run_subtask_chain` runs in a `thread::spawn`.
`TaskStore` is `Clone` (wraps path structs only, no connection held). `Arc<dyn
ReasoningProvider>` is `Send + Sync`. `Arc<dyn EmbeddingProvider>` must also be
`Send + Sync` — verify in `embedding.rs`. The `AtomicBool` cancel token is already
`Arc<AtomicBool>` — this pattern is proven in the existing single-step executor.

**Rollback implementation:** Keep rollback minimal and deterministic:
1. Chain execution never writes to disk directly; it only produces `pending_approval`
   proposals.
2. On cancellation, mark remaining pending steps as `skipped`, mark in-flight step as
   failed/cancelled, and auto-reject unresolved proposals for that subtask.
3. `approve_subtask_file_write` is the only path that writes to disk and should be
   blocked unless the parent subtask is already `completed`.
4. If a proposal was explicitly approved and written, it is intentionally not rolled back
   (explicit user action).

**Path safety for file write proposals:** Enforce with explicit relative-path validation
plus canonical prefix checks:
```rust
let workspace_root = std::fs::canonicalize(store.get_task_workspace(task_id)?.workspace_path)?;
let proposed = PathBuf::from(&proposal.proposed_path);
if proposed.is_absolute() || proposed.components().any(|c| c == std::path::Component::ParentDir) {
    return Err("proposed path must be relative and must not contain '..'".to_string());
}
let full_path = workspace_root.join(&proposed);
let parent = full_path.parent().ok_or("invalid target path")?;
std::fs::create_dir_all(parent)?;
let canonical_parent = std::fs::canonicalize(parent)?;
if !canonical_parent.starts_with(&workspace_root) {
    return Err("proposed path escapes task workspace".to_string());
}
```

**Drift detection false-positive suppression:** The exit criterion requires drift to
NOT fire on on-track scenarios. Key guard: similarity score threshold. If
`retrieve_relevant_chunks` returns any chunk with `similarity_score > 0.6`, short-
circuit to `is_drifting: false` without calling the LLM at all. The LLM call is a
second-pass only for genuinely ambiguous cases. This keeps latency low and false-
positive rate low.

**Quiet mode and proactive triggers:** Phase 11 already added `quiet_mode: bool` to
`AmbientState` and `ambient_set_quiet_mode` command. `AmbientState` is separate managed
state (it is not a field on `JeffState`). Phase 15 commands must read this before firing
any LLM call. Pattern:
```rust
pub fn trigger_task_resume(
    state: State<'_, JeffState>,
    ambient: State<'_, AmbientState>,
    task_id: i64,
) -> Result<ReorientationDto, String> {
    let quiet = ambient.is_quiet_mode();
    if quiet {
        // return suppressed dto
    }
    // normal path...
}
```
Multiple `State<'_>` parameters are allowed in Tauri commands.

**Re-orientation within 3 seconds:** The LLM call latency for a short (25-word) response
on gpt-4o-mini is typically 300–600ms. The 3-second budget is comfortable. But the
command is synchronous (blocking reqwest client). If the user returns to a task rapidly
and the previous call is still in flight, the second call will queue. This is acceptable
for Phase 15 — no cancellation needed for re-orientation calls.

---

## 9. Suggested execution order

1. M15.1 — schema + store + DTOs for Phase 15
2. M15.2 — `proactive.rs` module
3. M15.3 — Phase 15 commands + `main.rs` registration
4. M15.4 — `tauriClient.ts` + `App.tsx` Phase 15 UI
5. M15.5 — `phase15_check.sh` + full verification
6. Update `docs/PHASES.md` and `docs/ARCHITECTURE.md`
7. M16.1 — schema + store + DTOs for Phase 16
8. M16.2 — multi-step chain executor in `subtask.rs`
9. M16.3 — file write commands + path safety
10. M16.4 — `tauriClient.ts` + `App.tsx` Phase 16 UI
11. M16.5 — `phase16_check.sh` + full verification
12. Update `docs/PHASES.md` and `docs/ARCHITECTURE.md`

Do not start M16.1 until `phase15_check.sh` passes completely.

---

## 10. Scope boundaries

**Phase 15 is NOT:**
- A general LLM observability system
- A per-keystroke monitoring system
- A replacement for the existing coworking/nudge system (extend it, don't replace)
- Proactive audio output without user presence detection

**Phase 16 is NOT:**
- An unrestricted shell executor or code runner
- External browsing or API calls from inside subtasks
- Auto-applying file writes (approval gating is absolute)
- Unlimited step chains (cap at 5)
- A replacement for the existing single-step subtask path (keep it working)

---

## 11. ARCHITECTURE.md section to add for Phase 15

```
13. Proactive initiation layer (desktop/src-tauri/src/proactive.rs)
- three trigger evaluators: generate_reorientation, evaluate_drift, propose_speculative_subtask
- proactive_trigger_log table: per-trigger-type per-task cooldown enforcement
- task_focus_log table: genuine-return detection (> 5 minutes since last focus)
- all triggers check AmbientState.quiet_mode before firing LLM calls
- re-orientation: gpt-4o-mini with 25-word output budget; fires on task resume
- drift evaluation: similarity short-circuit (> 0.6 = on-track); LLM second-pass for ambiguous cases
- speculative subtask: reuses suggest_subtask_for_task + create_subtask_and_start; starts in background with `instruction_source=system`, surfaced as companion card
- frontend: window focus event → recordTaskFocus + triggerTaskResume; dismissible banner (8s auto-dismiss)
- quiet mode button in companion header → `setQuietMode(...)` (ambient client)
```

---

## 12. ARCHITECTURE.md section to add for Phase 16

```
14. Richer parallel work (desktop/src-tauri/src/subtask.rs extended)
- subtask_steps table: per-step status tracking (pending/running/completed/failed/skipped)
- subtask_file_write_proposals table: approval-gated write queue (pending_approval/approved/rejected)
- subtask_write_audit_log table: immutable record of every write decision
- run_subtask_chain: chain planning LLM call → step list (max 5) → sequential step execution
- step types: retrieval (retrieve_relevant_chunks), llm_call (reasoning provider), file_write_proposal
- file_write_proposal step never writes to disk; write deferred to approve_subtask_file_write command
- approve_subtask_file_write: allowed only after parent subtask completion; workspace path safety check → fs::write → auto_ingest_file_for_task → audit log
- rollback on cancel: pending_approval proposals auto-rejected; approved writes are not reversed (user approved them)
- frontend: file-write-approval-card per pending proposal; step progress indicator on subtask cards; 3s polling for proposals
```
