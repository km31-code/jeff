# Jeff Architecture (Phase 14)

## Goal

Phase 14 replaces the TypeScript keyword-based intent router with a
model-based classifier. Routing decisions are now semantic, not lexical,
and carry structured slots (target_description, instruction, draft_type,
topic) downstream to revision and subtask handlers. The keyword router
remains as a silent fallback when the classifier is unavailable or times out.

Key constraints: no new backend capability. The classifier is a pure
call to gpt-4o-mini with response_format json_object. Frontend fallback
fires within 120ms and logs to console for observability. All Phase 1-13
surfaces remain unmodified.

## Layering

1. Backend/runtime systems (unchanged capability set)
- retrieval + context pack
- chat + reasoning + grounding
- voice STT/TTS + interruption
- coworking runtime
- revision workflow
- bounded subtask workflow
- suggestion/flow engine
- action center/debug data sources

2. Streaming pipeline (`desktop/src-tauri/src/streaming.rs`, `chat_streaming.rs`)
- `InteractionToken` + `InteractionRegistry`: per-turn cancellation tokens,
  child tokens propagate cancel to all subtasks of a turn
- `send_message_streaming` tauri command: async, returns turn_id immediately,
  spawns `run_llm_stream` which streams LLM SSE and emits `stream://llm_token`
  events for each delta
- phrase-chunked TTS: `phrase_needs_synthesis` detects sentence boundaries;
  `spawn_tts_chunk` runs `synthesize_phrase_async` concurrently per phrase and
  emits `stream://tts_chunk` with base64 mp3 audio ordered by `phrase_id`
- `cancel_streaming_turn` tauri command: cancels any in-flight turn by turn_id;
  propagates to all concurrent TTS synthesis tasks via child tokens
- events: `stream://llm_token`, `stream://llm_complete`, `stream://tts_chunk`,
  `stream://turn_cancelled`, `stream://turn_complete`

3. Ambient presence layer (`desktop/src-tauri/src/ambient.rs`)
- system tray icon with status (idle / listening / working) and menu
- single-instance lock (second launch focuses existing overlay)
- global hotkey: CmdOrCtrl+Shift+J toggles overlay and focuses it for input
- overlay window: frameless, always-on-top, passive display does not steal focus
- collapsed state (compact bar, 72px height)
- expanded state (companion chat surface, 520px height)
- native OS notification dispatch with quiet mode suppression
- ambient state: `AmbientState` (Mutex-guarded, independent of JeffState)
- ambient IPC commands: `ambient_*` family (not in phase 1-10 contract)
- events emitted: `ambient://state-changed`, `ambient://overlay-shown`,
  `ambient://overlay-hidden`, `ambient://notification-click`,
  `ambient://hotkey-conflict`, `ambient://notification-dispatched`,
  `ambient://notification-suppressed`

4. Frontend orchestration layer (`desktop/src/App.tsx`)
- **Companion Mode (default)**:
  - context header
  - chat/voice primary interaction
  - inline action cards
- **Full Workspace (on-demand)**:
  - existing panel-heavy surfaces (artifacts, revisions, subtasks, suggestions, debug)

5. Overlay surface (`desktop/src/Overlay.tsx`)
- served from the same frontend bundle as App.tsx
- `main.tsx` branches on `isOverlayWindow()` (detects `#overlay` hash)
- collapsed: status dot, task label, hotkey hint, dismiss control
- expanded: recent messages (last 6), send input, open-workspace link,
  notification context banner, hotkey conflict warning
- subscribes to `ambient://` events via `@tauri-apps/api/event`
- uses `send_message_streaming` + `stream://` listeners when streaming is enabled
  (same pipeline as companion UI)

6. Ambient client (`desktop/src/ambientClient.ts`)
- thin invoke wrappers for all `ambient_*` commands
- `isOverlayWindow()` detection utility
- types: `AmbientStateDto`, `NotificationPayload`, `TrayStatus`,
  `OverlayMode`

7. Stream client (`desktop/src/streamClient.ts`)
- typed wrappers: `onLlmToken`, `onLlmComplete`, `onTtsChunk`, `onTurnCancelled`,
  `onTurnComplete`
- `isStreamingEnabled()`: checks VITE_JEFF_STREAMING env flag (true by default)

