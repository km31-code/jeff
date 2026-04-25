# Phases 23 and 24 — Implementation Plan

Status key: `[ ]` pending · `[~]` in progress · `[x]` done

This file is the live tracking document for Phases 23 and 24.
Update status fields inline as each milestone completes.
If handing off to Codex mid-phase, include the milestone number and the
last completed sub-item so it can resume from the right point.

---

## Phase 23: Live App Actions + Personalization + Workload Awareness

Four tracks executed in this order:
1. M23.1–M23.2: Personalization (foundation first, then signal collection)
2. M23.3: Workload awareness
3. M23.4: Calendar context
4. M23.5: Live app actions
5. M23.6: phase23_check.sh

### M23.1 — User Profile DB + Injection Foundation

**Status: [ ]**

Goal: create the user_profile table and the inject-on-every-prompt path.
No signal collection yet — just the plumbing that signals will write into.

Files to create:
- `desktop/src-tauri/src/user_model.rs` — new module

Files to modify:
- `desktop/src-tauri/src/store.rs` — add user_profile table migration, get/set/clear helpers
- `desktop/src-tauri/src/chat.rs` — inject profile summary into system prompt (gated)
- `desktop/src-tauri/src/proactive.rs` — inject profile summary into reorientation/revision prompts
- `desktop/src-tauri/src/commands.rs` — add get_user_profile_signals, clear_user_profile commands
- `desktop/src-tauri/src/main.rs` — register new commands
- `desktop/src/tauriClient.ts` — typed wrappers for new commands

