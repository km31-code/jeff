# Jeff — Next Phases (17–24)

## Why this ordering

Phases 0–16 built a complete technical core. The sequencing below prioritizes:
1. Hardening before new capability (Phase 17)
2. Non-technical user unblocking before distribution (Phase 18)
3. Privacy controls before deep content sensing (Phase 21)
4. Distribution before late-stage capability expansion (Phase 24)

This sequence closes all five felt properties from VISION.md and delivers a product
a non-technical user can install, trust, and use daily.

---

## Non-negotiable constraints for all phases

- Local-first. No user data leaves the device unless the user explicitly connects an
  external service (calendar, search API).
- No silent writes. All file writes remain explicitly approval-gated.
- New sensing must be opt-in, scoped to active task, and surfaced in the Privacy Center.
- Every phase ships with a `scripts/phaseN_check.sh` that verifies runtime behavior,
  not only symbol presence.
- Preserve the five felt properties in VISION.md as the north star for every decision.

---

## Phase 17: Reliability and Productization Gate

Stabilize the Phase 0–16 stack before adding capability. The existing grep-based check
scripts verify symbol presence but not runtime behavior. Latency budgets and failure modes
are currently undocumented.

Scope:
- Augment existing check scripts (11–16) with at least one behavioral assertion per phase:
  start/stop flows, cancel flows, streaming flows. Grep checks stay; behavioral checks are
  added on top.
- Document and assert latency budgets: startup to companion-ready (< 2s), first LLM token
  (< 1s), first audio token (< 400ms after first LLM token), classifier (< 150ms p50).
- Audit failure-mode handling for: API timeout, invalid/missing key, missing OS permission,
  DB lock contention. Each must produce a specific actionable message, never a blank crash.
- Add provider abstraction seams for reasoning, STT, TTS, embeddings, and intent classifier
  so call sites are not OpenAI-specific. OpenAI remains the only provider implementation
  in this phase; no second provider is required yet.

Exit criteria:
- `phase17_check.sh` runs behavioral assertions for critical Phase 11–16 paths and passes.
- All four latency budgets are measured and pass on a reference machine.
- The four failure modes (timeout, bad key, missing permission, DB lock) each produce
  correct, specific UI messages rather than blank or generic errors.
- Provider interfaces exist in code and OpenAI implementations conform to them.

---

## Phase 18: First-Run Onboarding + Secure Key Management

Remove technical setup friction. The target user (Georgetown sophomore, lawyer, PM) must
be able to install Jeff and send a first message within 5 minutes without touching a
terminal or reading documentation.

Scope:
- First-run wizard detected via `onboarding_complete` flag in app_settings (absent = first
  run). Four steps, rendered inside the overlay (no separate window):
  1. What Jeff is — three sentences, CTA to continue.
  2. API key setup — text field, validation call, result stored in macOS Keychain via
     tauri-plugin-keychain. The .env file remains a dev-only fallback; the wizard never
     reads or writes it.
  3. Workspace folder — file picker for first task folder, or "skip for now."
  4. Ready — shows the global hotkey, invites first voice or text message.
- Wizard is cancellable at any step; completion sets `onboarding_complete = true`.
- "Set up Jeff again" entry in tray menu re-runs the wizard.
- Specific empty and error states throughout companion view:
  - No active task: "Tell me what you're working on." with a text input, no blank screen.
  - API key invalid: actionable message with a link to fix it, not a generic error banner.
  - No workspace folder set: soft prompt to set one, app fully functional without it.

Exit criteria:
- A fresh user completes onboarding and sends a first message within 5 minutes using only
  in-app guidance.
- Invalid/missing key path is recoverable entirely from companion view UI.
- `phase18_check.sh` verifies: onboarding_complete flag, keychain write/read path, wizard
  step count in frontend, empty-state and error-state render branches.

---

## Phase 19: Presence Completion — Launch at Login + Session Restore

Fulfill "already present when you start working." Jeff must not require a manual launch
each session. Assumed macOS baseline: 13 (Ventura) throughout, using SMAppService.