8. Streaming TTS audio queue (frontend, `App.tsx`)
- `ttsActiveTurnIdRef`: gates tts_chunk events; persists after turn_complete so
  late-arriving chunks still play
- `streamTtsQueueRef`: Map<phrase_id, HTMLAudioElement> for ordered playback
- `scheduleStreamTtsPlayback`: plays next phrase when current ends or on arrival
- `stopStreamingTtsPlayback`: immediate cut on barge-in; drains queue and
  revokes object URLs

9. Partial STT (frontend, `App.tsx`)
- `tryStartPartialStt`: starts Web Speech API with interimResults=true;
  on confidence >= 0.7 routes message early and stops recorder
- `stopPartialStt`: cleans up recognition on barge-in or stop
- `partialSttSentRef`: guard prevents Whisper double-submission
- falls back to Whisper silently when Web Speech API is unavailable

10. Conversation intent routing (frontend)
- answer / revision / subtask / suggestion / unknown
- unknown routes to an explicit clarify prompt (no silent coercion to answer)
- calls existing command paths only

## Window Graph

```
process (single instance)
├── main window   [label: "main",    hidden by default, full workspace]
└── overlay window [label: "overlay", always-on-top, frameless, hidden by default]
```

Both windows close-to-hide. Quit is only reachable via the tray menu
("Quit Jeff") or `ambient_quit_app` command.

## Focus Model

Passive overlay display calls `window.show()` only. `set_focus()` is never
called from the `show_overlay` path, so background context events such as
selection capture do not interrupt the user's active app.

Explicit user summons use the interactive path: hotkey, tray show, onboarding,
notification clicks, and second-launch handoff call `show_overlay_interactive`.
That path focuses the overlay and emits `ambient://overlay-shown` with
`interactive: true`, allowing the frontend to focus the primary onboarding
control or message input. Hotkey dismiss hides the overlay.

## Notification Path

Completion events and proactive nudges call `ambient::dispatch_notification`
when Jeff is backgrounded (overlay and workspace both hidden).
If quiet mode is active the notification is suppressed and an
`ambient://notification-suppressed` event is emitted instead. Clicking a
native notification calls `ambient_notification_clicked` which expands the
overlay and emits `ambient://notification-click` with optional context
(kind + id) so the overlay can focus the relevant surface.

## Hidden vs Removed

- Hidden by default:
  - main workspace window (shown via tray or "Open Full Workspace")
  - action center panel
  - suggestion panel
  - revision/subtask/artifact work panels
  - runtime inspector/debug panels
- Removed:
  - nothing in backend capability.
  - no core systems deleted.

## Escape Hatch

`Open Full Workspace` (tray menu item or overlay button) shows and focuses
the full workspace window.

12. Intent classification layer (`desktop/src-tauri/src/classifier.rs`)
- `IntentLabel` enum: `Answer`, `Revision`, `Subtask`, `Suggestion`, `Unknown` (serde lowercase)
- `IntentSlotsDto`: `target_description`, `instruction`, `draft_type`, `topic` (all `Option<String>`)
- `IntentClassificationDto`: `intent: IntentLabel`, `confidence: f32`, `slots: IntentSlotsDto`
- `classify_intent(text, api_key) -> Result<IntentClassificationDto>`: calls gpt-4o-mini with
  `response_format: { type: "json_object" }`; parses via `parse_classification` which maps
  unknown label strings to `IntentLabel::Unknown` and defaults missing fields gracefully.
  Request timeout is bounded to protect response-start latency.
- Tauri command `classify_message_intent(task_id, message_text)`: reads `OPENAI_API_KEY` from env
- Library target (`src/lib.rs`) exposes `classifier` and `models` modules for integration tests
- `tests/intent_eval.rs`: live eval harness gated on `OPENAI_API_KEY`; 40-example labeled set at
  `tests/fixtures/intent_eval_set.json`; asserts >= 90% intent accuracy, checks slot accuracy,
  and prints p50/p95 latency
- Frontend (`App.tsx`): `classifyMessageIntentWithFallback` wraps classifier with an explicit
  timeout budget; on timeout or error logs `[jeff] intent_classifier_fallback: <reason>`
  and falls back to `inferMessageIntentKeyword`; slots passed to
  `autoCreateRevisionFromIntent` (uses `slots.instruction` and `slots.target_description`)
  and `autoCreateSubtaskFromIntent` (uses `slots.draft_type` via
  `inferSubtaskExecutionTypeFromDraftType`)