DB schema (idempotent migration in store.rs):
```sql
CREATE TABLE IF NOT EXISTS user_profile (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

user_model.rs responsibilities:
- `get_profile_value(conn, key) -> Option<String>`
- `set_profile_value(conn, key, value)` — upsert
- `clear_all_profile(conn)`
- `build_profile_injection(conn) -> Option<String>` — returns a compact (< 100 token)
  summary string if at least one signal exists; returns None if table is empty.
  The string is prepended to system prompts as a brief context block.

Injection gate: skip if `privacy_user_profile_memory_enabled` is false.
The setting key already exists in store.rs from Phase 21.

New Tauri commands:
- `get_user_profile_signals` — returns Vec<{ key, value, updated_at }>
- `clear_user_profile` — deletes all rows from user_profile table

Verification:
- user_profile table created in DB
- set then get round-trips a signal
- clear removes all rows
- build_profile_injection returns None when table is empty
- injection string appears in system prompt when signals exist

---

### M23.2 — Personalization Signal Collection + "Jeff Remembers" UI

**Status: [ ]**

Goal: populate the user_profile table from real interaction events and
expose those signals in the companion view.

Signals to collect (all writes go through user_model::set_profile_value):

1. **Writing style** — when a revision is accepted, parse the accepted text:
   compute average sentence length (word count / sentence count) and a simple
   formality score (ratio of formal indicators: no contractions, complete sentences).
   Keys: `style_avg_sentence_length`, `style_formality_score`.
   Triggered in: commands.rs accept_revision path (or revision.rs).

2. **Delegation patterns** — when a subtask is created and later accepted or
   rejected, increment counters per execution_type.
   Keys: `delegation_accepted_<execution_type>`, `delegation_rejected_<execution_type>`.
   Triggered in: commands.rs subtask accept/reject path.

3. **Work rhythm** — when task_focus_log gains a new entry, record the hour of day.
   Key: `work_rhythm_peak_hour` — the most frequent hour across all focus events.
   Triggered in: commands.rs record_task_focus path.

4. **Response length preference** — after each non-dismissed Jeff response, record
   word count. Running average stored as `response_length_preference`.
   Triggered in: message send completion in commands.rs or chat.rs.

5. **Quality rubrics** — plain text notes entered by the user in companion settings.
   Stored as `rubric_<n>` where n is an incrementing counter. Injected verbatim
   into revision and drafting prompts.
   New Tauri commands: `add_quality_rubric(text)`, `delete_quality_rubric(key)`.

6. **Feedback signal — rewrite delta** — if a user edits a Jeff-generated draft
   in the revision workflow and accepts, compute whether the word-level diff ratio
   is > 30% (significant rewrite). If so, updates style signals toward shorter
   sentences and lower formality score.
   Triggered in: revision accept path when user-edited text differs from original.

7. **Feedback signal — dismissed proactive triggers** — when a proactive trigger
   is dismissed, decrement a trigger-type weight.
   Key: `trigger_weight_<trigger_type>` (values: float, default 1.0, floor 0.1).
   Triggered in: commands.rs dismiss_proactive_trigger path.

"Jeff remembers" panel (companion view):
- Shows 2–3 active signals in plain language (e.g. "You prefer shorter responses",
  "You tend to focus work in the morning").
- Each signal has a delete button that calls delete_user_profile_signal(key).
- "Clear all" button calls clear_user_profile and empties the panel immediately.
- Panel is hidden if no signals exist yet.
- Panel is hidden if privacy_user_profile_memory_enabled is false.

Files to modify:
- `desktop/src-tauri/src/user_model.rs` — add signal-write helpers per signal type
- `desktop/src-tauri/src/revision.rs` — call signal updates on accept
- `desktop/src-tauri/src/subtask.rs` — call signal updates on subtask accept/reject
- `desktop/src-tauri/src/proactive.rs` — call signal updates on trigger dismiss
- `desktop/src-tauri/src/commands.rs` — add add_quality_rubric, delete_quality_rubric,
  delete_user_profile_signal commands; inject rubrics into revision prompts
- `desktop/src-tauri/src/main.rs` — register new commands
- `desktop/src/App.tsx` — "Jeff remembers" panel render
- `desktop/src/App.test.tsx` — panel render test
- `desktop/src/tauriClient.ts` — typed wrappers

Verification:
- After 5 accepted revisions, profile contains style signal keys
- After saving a rubric, revision system prompt contains it verbatim
- Panel renders with at least one signal after 5 interactions
- Per-signal delete removes only that key
- Clear all wipes user_profile table; panel empties immediately

---

### M23.3 — Workload Awareness

**Status: [ ]**

Goal: show the user all active tasks in one view, support task switching,
and notify about stale tasks with unreviewed work.

Files to create:
- `desktop/src-tauri/src/workload.rs` — new module

Files to modify:
- `desktop/src-tauri/src/commands.rs` — add workload commands
- `desktop/src-tauri/src/main.rs` — register commands
- `desktop/src-tauri/src/store.rs` — add stale_notification_sent_at helper
  (uses app_settings with key `stale_notify_<task_id>`)
- `desktop/src/App.tsx` — "Your workload" section
- `desktop/src/App.test.tsx` — workload section tests
- `desktop/src/tauriClient.ts` — typed wrappers

workload.rs responsibilities:
- `WorkloadSummary`: active tasks (any task_focus_log entry in last 14 days),
  stale tasks (last focus > 14 days ago), per-task pending item count
  (pending file write proposals + running subtasks + unreviewed speculative subtask results).
- `compute_workload_summary(conn) -> WorkloadSummary` — queries task_focus_log,
  subtask_file_write_proposals, subtasks table.
- 5-minute in-memory cache: `WorkloadCache { summary: Option<WorkloadSummary>, computed_at: Instant }`.
  Cache stored in JeffState or a new Mutex in AppState. Invalidated on task switch or
  any subtask/proposal status change.

New Tauri commands:
- `get_workload_summary` — returns WorkloadSummaryDto (active, stale, pending counts)
- `switch_active_task_from_companion(task_id)` — calls set_active_task + stops current
  workspace watcher + starts watcher on new task's workspace_path if set.
  Returns the new active TaskDto.

"Your workload" section (companion view):
- Collapsible, shown below the chat history.
- Active tasks: task title, last-focused relative time, pending item count.
- Clicking a task calls switch_active_task_from_companion.
- Stale tasks shown separately with "last worked on X days ago."

Stale-task notification:
- On app startup and on task_focus events, check all tasks for stale + unreviewed
  subtask results.
- For each qualifying task: if `stale_notify_<task_id>` setting is absent or was
  set > 24h ago, fire one native OS notification. Update the setting timestamp.
- Suppressed if quiet mode is active.
- Implemented in workload.rs `check_stale_task_notifications(conn, app_handle)`.
  Called from commands.rs at startup and from record_task_focus.

Cross-task collision detection:
- In subtask.rs `run_subtask_chain`, before the chain planning call, query the
  embeddings for recently-completed subtasks across all tasks (last 30 days).
- Compute cosine similarity between new subtask instruction embedding and each
  historical embedding using similarity.rs.
- If any result > 0.8: emit one soft companion notice event
  `subtask://collision-detected` with { matching_task_title, similarity_score }.
  No auto-merge, no block. One notice only.
