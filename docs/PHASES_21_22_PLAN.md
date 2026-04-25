# Jeff Phases 21-22 Plan

**Status:** Phase 21 complete; Phase 22 complete.

**Last updated:** 2026-04-25

**Source docs read first:** `CLAUDE.md`, `docs/VISION.md`, `docs/PHASES.md`,
`docs/PHASES_NEXT.md`, `docs/ARCHITECTURE.md`.

## Scope Boundary

This plan covers Phase 21 and Phase 22 only.

Do not implement Phase 23 personalization, workload awareness, calendar context, or
live app actions here. Phase 21 may add privacy-control rows and persistent settings
for future Phase 23 surfaces because `docs/PHASES_NEXT.md` explicitly requires those
controls before deeper sensing exists. Those rows must remain control/status surfaces,
not Phase 23 capability implementation.

Do not start Phase 22 selection capture until Phase 21 privacy controls and data
controls are implemented and verified. The ordering is part of the trust model:
users get controls before deeper content capture.

Every milestone below requires:
- a narrow implementation pass for that milestone only
- focused tests
- the relevant phase check script updated or created
- runtime proof where the milestone has user-visible behavior
- this file updated before moving to the next milestone

## Product North Star

The work must serve the five felt properties from `docs/VISION.md`:

- already present when the user starts working
- already knows the task without briefing or context pasting
- can be interrupted and can interrupt naturally
- does parallel work while the user keeps going
- initiates conversation instead of only responding

Phases 21 and 22 are specifically about making that presence trustworthy. Phase 21
lets users see and control every sensing surface. Phase 22 adds selected-text context
and less disruptive voice behavior only after those controls exist.

## Phase 21: Privacy and Trust Control Center

**Phase status:** complete.

### M21.1 Privacy Settings Model and Enforcement

**Status:** complete.

Completed:
- persistent `app_settings` privacy keys for all Phase 21 sensing surfaces
- runtime guards for workspace watcher, clipboard polling, active-window context, and
  proactive triggers
- future Phase 23 profile/calendar controls as inert persisted toggles only

**Goal:** create one authoritative privacy-control layer backed by persistent
`app_settings` keys.

Planned work:
- define typed privacy settings for:
  - workspace watcher
  - clipboard capture
  - active window context
  - proactive triggers
  - user profile memory placeholder for Phase 23
  - calendar context placeholder for Phase 23
- add store helpers and Tauri commands to read and update the privacy settings as
  one dashboard payload and as individual toggles
- enforce settings immediately in existing runtime paths:
  - workspace watcher off stops the active watcher and prevents startup restore
  - clipboard capture off stops polling and blocks ingestion even if older per-task
    clipboard settings are enabled
  - active window context off stops polling, hides the header context, and suppresses
    permission prompts
  - proactive triggers off suppresses reorientation, drift, speculative subtask, and
    proactive notification surfaces without disabling ordinary chat
  - future Phase 23 toggles persist but do not implement Phase 23 behavior

Acceptance checks:
- toggles persist across app restart through `app_settings`
- each toggle has a direct runtime effect
- disabling a sensing surface never leaves a background poller running
- default state preserves existing behavior except where a surface was already off by
  default, such as clipboard capture

### M21.2 Privacy Center Dashboard Surface

**Status:** complete.

**Goal:** expose "What Jeff knows" from tray and companion settings.

Planned work:
- add a Privacy Center view or modal reachable from:
  - tray menu
  - companion/settings surface
- show current state for every sensing surface:
  - workspace watcher: active task folder path, watched file count, on/off toggle
  - clipboard capture: on/off toggle and "off by default" reminder
  - active window context: accessibility permission status and on/off toggle
  - proactive triggers: on/off toggle
  - user profile memory: signal count, on/off toggle, clear button
  - calendar context: permission/status placeholder and on/off toggle
- make state copy plain and user-facing, not developer terminology
- keep Jeff usable if every sensing surface is off

Acceptance checks:
- dashboard renders correctly with no active task
- dashboard renders correctly with an active task and watched folder
- every toggle updates UI state immediately and persists
- tray entry opens the same surface as companion settings

