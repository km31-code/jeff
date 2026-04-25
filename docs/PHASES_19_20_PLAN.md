# Jeff — Phase 19 + Phase 20 Implementation Plan

**Status:** Phase 19 complete; Phase 20 complete.

## Status legend
- `[ ]` not started
- `[~]` in progress
- `[x]` complete

## Parallelization note

Per PHASES_NEXT.md, Phases 19 and 20 touch disjoint areas and can be
built concurrently if split by ownership:
- Phase 19 owns: `main.rs` startup path, `ambient.rs` tray, `store.rs`
  app_settings keys, `commands.rs` session-restore command.
- Phase 20 owns: new `context_observer.rs`, `state.rs` extension,
  `commands.rs` new commands, `Overlay.tsx` header, `chat.rs` +
  `proactive.rs` system-prompt injection.

No milestone from Phase 20 depends on a Phase 19 milestone. Both phases
share `main.rs` (startup wiring) — that seam is the only coordination
point. Merge Phase 19 startup additions before Phase 20 adds its own
setup hook to avoid a conflict.

---

## Phase 19: Presence Completion — Launch at Login + Session Restore

Goal: Jeff is already present at every login without a manual launch.
On relaunch, the full prior session state (active task, watcher,
overlay mode, quiet mode, clipboard capture) is restored silently,
with no window stealing focus.

### M19.1 — Login Item Registration (backend)
`[x]`

**What this does:** Wires macOS 13+ `SMAppService.mainAppService`
through `login_item.rs` so Jeff can register/deregister the main app as
a real Login Item. Adds the `launch_at_login` app_setting key for
persistence, but only updates it after the OS state accepts the request
or reports pending user approval.

**Files touched:**
- `desktop/src-tauri/Cargo.toml` — add `objc2` for the Objective-C
  runtime bridge used by ServiceManagement.
- `desktop/src-tauri/src/login_item.rs` — native SMAppService wrapper:
  `login_item_status()`, `login_item_enabled_or_pending()`, and
  `set_login_item_enabled(enabled)`.
- `desktop/src-tauri/src/store.rs` — add constant
  `APP_SETTING_LAUNCH_AT_LOGIN = "launch_at_login"`, plus
  `get_launch_at_login() -> Result<bool>` and
  `set_launch_at_login(enabled: bool) -> Result<()>`
- `desktop/src-tauri/src/commands.rs` — two new commands:
  `get_launch_at_login` reconciles the app_setting with SMAppService
  state, and `set_launch_at_login(enabled: bool)` calls
  `set_login_item_enabled` before persisting.
- `desktop/src-tauri/src/main.rs` — on startup: read `launch_at_login`
  from store and sync SMAppService state. If registration fails, clear
  the setting so the tray checkmark cannot lie.
- `desktop/src-tauri/src/lib.rs` — expose `login_item` for tests.

**Exit criteria for this milestone:**
- `get_launch_at_login` command round-trips correctly.
- `set_launch_at_login(true)` registers SMAppService mainAppService or
  returns a clear OS error without updating persisted state.
- `set_launch_at_login(false)` removes the login item.

---

### M19.2 — Tray Menu "Launch at Login" Toggle
`[x]`

**What this does:** Adds a checkmark tray menu item so non-technical
users can toggle launch-at-login without opening any settings panel.

**Files touched:**
- `desktop/src-tauri/src/ambient.rs` — `install_tray()`: add a
  "Launch at Login" `CheckMenuItem` whose checked state is read from
  `store.get_launch_at_login()` at tray install time. The click handler
  calls `set_launch_at_login(!current)` on `JeffState`.
  Note: Tauri 2 tray `CheckMenuItem` requires the app handle to update
  state; pattern follows the existing quiet-mode tray toggle at line 480.

**Exit criteria for this milestone:**
- Clicking "Launch at Login" in the tray menu toggles the checkmark
  and the `launch_at_login` app_setting flips accordingly.
- The checkmark reflects the true persisted state on app relaunch.