- Frontend: ephemeral banner in companion view. Dismiss clears it.

Verification:
- Workload section shows all tasks focused in last 14 days with correct pending counts.
- Switching tasks from companion restarts workspace watcher on new task folder.
- Stale-task notification fires at most once per task per 24h.
- Stale-task notification suppressed when quiet mode is on.
- Collision notice fires when cosine similarity > 0.8; does not fire below threshold.

---

### M23.4 — Calendar Context (EventKit)

**Status: [ ]**

Goal: surface upcoming calendar events as ambient context in the companion
header and re-orientation prompts.

Implementation approach: EventKit via Objective-C bridge using the `objc` crate.
This is macOS-only; the code is conditionally compiled with `#[cfg(target_os = "macos")]`.

Files to create:
- `desktop/src-tauri/src/calendar.rs` — new module

Files to modify:
- `desktop/src-tauri/src/state.rs` — add CalendarState mutex to AppState
  (or store in AmbientState). Fields: `next_event: Option<CalendarEventDto>`,
  `last_polled: Option<Instant>`.
- `desktop/src-tauri/src/commands.rs` — add calendar commands
- `desktop/src-tauri/src/main.rs` — register commands, start calendar poll task
- `desktop/src-tauri/src/proactive.rs` — include next_event in re-orientation
  system prompt when event is within 2 hours
- `desktop/src/App.tsx` — companion header event display
- `desktop/src/tauriClient.ts` — typed wrappers

calendar.rs responsibilities:
- `request_calendar_permission() -> bool` — calls EKEventStore requestAccessToEntityType.
  Plain-language explanation shown before the OS prompt.
- `get_next_event(hours: u8) -> Option<CalendarEventDto>` — reads events from the
  next N hours from the default calendar. Returns the soonest.
- `CalendarEventDto { title: String, starts_at: String, minutes_until: i64 }`.
- Poll interval: 60 seconds. Poll only when `privacy_calendar_context_enabled` is true
  and permission is granted. Polling stops when quiet mode is active.
- Polling task spawned in main.rs; updates CalendarState; emits
  `calendar://event-updated` event to frontend.

New Tauri commands:
- `request_calendar_permission` — triggers the EKEventStore permission request.
- `get_calendar_next_event` — returns current CalendarEventDto from state.
- `get_calendar_permission_status` — returns "granted" | "denied" | "not_determined".

Companion header:
- When CalendarEventDto is present: shows "Meeting in N min — [title]".
- Disappears when no event within 8 hours.
- Only shows when `privacy_calendar_context_enabled` is true.

Re-orientation prompt injection (proactive.rs):
- When generating re-orientation, if CalendarState has an event within 2 hours,
  append one sentence to the system prompt: "The user has a meeting in N minutes: [title]."

Privacy Center toggle: already exists (`privacy_calendar_context_enabled` in store.rs).
The Privacy Center UI from Phase 21 already shows it. No new toggle needed — only
the backend functionality needs wiring.