11. Workspace awareness layer (`desktop/src-tauri/src/watcher.rs`)
- watcher lifecycle is synchronized to the active task (startup restore + task switch)
- `WatcherState`: in-memory maps for active file watchers and clipboard poll handles
- `start_watcher`: spawns a `notify::RecommendedWatcher` in-process; events forwarded
  through a `tokio::sync::mpsc` channel to a debounce task (500 ms window, 200 ms poll)
- create/modify/remove/rename events are all handled; deleted files are removed from
  retrieval influence by clearing associated chunks and registry entries
- `auto_ingest_file_for_task` (in `retrieval.rs`): idempotent re-ingest keyed on
  `(task_id, canonical_path)` + high-resolution file version tag; reuses
  `replace_artifact_chunks` on updates
- ignore rules: hidden files, hidden dirs (relative to watch root), `artifacts/`,
  `node_modules/`, `.git/`, files >2 MB, unsupported extensions
- clipboard poll: `arboard` crate, 2-second interval, SHA-based dedup, off by default
- four new DB tables: `watched_folders`, `watched_file_registry`,
  `clipboard_capture_settings`, `recently_learned_log`
- new Tauri commands: `start_workspace_watcher`, `stop_workspace_watcher`,
  `get_watcher_status`, `list_recently_learned`, `set_clipboard_capture`,
  `get_clipboard_capture_setting`
- companion view surface: collapsible "recently learned" section with clipboard toggle

13. Proactive initiation layer (`desktop/src-tauri/src/proactive.rs`)
- three trigger evaluators: `generate_reorientation`, `evaluate_drift`, `propose_speculative_subtask`
- `proactive_trigger_log` table: per-trigger-type per-task cooldown enforcement
- `task_focus_log` table: genuine-return detection (> 5 minutes since last focus)
- all triggers check `AmbientState.is_quiet_mode()` before firing LLM calls
- re-orientation: gpt-4o-mini with 25-word output budget; fires on task resume
- drift evaluation: similarity short-circuit (> 0.6 = on-track); LLM second-pass for ambiguous cases
- speculative subtask: reuses `suggest_subtask_for_task` + `create_subtask_and_start`; starts in background with `instruction_source=system`, surfaced as companion card
- frontend: window focus event → `recordTaskFocus` + `triggerTaskResume`; dismissible banner (8s auto-dismiss)
- drift notice fires after message send on 1s delay; shown inline in companion view
- quiet mode toggle in companion header → `setQuietMode(...)` (ambient client)
- five new tauri commands: `trigger_task_resume`, `check_task_drift`, `trigger_speculative_subtask`, `dismiss_proactive_trigger`, `record_task_focus`

14. Richer parallel work layer (`desktop/src-tauri/src/subtask.rs`, `store.rs`, `commands.rs`)
- three new DB tables: `subtask_steps`, `subtask_file_write_proposals`, `subtask_write_audit_log`
- two new columns on `subtasks`: `max_steps INTEGER DEFAULT 5`, `current_step INTEGER DEFAULT 0`; added via idempotent ALTER TABLE migration
- `SubTaskRunner.start_subtask_chain`: spawns `run_subtask_chain` thread; registered in same cancel-flag map as single-step subtasks
- `run_subtask_chain`: chain planning phase (one LLM call → `ChainPlan` JSON → filter unknown step types → truncate to `MAX_SUBTASK_STEPS=5` → store all steps as pending); step execution loop (cancel check at each iteration, dispatch `retrieval | llm_call | file_write_proposal`)
- `file_write_proposal` step type: generates content via LLM, creates DB record with `status=pending_approval` — NEVER writes to disk
- cancel mid-chain: remaining steps marked `skipped`, pending proposals auto-rejected via `auto_reject_pending_proposals`
- `approve_subtask_file_write` command: validates relative path (no absolute, no `..`), joins with task workspace_path, writes file, records audit entry; `reject_subtask_file_write` skips the write and records rejection
- path safety: all components checked via `Component::Normal` enumeration; any non-normal component is rejected
- six new tauri commands: `list_subtask_steps`, `list_file_write_proposals`, `approve_subtask_file_write`, `reject_subtask_file_write`, `list_write_audit_log`, `start_subtask_chain`
- intent-routing path updated: `autoCreateSubtaskFromIntent` now calls `startSubtaskChain` (multi-step planning) instead of single-step `createSubtask`
- UI additions: file-write-approval cards in companion view (Approve/Reject); subtask step progress list under running subtasks in workspace view; write audit log section; 3s polling for pending proposals; 1.5s step-status polling for running subtasks
- SQLite concurrency fix: `connect()` now uses `PRAGMA busy_timeout = 10000` (via `execute_batch`) and `BEGIN IMMEDIATE` transactions for write operations; eliminates SQLITE_BUSY flakiness under concurrent subtask + chat writes