---

### M19.3 — Overlay Mode + Quiet Mode Persistence
`[x]`

**What this does:** `overlay_mode` and `quiet_mode` currently live only
in `AmbientState` (in-memory, reset to defaults on each launch). This
milestone persists both to app_settings and restores them on startup.

**Files touched:**
- `desktop/src-tauri/src/store.rs` — add constants
  `APP_SETTING_OVERLAY_MODE = "overlay_mode"` and
  `APP_SETTING_QUIET_MODE = "quiet_mode"`. Add
  `get_overlay_mode() -> Result<OverlayMode>` (defaults to Collapsed),
  `set_overlay_mode(mode: OverlayMode) -> Result<()>`,
  `get_quiet_mode() -> Result<bool>` (defaults false),
  `set_quiet_mode(quiet: bool) -> Result<()>`.
  `OverlayMode` must be imported or re-exported from `ambient.rs`.
- `desktop/src-tauri/src/ambient.rs` — `ambient_set_overlay_mode`
  command: after updating `AmbientState`, also call
  `state.store.set_overlay_mode(mode)`. Same for `ambient_set_quiet_mode`.
  Both require access to `JeffState` — add it as an additional `State`
  parameter (already available from tauri State<'_, JeffState>).
- `desktop/src-tauri/src/main.rs` — in the `setup` hook, after
  `AmbientState::new()` is managed, read overlay_mode and quiet_mode
  from the store and apply to `AmbientState` before any windows are
  shown. Must happen before `build_overlay_window`.

**Exit criteria for this milestone:**
- Launch Jeff in expanded mode, quit, relaunch: overlay opens collapsed
  (correct: expanded state is per-session by default) — actually re-read
  spec: "restore overlay collapsed/expanded state". So if the user had it
  expanded, it should come back expanded. Verify round-trip.
- Toggle quiet mode, quit, relaunch: quiet mode is still on.

---

### M19.4 — `restore_session` Command + First-Launch Notification
`[x]`

**What this does:** Consolidates session restore into a single named
command. Adds first-launch-of-a-session notification for users who have
Jeff set to launch at login.

**Files touched:**
- `desktop/src-tauri/src/commands.rs` — new Tauri command
  `restore_session(state, ambient_state)`:
  1. Restores active task's workspace watcher (reuses existing
     `restore_workspace_awareness_for_active_task`).
  2. Restores clipboard capture setting for the active task (reads
     `get_clipboard_capture` from store and calls
     `sync_clipboard_poll_for_active_task`).
  3. Returns a `SessionRestoreDto { had_active_task: bool,
     overlay_mode: OverlayMode, quiet_mode: bool }` for the frontend.
- `desktop/src-tauri/src/main.rs` — replace the current inline call to
  `restore_workspace_awareness_for_active_task` in `setup` with a call
  to the new `restore_session` logic (or keep the inline call and add
  `restore_session` as the public Tauri command wrapper).
  After restore: if no active task exists AND `session_restored_at`
  app_setting is absent (true first launch), call
  `ambient::dispatch_notification` with the message
  "Jeff is running. Press Cmd+Shift+J to bring it up." and then write
  `session_restored_at = "1"` to app_settings.
- `desktop/src-tauri/src/main.rs` — audit startup path end-to-end for
  any `set_focus` call. Current code never calls `set_focus` in the
  `show_overlay` path (confirmed: line 242 of `ambient.rs`). Assert this
  by grepping in the check script.
- `desktop/src-tauri/src/main.rs` — add `commands::restore_session` to
  `invoke_handler`.

**Exit criteria for this milestone:**
- `restore_session` command returns correct active task and state.
- On first-ever launch (clean install), native notification fires.
- On subsequent launches, notification does not repeat.
- No `set_focus` appears in the startup code path.

---

### M19.5 — `scripts/phase19_check.sh`
`[x]`

**What this does:** Writes the behavioral check script.