### M21.3 Audit View

**Status:** complete.

**Goal:** let users inspect exactly what Jeff proposed, applied, rejected, and surfaced.

Planned work:
- add command support for listing proactive trigger history for the active task
- reuse existing `list_write_audit_log` for file-write audit entries
- add an Audit section inside Privacy Center for the active task
- show:
  - subtask write audit log entries from `subtask_write_audit_log`
  - proactive trigger entries from `proactive_trigger_log`
- include timestamps, status/action, trigger/write type, and user-visible summary

Acceptance checks:
- empty audit history has a clear empty state
- active-task audit history matches existing database records
- no audit entry is editable from this surface

### M21.4 Clear Active Task Data

**Status:** complete.

**Goal:** remove Jeff-held content for the active task while keeping the task record.

Planned work:
- add a confirmation-gated command for clearing current-task data
- within one transactional store operation, remove task-scoped:
  - chat messages
  - artifacts and artifact versions
  - retrieval chunks and ingested item records
  - watched folder registry entries
  - recently learned entries
  - clipboard capture setting for the task
  - subtasks, subtask steps, subtask file-write proposals, and write audit rows
  - suggestions, session mode state, task focus rows, and proactive trigger rows
- keep:
  - the `tasks` row
  - active task selection if this was active
  - user workspace files on disk
- stop watcher and clipboard polling for the task before or during clear
- refresh UI state immediately after clear

Acceptance checks:
- task still exists and remains switchable
- chat/history/context/audit surfaces for that task are empty after clear
- no user workspace files are deleted
- watcher/clipboard poll are not left running for cleared data

### M21.5 Clear All Jeff Data

**Status:** complete.

**Goal:** reset Jeff to first-run state with no residual local data.

Planned work:
- add a high-friction confirmation dialog requiring explicit confirmation text
- stop all background watchers, clipboard polls, context polling, proactive triggers,
  and in-flight background work before clearing
- delete the OpenAI API key from macOS Keychain through existing keychain helper
- wipe or recreate the SQLite database so all Jeff tables are empty
- reset in-memory state that mirrors persistent data
- clear all `app_settings`, including onboarding, privacy toggles, launch-at-login
  preference, overlay state, and future Phase 23 placeholder toggles
- return UI to first-run onboarding state

Acceptance checks:
- `get_onboarding_status` reports first-run state after clear
- keychain API key source reports none after clear
- SQLite contains no user task/content/profile data after clear
- app can continue running and complete onboarding again without restart

### M21.6 Phase 21 Verification Script

**Status:** complete.

**Goal:** make Phase 21 completion reproducible.

Planned work:
- create `scripts/phase21_check.sh`
- verify command registration for dashboard, toggles, clear-task-data,
  clear-all-data, and audit view
- verify app_settings keys and persistence helpers
- verify watcher/context/proactive enforcement guards
- run targeted Rust tests for store clearing and privacy setting round trips
- run frontend tests for Privacy Center rendering, toggles, audit view, and
  confirmation flows

Acceptance checks:
- `scripts/phase21_check.sh` passes from a clean shell
- the script checks behavior, not only symbol presence

## Phase 22: Selection Capture and Voice Naturalness

**Phase status:** complete.

### M22.1 Selection Capture State, Privacy Gate, and Hotkey

**Status:** complete.

**Goal:** add explicit, user-triggered selected-text capture without ambient content
reading.

Planned work:
- add in-memory selected-text state with:
  - text
  - word count
  - app name
  - document title
  - captured timestamp
  - source type: native accessibility or browser extension
- never persist captured text to SQLite, files, logs, or clipboard
- register a second global hotkey, default `CmdOrCtrl+Shift+V`, configurable later in
  settings if the default conflicts
- gate the hotkey path on:
  - Accessibility permission from Phase 20
  - Phase 21 selection/content capture privacy toggle
- show a clear fallback if the hotkey cannot be registered

Acceptance checks:
- with privacy toggle off, hotkey does nothing except show an explanatory disabled state
- captured text exists only in memory
- hotkey conflict is visible and recoverable