15. User model layer (`desktop/src-tauri/src/user_model.rs`)
- `user_profile` table: key-value pairs with `updated_at`; incremental update after
  each session; never transmitted off-device
- signals captured: writing style (`style_avg_sentence_length`, `style_formality_score`
  derived from accepted revision text via contraction ratio), delegation patterns
  (`delegation_accepted_<type>`, `delegation_rejected_<type>`), work rhythm
  (`work_rhythm_focus_hours` ring buffer → `work_rhythm_peak_hour` mode),
  response length preference (exponential moving average), quality rubrics
  (`rubric_N` keys stored verbatim), trigger weights (`trigger_weight_<type>`)
- `build_profile_injection(store)`: returns compact (< 100 token) block prepended to
  chat, revision, and reorientation system prompts; returns `None` when table is empty
- signal writers: `record_revision_accepted`, `record_revision_rewrite` (LCS word-diff
  ratio detects significant rewrites), `record_subtask_accepted/rejected`,
  `record_focus_hour`, `record_response_length`, `record_trigger_dismissed`,
  `add_quality_rubric`
- `get_readable_signals`: produces `Vec<SignalSummary>` with plain-language labels for
  the "Jeff remembers" panel; each signal has a delete button, clear-all wipes table
- tauri commands: `get_user_profile_signals`, `delete_profile_signal`,
  `clear_user_profile`, `add_quality_rubric`, `get_profile_injection_preview`

16. Workload awareness layer (`desktop/src-tauri/src/workload.rs`)
- `compute_workload_summary(store)`: returns `WorkloadSummaryDto` with `active_tasks`
  (focused within 14 days, sorted most-recent first) and `stale_tasks` (> 14 days,
  sorted least-recent first); caller caches for 5 minutes
- `WorkloadTaskDto`: id, title, last_focused_at, days_since_focus, pending_item_count,
  is_active; pending count = pending file write proposals + running subtasks +
  unreviewed speculative results
- `check_stale_task_notifications`: fires one native OS notification per stale task
  per 24 hours when unreviewed speculative results exist; respects quiet mode;
  throttled via `stale_notify_<task_id>` app_setting timestamp
- cross-task collision check: before starting a new subtask, cosine similarity >
  0.8 against recent completed subtasks across all tasks triggers a soft companion
  notice ("similar work exists in [task]"); never auto-merges
- tauri commands: `get_workload_summary`, `switch_active_task`

17. Calendar context layer (`desktop/src-tauri/src/calendar.rs`)
- macOS EventKit integration via direct Objective-C runtime bridge
  (`#[cfg(target_os = "macos")]`); non-macOS builds compile to no-ops
- `fetch_next_event(hours)`: queries EKEventStore for events starting within the next
  `hours` hours (production default: 8); returns the soonest event as
  `CalendarEventDto { title, starts_at, minutes_until }`
- permission flow: `request_calendar_permission()` calls
  `requestAccessToEntityType:completion:` asynchronously (OS shows native dialog);
  `get_calendar_permission_status()` maps EKAuthorizationStatus to
  "granted" | "denied" | "not_determined"
- `CalendarState` mutex in AppState: caches current `Option<CalendarEventDto>`;
  background poll task in main.rs updates it every 60 seconds when
  `privacy_calendar_context_enabled` is true and permission is granted
- companion header emits `calendar://next_event` on state change; frontend shows
  "Meeting in N min — [title]" badge; disappears when no event is within 8 hours
- reorientation prompt includes the next event when within 2 hours
- nothing calendar-related is persisted to SQLite; all data is transient in memory
- tauri commands: `request_calendar_permission`, `get_calendar_permission_status`,
  `get_calendar_next_event`