**Verifications:**
1. `grep "SMAppService"` in `login_item.rs`.
2. `grep "APP_SETTING_LAUNCH_AT_LOGIN"` exists in `store.rs`.
3. `grep "APP_SETTING_OVERLAY_MODE\|APP_SETTING_QUIET_MODE"` exist in `store.rs`.
4. `grep "restore_session"` exists in `commands.rs`.
5. `grep "set_focus" desktop/src-tauri/src/main.rs` returns nothing
   (no set_focus in main startup path).
6. `grep "session_restored_at"` exists in `main.rs` or `commands.rs`.
7. Behavioral: run `cargo test --bin jeff-desktop session_settings_round_trip`.
8. Behavioral: run `cargo test --bin jeff-desktop login_item`.
9. Regression: run `cargo test --bin jeff-desktop`.

**Files touched:**
- `scripts/phase19_check.sh` (new file)

---

## Phase 20: Active Window Context (Title-Level)

Goal: Jeff knows which app and document the user has open without the
user configuring a workspace folder. Context is title-only (no document
content). Full graceful degradation when the macOS Accessibility
permission is not granted.

### M20.1 — `context_observer.rs` Module + Permission Check
`[x]`

**What this does:** Creates the new module that polls NSWorkspace and
AXUIElement. No UI yet — pure backend plumbing and permission gate.

**Files touched:**
- `desktop/src-tauri/Cargo.toml` — add:
  - `objc2 = "0.6.4"`
  This provides Objective-C runtime access for NSWorkspace. AXUIElement
  is accessed via raw FFI against `ApplicationServices` framework
  (already linked on macOS; no new Cargo dep needed).
- `desktop/src-tauri/src/context_observer.rs` (new file):
  - `pub struct ActiveWindowContext { pub app_name: String, pub document_title: String, pub captured_at: i64 }`
  - `pub fn is_accessibility_trusted() -> bool` — wraps
    `AXIsProcessTrustedWithOptions(NULL)`.
  - `pub fn request_accessibility_permission()` — calls
    `AXIsProcessTrustedWithOptions` with `kAXTrustedCheckOptionPrompt: true`
    to surface the macOS permission dialog.
  - `pub fn poll_active_window() -> Option<ActiveWindowContext>` — uses
    `NSWorkspace.sharedWorkspace().frontmostApplication()` for `app_name`,
    then `AXUIElementCreateApplication(pid)` +
    `AXUIElementCopyAttributeValue(kAXFocusedWindowAttribute)` +
    `AXUIElementCopyAttributeValue(kAXTitleAttribute)` for `document_title`.
    Returns `None` if permission is not granted, app is Jeff itself, or
    AX call fails.
  - All macOS API calls gated on `#[cfg(target_os = "macos")]`.
  - Non-macOS stub: `is_accessibility_trusted() -> false`,
    `poll_active_window() -> None`.
- `desktop/src-tauri/src/lib.rs` — add `pub mod context_observer`.

**Exit criteria for this milestone:**
- `cargo build` compiles without error on macOS.
- `poll_active_window()` returns `None` rather than panicking when
  accessibility permission is not yet granted.
- `is_accessibility_trusted()` returns the correct system state.

---

### M20.2 — ContextState + Polling Task + Tauri Commands
`[x]`

**What this does:** Manages the active context state in memory, starts
the 3-second polling loop, and exposes it via Tauri commands.

**Files touched:**
- `desktop/src-tauri/src/state.rs` — add `ContextState`:
  ```rust
  pub struct ContextState {
      inner: Mutex<ContextStateInner>,
  }
  struct ContextStateInner {
      current: Option<ActiveWindowContext>,
      nudged_titles: HashSet<String>,
  }
  impl ContextState {
      pub fn new() -> Self { ... }
      pub fn update(&self, ctx: Option<ActiveWindowContext>) { ... }
      pub fn current(&self) -> Option<ActiveWindowContext> { ... }
      pub fn should_nudge(&self, title: &str) -> bool { ... }
      pub fn should_nudge_for_switch(&self, title: &str) -> bool { ... }
      pub fn mark_nudged(&self, title: String) { ... }
  }
  ```
  `ActiveWindowContext` is imported from `context_observer`.
  `ContextState` is NOT persisted to SQLite.