### M22.2 Native AX Selected Text Capture

**Status:** complete.

**Goal:** capture selected text from native macOS apps through Accessibility APIs.

Planned work:
- add a macOS selection capture module that reads the frontmost focused element's
  selected-text accessibility attribute on hotkey press
- attach provenance from active window context when available
- normalize and size-limit captured text to avoid accidental huge captures
- return explicit fallback messages when selected text is unavailable:
  - no permission
  - no frontmost app
  - app does not expose selected text
  - no text selected
- validate manually in TextEdit or Pages, with TextEdit as the minimum reliable local
  runtime proof target

Acceptance checks:
- selected text in a native app produces captured text plus provenance
- unavailable selection shows "Could not capture text from [App]. Paste it manually."
- no polling or continuous document reading is introduced

### M22.3 Capture Indicator and Prompt Injection

**Status:** complete.

**Goal:** make captured context visible, dismissible, and useful for the next message.

Planned work:
- show indicator in companion and overlay:
  - "Captured [N words] from [App Name]"
  - include document title when available
  - dismiss control clears in-memory capture
- prepend captured text and provenance to chat/reorientation context for the next user
  message only
- clear captured text after it is consumed by a message or dismissed
- ensure captured text does not enter chat history unless the assistant response quotes
  or summarizes it as part of normal conversation

Acceptance checks:
- indicator appears after capture
- dismiss immediately removes captured context from memory
- the next message can answer from captured text without user paste
- a later message does not keep using stale captured text

### M22.4 Browser Extension Bridge for Supported Web Editors

**Status:** complete.

**Goal:** support selected-text capture in StoryMaps/Docs-style web editors where native
AX selected text is unreliable.

Planned work:
- add a minimal opt-in browser-extension bridge for supported editor surfaces
- extension behavior:
  - captures only user-selected text
  - captures only after explicit user action or configured capture hotkey
  - never scrapes ambient page content
  - attaches provenance: browser, site/editor, document title, captured timestamp
  - sends snippets to Jeff through a local-only bridge
- app bridge behavior:
  - accepts snippets only from the extension with an app-generated local token or
    equivalent pairing guard
  - stores snippets only in the same in-memory selection state as native capture
  - respects Phase 21 privacy toggle
- start with one supported web-editor class and document unsupported surfaces clearly

Acceptance checks:
- one supported web editor reaches the same indicator and prompt-injection path as
  native capture
- extension cannot send when the Privacy Center toggle is off
- no ambient page scrape path exists

### M22.5 Keystroke-Rate Typing Awareness

**Status:** complete.

**Goal:** prevent Jeff's voice from interrupting active typing.

Planned work:
- add a rate-only typing monitor:
  - counts keydown events over time
  - does not record key values, key codes, text, or target app content
  - stores only `user_is_typing: bool` and recent rate metadata in memory
- threshold: actively typing means more than 1 keystroke per 2 seconds
- gate monitoring through the Privacy Center and OS permission state
- feed typing state into the frontend streaming TTS queue
- technical risk to resolve before implementation: macOS may require Input Monitoring
  permission for global key-rate monitoring, separate from Accessibility. If so, stop
  and surface the permission requirement before claiming the milestone complete.

Acceptance checks:
- typing state changes without storing key content
- quiet/privacy off disables the monitor
- tests prove only rates/booleans are exposed to app state

### M22.6 Activity-Aware TTS Queue

**Status:** complete.

**Goal:** delay or suppress speech while the user is typing.

Planned work:
- update streaming TTS playback scheduling:
  - if `user_is_typing` is false, play normally
  - if true, delay outgoing TTS playback up to 3 seconds
  - if still typing after 3 seconds, deliver text-only and discard queued audio
- apply the same behavior to non-streaming speech playback if that path is still used
- keep interruption and cancellation behavior from Phase 12 intact

Acceptance checks:
- TTS delay starts when typing is active
- after 3 seconds of continued typing, no audio plays for that response
- if typing stops before 3 seconds, queued audio resumes in order
- cancellation still drains queued audio immediately