Scope:
- macOS Login Item registration via SMAppService (macOS 13+).
- In-app toggle in tray menu: "Launch at Login" persisted as `launch_at_login` in
  app_settings.
- On startup: restore active task, workspace watcher, overlay collapsed/expanded state,
  quiet mode, clipboard capture setting — all without user action and without stealing
  focus from the user's current app. set_focus is never called on any window during
  automatic startup.
- Startup time: companion-ready state (tray visible, hotkey registered, active task
  loaded) within 2 seconds of process launch.
- First-launch (no prior session): Jeff starts tray-only and fires a native OS notification
  inviting the user to press the hotkey.

Exit criteria:
- Enabling the toggle appears in macOS Login Items list; Jeff launches on next login.
- On relaunch after a prior session, active task and overlay state are restored within 2
  seconds without any window stealing focus.
- Disabling the toggle removes the login item and is confirmed by SMAppService state.
- `phase19_check.sh` verifies: SMAppService registration code, launch_at_login setting
  round-trips, session-restore command in commands.rs, no set_focus calls in startup path.

---

## Phase 20: Active Window Context (Title-Level)

Fulfill "already knows your task" even when no workspace folder has been configured.
Jeff reads the frontmost app name and window/document title via macOS Accessibility API
on a poll interval. Title only — no document text content.

Scope:
- `context_observer.rs` (new module): polls frontmost app and window title every 3 seconds
  via NSWorkspace + AXUIElement. No continuous listener. Polling stops when quiet mode is
  active.
- macOS accessibility permission request via AXIsProcessTrustedWithOptions on first use,
  with a plain-language explanation in companion: "Jeff needs accessibility permission to
  know which document you have open." Wizard step or inline prompt, not a silent pop-up.
- Active context stored in memory (added to AmbientState or a new ContextState mutex):
  `{ app_name, document_title, captured_at }`. Not persisted to SQLite.
- Companion view header: shows current app + document title (e.g., "Pages — Thesis Draft")
  when permission is granted and a document is active.
- All LLM paths (chat, reorientation, drift) receive active window context prepended as a
  brief system-prompt addition when available.
- Document-switch nudge: when frontmost document title changes to something that does not
  match the active task, Jeff surfaces one soft prompt: "You switched to [document]. Want
  to start or switch tasks?" One prompt per switch, no repeat on the same document.
- Full graceful degradation when permission is not granted: no errors, no empty states,
  no polling. App behaves exactly as before this phase.

Out of scope:
- Reading document text via OCR or accessibility attributes. Title only.
- Any monitoring of content from other users' processes.
- Browser tab content (covered by selection capture in Phase 22).

Exit criteria:
- With permission granted and a document open, companion header shows app and title within
  5 seconds of document open.
- LLM response to "what am I working on?" correctly references the frontmost document
  without any user paste.
- Document-switch nudge fires once per switch and does not repeat for the same document.
- With permission not granted, app runs identically to pre-Phase 20 with no visible
  change.
- `phase20_check.sh` verifies: context_observer.rs module, permission request branch,
  context prepended in at least chat and reorientation system prompts, companion header
  conditional render, graceful-degradation path, no SQLite writes for transient context.

---

## Phase 21: Privacy and Trust Control Center

Add user trust controls before any deep content capture is introduced. Users must be
able to see exactly what Jeff has access to and turn off anything they choose.

This phase exists here — before Phase 22 selection capture and before Phase 23
personalization — because those phases introduce deeper sensing. Users need controls
before the capability, not after.

Scope:
- "What Jeff knows" dashboard, accessible from tray menu and from companion settings.
  Shows each active sensing surface with its current state:
  - Workspace watcher: folder path, files count, on/off toggle.
  - Clipboard capture: on/off toggle, reminder that it is off by default.
  - Active window context: permission status (granted / not granted), on/off toggle.
  - Proactive triggers: on/off toggle (equivalent to quiet mode for trigger surfaces).
  - User profile memory (Phase 23): shows signal count, on/off toggle, clear button.
  - Calendar context (Phase 23): permission status, on/off toggle.