- `desktop/src-tauri/src/main.rs` — in the `setup` hook:
  - Manage `ContextState::new()` via `app.manage`.
  - Spawn a Tokio task that loops every 3 seconds:
    reads `AmbientState.is_quiet_mode()`, and if not quiet, calls
    `context_observer::poll_active_window()` and updates `ContextState`.
    Task holds a clone of the `AppHandle` to access both states.
  - Polling task is fire-and-forget for now (cancelled when process exits).
- `desktop/src-tauri/src/commands.rs` — three new commands:
  - `get_active_window_context(state: State<ContextState>) -> Option<ActiveWindowContextDto>`
  - `get_accessibility_permission_status() -> bool`
  - `request_accessibility_permission()` — calls
    `context_observer::request_accessibility_permission()`
- `desktop/src-tauri/src/main.rs` — register the three new commands in
  `invoke_handler`.

**Exit criteria for this milestone:**
- With accessibility permission granted and a document open, polling
  loop updates `ContextState` within 5 seconds.
- `get_active_window_context` returns null when permission is not granted.
- Polling stops producing updates when quiet mode is enabled (context
  freezes at last known value; does not error).

---

### M20.3 — Companion Header Showing Active Context
`[x]`

**What this does:** Frontend shows the active app and document title in
the companion/overlay header. Renders nothing when context is absent.

**Files touched:**
- `desktop/src/Overlay.tsx` — add a 3-second polling effect using
  `invoke("get_active_window_context")`. Store result in local state
  `activeContext: ActiveWindowContextDto | null`.
  In the collapsed bar header: when `activeContext` is non-null and
  `document_title` is non-empty, render one line below the task label:
  `"{app_name} — {document_title}"` in a muted style (no bold,
  smaller font). When null: render nothing (no empty div, no placeholder).
- `desktop/src/App.tsx` — same polling and rendering pattern for the
  companion view context header (the header area above the message list).

**Exit criteria for this milestone:**
- With permission granted and Finder window focused, overlay collapsed
  bar shows "Finder — [folder name]" within 5 seconds.
- With permission not granted, header is absent with no visible change
  from pre-Phase 20 layout.
- No layout shift occurs when context appears or disappears.

---

### M20.4 — LLM System Prompt Injection
`[x]`

**What this does:** All three LLM call paths (chat, reorientation,
drift) receive the active window context as a prefix line in the
system prompt when context is available. Under 30 tokens.

**Files touched:**
- `desktop/src-tauri/src/chat.rs` — in `send_message` and
  `send_message_streaming`, before building the `messages` vec, call
  `context_observer::poll_active_window()` (or read from `ContextState`
  if preferred to avoid a redundant syscall). If non-None, prepend to
  the system prompt:
  `"User's active app: {app_name}. Document: {document_title}."`.
  Do nothing if None.
- `desktop/src-tauri/src/proactive.rs` — same injection in
  `generate_reorientation` and `evaluate_drift` system prompts.
- `ContextState` should be threaded into these call sites via
  `JeffState` or passed as a separate `State` parameter on the command.
  Prefer adding `context_state: State<'_, ContextState>` to the relevant
  commands in `commands.rs` so `chat.rs` can receive it.

**Exit criteria for this milestone:**
- With permission granted and a document open, asking Jeff "what am I
  working on?" returns a response that references the frontmost document
  title (even without a workspace folder configured).
- With permission not granted, LLM calls are identical to pre-Phase 20.

---

### M20.5 — Document-Switch Nudge
`[x]`

**What this does:** When the frontmost document title changes to
something that does not match the active task, Jeff surfaces one soft
prompt. One prompt per unique document title, never repeated.

**Files touched:**
- `desktop/src-tauri/src/context_observer.rs` — no change (nudge logic
  belongs in the polling task, not in the observer).