Verification:
- With permission and an upcoming event, companion header shows event within 10 seconds.
- Calendar context appears in re-orientation when event is within 2 hours.
- Toggling privacy_calendar_context_enabled off stops polling and clears header.
- Without permission, app behaves exactly as before (no polling, no header, no errors).
- No calendar data written to SQLite — state is transient in memory only.

---

### M23.5 — Live App Actions

**Status: [ ]**

Goal: extend the Phase 22 browser-extension bridge with a write path so Jeff
can apply approved edits directly in supported web editors.

This is the most complex milestone in Phase 23. Execute with care.

**Backend: apply-edit endpoint in selection_capture.rs**

The existing HTTP bridge server (Phase 22) is extended with a new route:
`POST /apply-edit`

Request body (JSON):
```json
{
  "token": "<bridge_token>",
  "editor_surface": "storymaps | google_docs",
  "selection_anchor_hash": "<sha256 of the original selected text>",
  "before_text": "<original selected text>",
  "after_text": "<replacement text>",
  "document_title": "<string>"
}
```

The backend:
1. Validates the token (same path as Phase 22).
2. Returns `{ "status": "pending_approval", "receipt_id": <i64> }` immediately.
3. Stores a pending live edit record in the new `live_edit_receipts` DB table.
4. Emits `live_action://apply_requested` event to the frontend with the receipt_id.

The frontend:
- Shows a preview diff (before vs. after in a two-column card).
- Two buttons: "Apply" and "Reject".
- "Apply" calls `approve_live_edit(receipt_id)`.
- "Reject" calls `reject_live_edit(receipt_id)`.

On approval: backend emits `live_action://approved { receipt_id }` to extension.
Extension receives the event via its polling loop or a persistent connection,
then executes the replacement using `execCommand('insertText', ...)` or
`Selection.getRangeAt(0).deleteContents()` + `insertNode`.

**DB: live_edit_receipts table (idempotent migration in store.rs)**
```sql
CREATE TABLE IF NOT EXISTS live_edit_receipts (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    editor_surface TEXT NOT NULL,
    document_title TEXT NOT NULL,
    before_hash   TEXT NOT NULL,
    after_hash    TEXT NOT NULL,
    timestamp     TEXT NOT NULL DEFAULT (datetime('now')),
    status        TEXT NOT NULL DEFAULT 'pending_approval'
    -- status values: pending_approval | approved | rejected | applied | failed | fallback
);
```

**Anchor validation**
Before the extension applies a replacement:
1. Compute SHA-256 of the current selected text in the page.
2. Compare to `selection_anchor_hash` from the receipt.
3. If mismatch: do not apply. Send `POST /apply-fallback { receipt_id }` to backend.
4. Backend updates receipt status to `fallback` and emits `live_action://fallback_triggered`.
5. Frontend shows guided apply: copy-ready patch text + "The document changed — paste this
   manually where it belongs." No silent no-op.

**Extension changes** (browser-extension/selection-capture/):
- background.js: add `/apply-edit` result polling (GET `/pending-approval/<token>`
  or use long-poll). When approval event arrives, dispatch to content.js.
- content.js: add `applyEditInPlace(beforeText, afterText)` function.
  Verifies anchor (SHA-256 comparison), applies via Selection API, sends anchor result
  back to background.

**Supported surfaces for Phase 23:**
- StoryMaps editor (storymaps.arcgis.com) — supported via existing extension manifest
- Google Docs (docs.google.com) — requires content.js extension on that domain.
  Add docs.google.com to extension manifest matches.

**Unsupported app fallback:**
- When a live action is requested from a context where no extension bridge is active
  (native app, unsupported site), the companion shows a guided-apply card:
  copy-ready text, "Paste this where it belongs in [App]." Never a silent no-op.

**New Tauri commands:**
- `approve_live_edit(receipt_id)` — updates receipt status, emits approval event to extension
- `reject_live_edit(receipt_id)` — updates status to `rejected`
- `list_live_edit_receipts(task_id)` — returns receipts for audit view
- `get_pending_live_edits` — returns receipts with status `pending_approval`