- Data controls:
  - Clear active task data: removes chat history, artifacts, subtask history, and
    ingested chunks for the current task only. Task record itself is kept.
  - Clear all Jeff data: wipes the entire SQLite database, removes the Keychain entry,
    removes the user profile, and resets all app_settings. Requires a confirmation dialog.
    App returns to first-run state after this action.
- Audit view: shows the subtask_write_audit_log and proactive_trigger_log for the active
  task so users can see exactly what Jeff has proposed, applied, and surfaced.
- All toggles are backed by persistent app_settings keys. Changing a toggle takes effect
  immediately.

Exit criteria:
- Every sensing surface listed above has a working toggle that persists across sessions.
- Clear active task data leaves the task record intact but removes all associated content.
- Clear all Jeff data leaves the app in first-run state with no residual data in SQLite,
  Keychain, or profile.
- Audit view accurately reflects write and trigger history for the active task.
- `phase21_check.sh` verifies: dashboard commands in commands.rs, toggle persistence for
  each surface, clear-task-data command, clear-all-data command, audit view commands.

---

## Phase 22: Selection Capture + Voice Naturalness

Two capabilities that together make the daily interaction feel like working with a person
rather than operating a tool. Neither requires new infrastructure — both build on
Phase 12 (streaming) and Phase 20 (accessibility permission already granted).

**Selection capture**

The gap: Jeff knows the document title (Phase 20) but not its content unless a workspace
folder is configured. For a user editing in Pages, Google Docs, or any app that is not
a watched folder, Jeff cannot see the draft. Selection capture closes this gap through
two paths: AX hotkey capture for native apps and a targeted browser-extension bridge for
supported web editors.

Scope:
- A second global hotkey (configurable, default CmdOrCtrl+Shift+V): captures the
  currently selected text from the frontmost app via AXUIElement selected text attribute.
- Captured text is sent to Jeff as context for the next message. The companion view shows
  a brief indicator: "Captured [N words] from [App Name]" with a dismiss control.
- Captured text is stored in memory only (not SQLite, not clipboard). Cleared when the
  session ends or when the user dismisses the indicator.
- Source provenance is attached to each capture: app_name, document_title, captured_at.
  This is visible in the indicator and passed to Jeff in the system prompt.
- Fallback when AXUIElement selected-text attribute is unavailable for the app: indicator
  shows "Could not capture text from [App]. Paste it manually." No silent failure.
- Capture is gated by the accessibility permission from Phase 20 and by the Privacy
  Center toggle from Phase 21.
- Browser extension bridge track (priority in this phase, not deferred): support
  StoryMaps/Docs-style web editors where AX selected text is unreliable. The extension
  captures user-selected text only (no ambient page scraping), sends provenance-tagged
  snippets to Jeff, and is explicitly opt-in per browser/editor surface.

Out of scope:
- Full document text extraction (selection only).
- Continuous background capture (hotkey-triggered only, no ambient reading).

**Voice naturalness**

The gap: Jeff currently speaks whenever the LLM responds, even if the user is actively
typing. Jeff's spoken responses include filler phrases. There is no voice character
consistency. This makes the voice feel like a notification rather than a coworker.

Scope:
- Activity-aware speech: a keystroke-rate monitor (rate only, no key content captured)
  sets `user_is_typing: bool` in AmbientState. When the user is actively typing (> 1
  keystroke per 2 seconds), Jeff delays outgoing TTS by up to 3 seconds. If still typing
  after 3 seconds, the response is delivered as text-only with no audio. The delay and
  text-only path are both implemented in the streaming TTS queue in App.tsx.
- Natural interjections for short responses (under 15 words): Jeff prepends one of a small
  set of natural acknowledgment phrases ("got it", "on it", "here you go") before the
  content. Implemented as a deterministic selection from a fixed list, not an LLM call.
- Brevity filter: applied to all text destined for TTS before synthesis. Removes filler
  phrases ("certainly", "absolutely", "of course", "great question", "sure thing",
  "of course!") via string replacement. No LLM call. Does not affect text-only output.
- Voice character addendum in all chat system prompts: "Be concise. One to three sentences
  unless the user asks for more. No filler phrases."