### M22.7 Spoken Brevity, Interjections, and Voice Character

**Status:** complete.

**Goal:** make Jeff sound less like a notification and more like a concise coworker.

Planned work:
- add a deterministic TTS text transform before synthesis:
  - remove filler phrases listed in `docs/PHASES_NEXT.md`
  - do not mutate text-only chat output
- add deterministic natural interjection selection for spoken responses under 15 words:
  - fixed list only
  - no extra LLM call
  - stable selection based on turn id or phrase hash
- add voice character addendum to chat system prompts:
  - "Be concise. One to three sentences unless the user asks for more. No filler
    phrases."
- avoid adding the interjection to written text unless explicitly required by runtime
  proof; the target surface is spoken output

Acceptance checks:
- filler phrase removal is case-insensitive and punctuation-safe
- text-only output remains unchanged by TTS transforms
- short spoken responses include one deterministic interjection
- all chat prompt paths include the voice character addendum

### M22.8 TTS Voice Selection

**Status:** complete.

**Goal:** let users choose Jeff's spoken voice.

Planned work:
- add persistent `tts_voice` app setting
- expose voice selection in tray settings and companion settings
- validate selected voice against available OpenAI TTS voices known to the app
- use the selected voice on the next synthesis call for streaming and non-streaming
  TTS
- keep a sensible default if setting is absent or invalid

Acceptance checks:
- voice setting persists across restart
- changing voice affects the next TTS request payload
- invalid voice setting falls back without crashing

### M22.9 Phase 22 Verification Script

**Status:** complete.

**Goal:** make Phase 22 completion reproducible.

Planned work:
- create `scripts/phase22_check.sh`
- verify:
  - selection-capture hotkey registration
  - native AX selected-text path
  - browser-extension bridge path
  - provenance fields
  - in-memory-only capture state
  - indicator render and dismiss behavior
  - fallback branch
  - privacy gating
  - rate-only typing monitor
  - typing-delay and text-only TTS paths
  - brevity filter
  - deterministic interjection list
  - `tts_voice` setting and request payload use
- run targeted Rust and frontend tests

Acceptance checks:
- `scripts/phase22_check.sh` passes from a clean shell
- manual runtime proof covers native capture and at least one supported web-editor
  capture path

## Resume Log

- 2026-04-24: drafted plan from `CLAUDE.md`, `docs/VISION.md`,
  `docs/PHASES.md`, `docs/PHASES_NEXT.md`, `docs/ARCHITECTURE.md`, plus a
  read-only scan of existing commands/settings/audit/privacy-adjacent code.
- 2026-04-24: completed Phase 21 implementation. Added persistent privacy
  controls, Privacy Center UI, tray entry, audit view, clear active task data,
  clear all Jeff data, focused tests, and `scripts/phase21_check.sh`.
  Verification: `./scripts/phase21_check.sh` passed with 47 checks and 0
  failures.
- 2026-04-25: completed Phase 22 implementation. Added in-memory selected-text
  capture state, `CmdOrCtrl+Shift+V` capture hotkey, macOS AX selected-text
  capture, token-gated localhost browser bridge, optional StoryMaps/Docs browser
  extension, capture indicator and dismiss flow, one-turn prompt injection,
  rate-only typing activity, activity-aware TTS delay/text-only fallback, spoken
  brevity/interjection transform, persistent `tts_voice`, Privacy Center/tray
  voice controls, frontend tests, and `scripts/phase22_check.sh`.
  Verification: `./scripts/phase22_check.sh` passed with 48 checks and 0
  failures; `cargo test --manifest-path desktop/src-tauri/Cargo.toml --bin
  jeff-desktop` passed with 153 tests; `npm test -- --run` passed with 28
  tests; `npm run build`, `npm run lint`, `cargo build --manifest-path
  desktop/src-tauri/Cargo.toml`, and `git diff --check` passed. Manual TextEdit
  and StoryMaps extension runtime proof still requires an interactive GUI
  selection and browser-extension install on the user's machine.