Files to modify:
- `desktop/src-tauri/src/selection_capture.rs` — add /apply-edit route, /pending-approval route
- `desktop/src-tauri/src/store.rs` — live_edit_receipts migration + CRUD helpers
- `desktop/src-tauri/src/commands.rs` — approve/reject/list commands
- `desktop/src-tauri/src/main.rs` — register commands
- `desktop/src/App.tsx` — preview diff card, approve/reject buttons, guided-apply card
- `desktop/src/App.test.tsx` — live edit card tests
- `desktop/src/tauriClient.ts` — typed wrappers
- `browser-extension/selection-capture/background.js` — polling loop + apply dispatch
- `browser-extension/selection-capture/content.js` — applyEditInPlace + anchor check
- `browser-extension/selection-capture/manifest.json` — add docs.google.com to matches

Verification:
- In StoryMaps: Jeff proposes rewrite, preview diff shows, user approves, text updates in page.
- In Google Docs: same flow.
- Anchor mismatch triggers fallback path (user-visible, not silent).
- Every apply/reject/fail has a corresponding receipt in live_edit_receipts.
- Unsupported app shows guided-apply copy card, never a blank or silent state.

---

### M23.6 — phase23_check.sh

**Status: [ ]**

Write `scripts/phase23_check.sh` covering:

Personalization checks:
- user_model.rs module exists with expected functions
- user_profile table in DB migration
- profile injection in chat.rs (system prompt prepend path)
- profile injection in proactive.rs (reorientation path)
- rubric write command and inject path exist
- "Jeff remembers" panel and "Clear all" in App.tsx
- privacy gate (privacy_user_profile_memory_enabled) guards injection

Workload checks:
- workload.rs module exists
- get_workload_summary command registered
- switch_active_task_from_companion command restarts watcher
- stale notification throttle key format in app_settings
- cross-task collision check in subtask.rs

Calendar checks:
- calendar.rs module exists (or conditional compilation marker)
- EventKit permission branch present
- calendar poll task spawned from main.rs
- calendar context in companion header render
- calendar context in reorientation system prompt path
- privacy gate (privacy_calendar_context_enabled) guards poll

Live action checks:
- /apply-edit route in selection_capture.rs
- anchor validation in extension content.js
- fallback path code (apply-fallback route)
- live_edit_receipts table in DB migration
- approve_live_edit and reject_live_edit commands registered
- preview diff render in App.tsx
- guided-apply fallback render in App.tsx
- docs.google.com in extension manifest

Behavioral tests:
- `cargo test --manifest-path ... user_model` passes
- `cargo test --manifest-path ... workload` passes
- `npm --prefix desktop test -- --run` passes
- phase22_check.sh still passes (regression guard)

---

## Phase 24: Distribution + Auto-Update

### M24.1 — Universal Binary + Signing + Notarization Config

**Status: [ ]**

Goal: configure the Tauri build for a signed, notarized universal binary.
No actual signing keys committed — config structure only. CI secrets provide creds.

Files to modify:
- `desktop/src-tauri/tauri.conf.json`:
  - `targets`: add `"universal-apple-darwin"` under `bundle.macOS`
    (or via `tauri build --target universal-apple-darwin` in CI)
  - `bundle.macOS.signingIdentity`: `"$(APPLE_SIGNING_IDENTITY)"` (resolved from CI env)
  - `bundle.macOS.providerShortName`: `"$(APPLE_PROVIDER_SHORT_NAME)"` (CI env)
  - `bundle.macOS.entitlements`: point to a new `entitlements.plist` file
  - `bundle.macOS.minimumSystemVersion`: `"13.0"` (Ventura, consistent with Phase 19)
- `desktop/src-tauri/entitlements.plist` — new file with required entitlements:
  - `com.apple.security.cs.allow-jit`
  - `com.apple.security.network.client`
  - `com.apple.security.files.user-selected.read-write`
  - Accessibility and calendar entitlements as needed