- TTS voice selection: exposed in tray settings. Users can pick from available OpenAI TTS
  voices. Setting persists as `tts_voice` in app_settings and takes effect on next
  synthesis call.

Exit criteria:
- With accessibility permission granted, pressing the selection-capture hotkey in Pages
  or TextEdit with text selected sends that text to Jeff as context.
- Indicator shows app name and word count; dismiss clears the capture.
- Fallback message appears for apps where AXUIElement selected-text is unavailable.
- In at least one supported web editor (for example StoryMaps-class), browser-extension
  selection capture provides the same provenance + indicator path as native capture.
- When the user is actively typing, TTS is delayed; text-only path fires after 3 seconds.
- Brevity filter removes all specified filler phrases from TTS output.
- Natural interjection appears on responses under 15 words.
- TTS voice setting persists and changes output voice.
- `phase22_check.sh` verifies: selection-capture hotkey registration, AXUIElement read
  path, extension bridge selection-capture path, provenance fields, indicator render,
  fallback branch, keystroke-rate monitor (rate-only), typing-delay path, brevity filter,
  interjection list, voice setting.

---

## Phase 23: Live App Actions + Personalization + Workload Awareness

Phase 23 has three high-priority tracks and is a major execution phase:
1. Live app actions (very important)
2. Personalization
3. Workload awareness

The live-action track is not optional polish. It is a core vision push and should be
executed aggressively with strong quality bars, because Jeff must not stop at advising
edits when a supported app can safely accept them.

**Live app actions (very important)**

Scope:
- Add explicit "Apply in app" action for supported editors so Jeff can execute approved
  rewrites directly in live documents, not only suggest copy-paste.
- Start with web-editor bridge path (from Phase 22) and extend to additional supported
  surfaces where deterministic selection anchoring is available.
- Support paragraph- and section-level patch application with a preview diff before apply.
- Add robust anchor validation before write (selection hash + nearby context check) and a
  recoverable fallback path when anchors drift.
- Keep strict approval gating: every live-app write requires explicit user confirmation.
- Persist write receipts for every live edit attempt: app/editor, document title, before/
  after excerpt hash, timestamp, status (applied/rejected/failed/fallback).
- Unsupported apps always degrade to guided apply (copy-ready patch + quick instructions),
  never a broken or silent no-op path.

Exit criteria:
- In at least two supported live-editor surfaces, Jeff can apply an approved rewrite in
  place with preview + confirmation.
- Drift/anchor mismatch never causes silent corruption; failed anchors always fall back to
  guided apply with user-visible reason.
- Every applied/rejected/failed live write has a corresponding receipt entry.
- `phase23_check.sh` verifies: live-action command path, preview/approval gate, anchor
  validation and drift fallback, receipt logging, supported/unsupported app behavior.

**Personalization**

Scope:
- `user_model.rs` (new module): builds and maintains a compact user profile in SQLite.
- `user_profile` table: key-value pairs updated incrementally after each session.
  ```sql
  CREATE TABLE IF NOT EXISTS user_profile (
      key TEXT PRIMARY KEY,
      value TEXT NOT NULL,
      updated_at TEXT NOT NULL DEFAULT (datetime('now'))
  );
  ```
- Profile signals captured (all local, never transmitted):
  - Writing style: average sentence length and formality score derived from accepted
    revision text.
  - Delegation patterns: which subtask execution_types the user accepts vs. rejects.
  - Work rhythm: hour-of-day distribution of task focus events from task_focus_log.
  - Response length preference: average word count of responses the user does not dismiss.
  - Quality rubrics: user-authored preference notes ("I prefer bullet points over prose",
    "Always cite sources") saved via a text input in companion settings and injected
    verbatim into revision and drafting prompts.
  - Feedback signals: when a user rewrites a Jeff-generated draft, the delta is used to
    update style signals. When a user dismisses a proactive trigger, the trigger type is
    down-weighted in the profile.
- Profile injection: compact summary (< 100 tokens) prepended to chat, revision, and
  reorientation system prompts. Gated: if no signals exist yet, nothing is injected.