18. Live app actions layer (`desktop/src-tauri/src/selection_capture.rs`, extended)
- HTTP bridge server (port 47832) extended with live-edit routes:
  - `POST /apply-edit`: receives `{ token, anchor_hash, anchor_context, patch_text }`;
    validates bridge token; creates `live_edit_receipts` DB record with
    `status=pending_approval`; emits `live_action://apply_requested` event to frontend
  - `GET /pending-approval/<token>`: polled by browser extension; returns `{ approved,
    patch_text }` once `approve_live_edit` command fires, or `{ rejected }` on reject
  - `POST /apply-result`: extension reports `{ token, status: "applied" | "failed" }`;
    updates receipt record; emits `live_action://result` event to frontend
  - `POST /apply-fallback`: extension calls when anchor hash or context check fails;
    creates receipt with `status=guided_apply`; emits `live_action://fallback_triggered`
- anchor validation in content.js: selection hash + surrounding 50-char context check
  before writing; mismatches route to `/apply-fallback` with reason
- receipt log (`live_edit_receipts` table): app_name, document_title, anchor_hash,
  before_excerpt, after_excerpt, status, created_at, resolved_at
- tauri commands: `approve_live_edit`, `reject_live_edit`, `list_live_edit_receipts`,
  `get_pending_live_edits`
- browser extension (background.js): polls `/pending-approval` every 3 seconds after
  receiving a `live_action://apply_requested` message from the native app; dispatches
  approved patch to content.js; reports result back via `/apply-result`
- frontend: live edit proposal card shows before/after excerpt, Approve/Reject buttons;
  guided-apply fallback card shows patch text + instruction copy when anchor drifts

19. Distribution and auto-update (`tauri.conf.json`, `main.rs`, `.github/workflows/release.yml`)
- universal binary: `targets: ["app", "dmg"]` with `universal-apple-darwin` target
  (arm64 + x86_64 lipo'd together); minimum macOS 13.0 (Ventura)
- code signing: `signingIdentity` and `providerShortName` in tauri.conf.json are
  injected from GitHub Actions secrets at CI build time (null in source for local dev)
- notarization: `xcrun notarytool submit` in CI `notarize` job; staple ticket applied
  before .dmg upload; Gatekeeper passes without user override on signed builds
- auto-update: `tauri-plugin-updater` initialized in main.rs; background task on
  startup calls `app.updater()?.check()`; if update available, native dialog:
  "Jeff update available" with Install / Later buttons; `download_and_install()` +
  `app.restart()` on confirm
- update feed: `https://github.com/km31-code/jeff/releases/latest/download/latest.json`
  hosting `{ version, pub_date, platforms.darwin-universal: { signature, url } }`;
  `pubkey` in tauri.conf.json is `{{TAURI_PUBLIC_KEY}}` placeholder — injected by CI
- CI pipeline (`.github/workflows/release.yml`): 5 sequential jobs triggered on push
  to `release` branch: test (cargo test + phase17_check.sh + npm test) → build
  (unsigned universal binary) → sign (codesign with hardened runtime +
  entitlements.plist) → notarize (notarytool + stapler) → release (create GitHub
  Release with .dmg, signed updater archive, latest.json)

## Safety Boundaries (unchanged)

- no silent file application
- no unrestricted autonomy
- no external browsing/tooling
- single active-task scope

## Windows-ready when

- tauri-plugin-updater endpoint has a `win32-x86_64` platform entry in `latest.json`
- CI pipeline adds a `windows-2022` runner job alongside the existing `macos-14` job
- SMAppService (Phase 19 login-item registration) replaced with Windows Task Scheduler
  equivalent via `schtasks` or the `windows-service` crate
- EventKit (Phase 23 calendar context) replaced with Windows Calendar API via the
  Windows Runtime (`windows` crate) under a `#[cfg(target_os = "windows")]` gate
- Global shortcut and AXUIElement accessibility paths have Windows implementations:
  - `tauri-plugin-global-shortcut` already cross-platform; no change needed
  - `context_observer.rs` AXUIElement calls replaced with `IAccessible` / `UIAutomation`
    under `#[cfg(target_os = "windows")]`
- `entitlements.plist` is macOS-only; Windows code-signing uses `signtool.exe` with
  a separate EV certificate in the CI `sign` job