- `desktop/src-tauri/Cargo.toml` — add `tauri-plugin-updater`

Notarization: done via `xcrun notarytool submit` in CI (see M24.3).
The Tauri build config does not run notarytool directly — CI handles it.

Minimum macOS: already targeted at 13 via SMAppService (Phase 19). Confirm
`bundle.macOS.minimumSystemVersion = "13.0"` is explicit in tauri.conf.json.

Verification:
- `tauri.conf.json` has universal binary target
- `entitlements.plist` exists with at least the above keys
- `tauri-plugin-updater` in Cargo.toml
- Minimum system version 13.0 present in config

---

### M24.2 — Auto-Update Implementation

**Status: [ ]**

Goal: Jeff silently checks for updates on launch and shows a native dialog.

Files to modify:
- `desktop/src-tauri/src/main.rs` — add tauri_plugin_updater initialization;
  spawn background update check after app is ready
- `desktop/src-tauri/tauri.conf.json` — add updater section:
  ```json
  "updater": {
    "active": true,
    "endpoints": ["https://github.com/OWNER/REPO/releases/latest/download/latest.json"],
    "dialog": false,
    "pubkey": "{{TAURI_PUBLIC_KEY}}"
  }
  ```
  Note: `dialog: false` means we handle the dialog in Rust code (not Tauri's built-in).
  This gives control over the button labels and timing.
- `desktop/src-tauri/Cargo.toml` — tauri-plugin-updater already added in M24.1

Update check logic (main.rs):
1. After app startup (2-second delay to not block tray ready):
   `app.updater().check().await` — silent, background.
2. If update available: show a native OS dialog (tauri dialog plugin):
   "Jeff update available — [Install] [Later]"
3. User clicks Install: `update.download_and_install().await`, then
   `app.restart()`.
4. User clicks Later: no action, check again next launch.
5. No forced update. No update prompt if no update is available.

Update feed format (GitHub Releases `latest.json`):
```json
{
  "version": "x.y.z",
  "notes": "What changed",
  "pub_date": "2026-...",
  "platforms": {
    "darwin-universal": {
      "signature": "...",
      "url": "https://github.com/.../jeff_x.y.z_universal.dmg.tar.gz"
    }
  }
}
```
This file is generated and uploaded by the CI pipeline (M24.3).

Verification:
- tauri-plugin-updater initialized in main.rs
- Background check runs after startup
- Dialog shows "Install" / "Later" options
- Update feed URL is an HTTPS GitHub Releases endpoint
- Update public key (TAURI_PUBLIC_KEY) referenced from env, not hardcoded

---

### M24.3 — Release CI Pipeline (GitHub Actions)

**Status: [ ]**

Goal: automated build → test → sign → notarize → upload on push to `release` branch.

Files to create:
- `.github/workflows/release.yml` — CI pipeline

Pipeline job order (strictly sequential, each job depends on the previous):

1. **test** — runs on `macos-14` (Apple Silicon runner for universal build):
   - `cargo test` (unit tests)
   - `bash scripts/phase17_check.sh` (latency budget regression gate)
   - `npm --prefix desktop test -- --run` (frontend tests)
   - If any test fails: pipeline stops, no signing proceeds.

2. **build** (depends on: test):
   - `cargo tauri build --target universal-apple-darwin`
   - Produces `.app` bundle and unsigned `.dmg`

3. **sign** (depends on: build):
   - Import Developer ID cert from `APPLE_CERTIFICATE` secret
   - `codesign --deep --force --options runtime --sign "$APPLE_SIGNING_IDENTITY" Jeff.app`
   - `codesign --verify --deep --strict Jeff.app`

4. **notarize** (depends on: sign):
   - `xcrun notarytool submit Jeff.dmg --apple-id "$APPLE_ID" --password "$APPLE_APP_PASSWORD"
     --team-id "$APPLE_TEAM_ID" --wait`
   - `xcrun stapler staple Jeff.dmg`

5. **release** (depends on: notarize):
   - Generate `latest.json` with version from `Cargo.toml`, platform entry,
     Sparkle signature (generated with Tauri updater key), and the DMG URL.
   - Upload `.dmg` as GitHub release asset.
   - Upload `latest.json` as release asset (or to a separate static endpoint).

CI secrets required (documented in pipeline comments, not committed):
- `APPLE_CERTIFICATE` — base64-encoded .p12
- `APPLE_CERTIFICATE_PASSWORD`
- `APPLE_SIGNING_IDENTITY`
- `APPLE_ID`
- `APPLE_APP_PASSWORD`
- `APPLE_TEAM_ID`
- `APPLE_PROVIDER_SHORT_NAME`
- `TAURI_PRIVATE_KEY` — for signing updater payloads
- `TAURI_KEY_PASSWORD`

Trigger: `push` to `release` branch.
No pipeline changes affect `master` branch CI (if any).

Verification:
- `.github/workflows/release.yml` exists
- Job order: test → build → sign → notarize → release
- phase17_check.sh runs as a test gate before build proceeds
- Pipeline trigger is `release` branch only
- All secret references use `${{ secrets.X }}` syntax (no hardcoded values)

---

### M24.4 — Architecture Note + Final Verification

**Status: [ ]**

Files to modify:
- `docs/ARCHITECTURE.md` — add at the end of the document:
  ```
  ## Windows-ready when
  - tauri-plugin-updater endpoint has a win32-x86_64 platform entry
  - CI pipeline adds a windows-2022 runner job
  - SMAppService (Phase 19) replaced with Windows Task Scheduler equivalent
  - EventKit (Phase 23) replaced with Windows Calendar API
  - Global shortcut and AXUIElement paths have Windows implementations
  ```

End-to-end distribution smoke test (manual, not automated):
- Build a local release build: `cargo tauri build`
- Mount the resulting DMG, drag to Applications
- Launch Jeff, verify no "unidentified developer" dialog (requires real cert)
- Complete onboarding as a fresh user (no terminal)
- Send a first message successfully

Verification:
- ARCHITECTURE.md has Windows-ready note
- Smoke test passes (documented in check script output)

---

### M24.5 — phase24_check.sh

**Status: [ ]**

Write `scripts/phase24_check.sh` covering:

Config checks:
- universal-apple-darwin target present in tauri.conf.json
- bundle.macOS.minimumSystemVersion is "13.0"
- bundle.macOS.signingIdentity key present (value not required, CI provides it)
- entitlements.plist exists with at least com.apple.security.network.client
- tauri-plugin-updater in Cargo.toml
- updater.endpoints contains an HTTPS URL in tauri.conf.json
- updater.pubkey key present (references TAURI_PUBLIC_KEY)

CI checks:
- `.github/workflows/release.yml` exists
- Trigger is `release` branch
- Job order: test job exists, sign job has `needs: build`, notarize has `needs: sign`
- `phase17_check.sh` is called in the test job
- No hardcoded secrets (grep for APPLE_ID without ${{ syntax))

Auto-update checks:
- `tauri_plugin_updater` initialized in main.rs
- Background check spawned after app ready
- Install / Later dialog wired in main.rs

Behavioral:
- `cargo build --release` succeeds (or `cargo check --release` as proxy)
- phase23_check.sh still passes (regression guard)

---

## Milestone execution order

```
M23.1 → M23.2 → M23.3 → M23.4 → M23.5 → M23.6
                                               ↓
                                M24.1 → M24.2 → M24.3 → M24.4 → M24.5
```

M23.3 (workload) and M23.4 (calendar) do not depend on each other and could be
built in parallel if handing off to Codex, but work one at a time per CLAUDE.md rules.

---

## Handoff notes for Codex resume

If resuming from Codex, specify:
1. The last completed milestone number and sub-item.
2. The exact file currently being edited.
3. Any deviation from this plan discovered during implementation.
4. Run `scripts/phase22_check.sh` first to confirm baseline is still green.

Do not start a new milestone until the previous one's verification items all pass.