- "Jeff remembers" panel in companion: shows 2–3 active profile signals in plain language.
  Each signal has a delete button. "Clear all" wipes the user_profile table.
- Profile updates are async and never block any user-facing response path.

**Workload awareness**

Scope:
- `workload.rs` (new module): computes a workload summary across all tasks on demand,
  cached for 5 minutes in memory.
  - Active tasks: focused in the last 14 days.
  - Stale tasks: no focus for > 14 days.
  - Tasks with pending items: unresolved file write proposals, running subtasks, or
    unreviewed speculative subtask results.
- "Your workload" section in companion view: collapsible, shows active tasks with
  last-focused timestamp and any pending item count. Task items are clickable to switch.
- Task switching from companion: switches active task and restarts the workspace watcher
  on the new task's folder, without opening the full workspace.
- Stale-task notification: if a stale task has an unreviewed subtask result, Jeff fires
  one native OS notification per task per 24 hours. Respects quiet mode.
- Cross-task collision detection: when starting a new subtask, Jeff checks whether any
  other task has a recently-completed subtask with cosine similarity > 0.8 to the new
  one. If so, one soft companion notice: "Similar work exists in [task] — want me to
  pull it in?" No auto-merge.

**Calendar context (lightweight, opt-in)**

Scope:
- macOS EventKit integration: reads events from local calendar (no OAuth, no remote
  call, device-local data only). Reads the next 8 hours only.
- Calendar permission requested on toggle-on, with a plain-language explanation.
- Upcoming events surface in companion header: "Meeting in 45 min — Design review."
- Jeff includes the next-event context in re-orientation messages when an event is within
  2 hours and the feature is enabled.
- Toggle in Privacy Center (Phase 21 dashboard) and in companion settings.
- Stores nothing calendar-related in SQLite. All calendar data is transient in memory.

Exit criteria:
- Live app actions work in at least two supported surfaces with explicit approval and
  reliable fallback.
- After 5 accepted revisions, Jeff's revision proposals reflect the user's style signals.
- After a quality rubric is saved, the next revision prompt includes it verbatim.
- "Jeff remembers" panel shows at least one signal after 5 meaningful interactions.
- Clear all wipes the user_profile table; panel empties immediately.
- Workload section shows all tasks focused in the last 14 days with correct pending counts.
- Switching tasks from companion restarts the workspace watcher correctly.
- Stale-task notification fires once per task per 24 hours maximum and is suppressed in
  quiet mode.
- Cross-task collision notice appears for subtask cosine similarity > 0.8 and not below.
- With calendar permission and an upcoming event, companion header shows the event within
  10 seconds of polling.
- Calendar context appears in re-orientation when an event is within 2 hours.
- `phase23_check.sh` verifies: user_model.rs module, user_profile table, profile
  injection in chat.rs and proactive.rs, rubric write/inject path, remember/clear UI,
  workload.rs module, workload summary command, task-switch command and watcher restart,
  stale-task notification throttle, collision check in subtask creation path, EventKit
  permission branch, calendar context in companion header, calendar context in
  reorientation prompt, opt-in enforcement, plus all live-action checks above.

---

## Phase 24: Distribution + Auto-Update

Make Jeff installable and maintainable outside developer workflows. No non-technical user
should need a terminal, a Rust toolchain, or a .env file.

Scope:
- macOS universal binary (x86_64 + arm64): `cargo build --target universal-apple-darwin`
  via `tauri build`.
- Code signing: Developer ID Application certificate applied via Tauri build config.
- Notarization: `xcrun notarytool submit` via Apple notary API using App Store Connect API
  key. Automated, not interactive.
- Distribution artifact: signed, notarized .dmg with drag-to-Applications install. No
  separate installer package required.
- Auto-update: tauri-plugin-updater with Sparkle 2. Update feed hosted on a static HTTPS
  endpoint (GitHub Releases). On launch, silent background check; if an update is
  available, a native macOS dialog appears: "Jeff update available — [Install] [Later]".
  User can dismiss; update is not forced.