- `desktop/src-tauri/src/main.rs` — polling task (from M20.2): after
  updating `ContextState`, if the new `document_title` differs from the
  active task's title AND `ContextState.should_nudge(new_title)` is true,
  emit a Tauri event `context://document-switch` with payload
  `{ app_name, document_title }` to all windows. Call
  `ContextState.mark_nudged(new_title)` after emitting so the nudge cannot
  fire again for the same title. First observation is not treated as a
  switch.
  Emit only when `is_accessibility_trusted()` is true.
- `desktop/src/Overlay.tsx` — subscribe to `context://document-switch`.
  On event: show a dismissible inline banner (same style as the drift
  notice banner, 8-second auto-dismiss):
  `"You switched to [document_title]. Want to start or switch tasks?"`
  with start/switch task CTAs. If the user dismisses, do nothing — the
  emitted nudge is already marked in backend state.
- `desktop/src/App.tsx` — same subscription and banner in the companion
  view.

**Exit criteria for this milestone:**
- Switching from one document to another fires the banner exactly once.
- Switching back to the same document does not fire a second banner.
- Closing the banner does not cause another banner to appear for the
  same document during that session.
- With permission not granted, no nudge fires.

---

### M20.6 — `scripts/phase20_check.sh`
`[x]`

**What this does:** Writes the behavioral check script.

**Verifications:**
1. `grep -r "context_observer"` finds the module in `src/lib.rs`.
2. `grep "AXIsProcessTrustedWithOptions\|poll_active_window"` in
   `context_observer.rs`.
3. `grep "ContextState"` exists in `state.rs`.
4. `grep "get_active_window_context\|request_accessibility_permission"` in
   `commands.rs`.
5. `grep "context://document-switch"` exists in `Overlay.tsx` and `App.tsx`.
6. `grep "get_active_window_context" Overlay.tsx App.tsx` — both files
   use the command.
7. Context injection grep: `grep "active app\|document_title" chat.rs` and
   `grep "active app\|document_title" proactive.rs`.
8. SQLite safety: `grep -r "INSERT.*context\|context.*INSERT" store.rs`
   returns nothing (context is never written to SQLite).
9. Behavioral: `cargo test --bin jeff-desktop context_state` for
   `should_nudge`, `should_nudge_for_switch`, and `mark_nudged` unit tests.
10. Frontend regression: `npm run test`.

**Files touched:**
- `scripts/phase20_check.sh` (new file)

---

## Milestone dependency order

```
Phase 19                  Phase 20
--------                  --------
M19.1 (SMAppService)      M20.1 (context_observer module)
  |                         |
M19.2 (tray toggle)       M20.2 (ContextState + polling + commands)
  |                         |
M19.3 (state persistence) M20.3 (companion header)
  |                         |
M19.4 (restore_session)   M20.4 (LLM injection)
  |                         |
M19.5 (check script)      M20.5 (switch nudge)
                            |
                          M20.6 (check script)
```

M19.4 and M20.2 both touch `main.rs` setup. Merge Phase 19 changes
first to avoid conflicts.

---

## Exit criteria summary (per PHASES_NEXT.md)

### Phase 19 done when all of the following pass:
- Enabling the toggle appears in macOS Login Items; Jeff launches on next login.
- On relaunch after a prior session, active task and overlay state are
  restored within 2 seconds without any window stealing focus.
- Disabling the toggle removes the login item, confirmed by SMAppService state.
- `phase19_check.sh` passes all grep and behavioral assertions.

### Phase 20 done when all of the following pass:
- With permission granted and a document open, companion header shows
  app and title within 5 seconds of document open.
- LLM response to "what am I working on?" correctly references the
  frontmost document without any user paste.
- Document-switch nudge fires once per switch and does not repeat for
  the same document.
- With permission not granted, app runs identically to pre-Phase 20
  with no visible change.
- `phase20_check.sh` passes all grep and behavioral assertions.