- Release CI pipeline (GitHub Actions): build → test (unit + phase17 regression matrix)
  → sign → notarize → upload release asset. Triggers on push to `release` branch.
- Release health checklist: Phase 17 latency budgets must pass as a CI step before
  signing proceeds.
- Minimum macOS version: 13 (Ventura), consistent with SMAppService (Phase 19).
- Windows is tracked as the next platform (Tauri supports it). Architecture doc gets a
  "Windows-ready when:" note. No implementation in this phase.

Exit criteria:
- A non-technical user can download the .dmg, drag Jeff to Applications, complete
  onboarding (Phase 18), and send a first message without opening a terminal.
- macOS Gatekeeper passes (no "unidentified developer" dialog).
- Sparkle update: test channel delivers a new version; user clicks Install; app relaunches
  to the updated version.
- CI pipeline runs build + test + sign + notarize on every push to `release` branch.
- `phase24_check.sh` verifies: universal binary target in tauri.conf.json, sign and
  notarize config present, tauri-plugin-updater in Cargo.toml, update feed URL in config,
  CI workflow file with correct job order (test before sign).

---

## v1 completion definition

Jeff v1 is complete when all of the following are true after Phase 24:

- Installs for a non-technical user without a terminal (Phase 18, 24)
- Launches and recovers silently at login without manual bootstrapping (Phase 19)
- Knows the active document without folder setup (Phase 20)
- Exposes user controls for every sensing surface before deep capture is enabled (Phase 21)
- Can read selected draft text when the user presses a hotkey (Phase 22)
- Speaks naturally, does not interrupt active typing (Phase 22)
- Learns the user's style and rubrics across sessions (Phase 23)
- Sees the full workload across all tasks (Phase 23)
- Surfaces upcoming calendar events as context (Phase 23)
- Applies approved edits directly in supported live apps, with reliable fallback in
  unsupported/drift cases (Phase 23)
- Performs parallel multi-step work with approval-gated file writes (Phase 16)
- Initiates re-orientation, drift flagging, and speculative subtask (Phase 15)
- Updates safely through a signed release channel (Phase 24)

That is a complete, trustworthy, daily-usable coworker for a non-technical user.

---

## Deferred after v1

The following are explicitly out of scope for Phases 17–24. They are not forgotten —
they are sequenced after the core product is stable and trusted.

- **Universal browser coverage** for selection/live-apply: full cross-browser parity
  remains post-v1 after supported-surface behavior is stable.
- **Generalized cross-app automation** (beyond supported surfaces): remains post-v1 due
  app-level variability and safety complexity.
- **Web grounding in subtask chains**: a search API call inside subtask steps. The
  capability exists in design but the complexity (rate limits, result quality, source
  transparency) is better addressed once the core chain execution is proven in the wild.
- **Email integration**: requires OAuth, high privacy surface, moving-target APIs.
- **Windows support**: next platform after macOS distribution is stable.
- **Local / offline model runtime**: valuable for privacy and offline use; deferred until
  after the OpenAI provider path is fully productionized and the provider abstraction
  seams (Phase 17) make it an adapter, not a rewrite.
- **Voice emotion / stress detection**: out of scope for v1 and post-v1.
- **Mobile companion**: fundamentally different UX surface; not in v1 scope.
- **Team / multi-user backend**: Jeff is explicitly a personal coworker. Team features
  require a backend service with auth, data isolation, and compliance work.

---

## Execution order

```
17 → 18 → 19 → 20 → 21 → 22 → 23 → 24
```

Parallelization:
- 19 and 20 can be built in parallel by split ownership (startup/presence vs. context
  observer module) since they touch different Rust modules and different frontend areas.
- 25+ (any future post-v1 phases) should not start until Phase 24 is shipped and
  real-world usage data is available to inform prioritization.

Sequencing rationale:
- 17 before everything: reduce fragility before adding surface.
- 18 before broader rollout: non-technical users must be unblocked before distribution.
- 21 before 22: trust controls must exist before selection-content capture is introduced.
- 24 before post-v1 work: distribution quality must be validated before compounding scope.
