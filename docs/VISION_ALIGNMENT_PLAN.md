# Jeff — Vision Alignment Plan

This document is a full audit of every gap between the current implementation and
the product vision, with a concrete checklist for closing each one. Work one section
at a time. Do not mark a section complete until every item in it is checked.

**Priority order:**
A → B → C → D → E → F → G → H

Rationale: A (single window) is load-bearing for everything else — every other
improvement is undermined by the two-window design. B (visual redesign) is done
right after structural work so subsequent UI changes build on the correct foundation.
C–G are vision-gap closures. H (testing guide) is the verification framework.

---

## A. Single Window Architecture

**The problem:** `main.tsx` branches on `isOverlayWindow()` to render either
`Overlay.tsx` (companion bar) or `App.tsx` (full workspace) in two separate OS
windows. The companion's "open full workspace" button opens a second OS window.
`SINGLE_WINDOW_PLAN.md` acknowledges this fights the vision and has the full
implementation spec. It is not yet implemented.

**What done looks like:** One OS window at all times. The overlay is the window.
Pressing "open full workspace" resizes and restores the same window. State is
consistent because everything reads from SQLite on mount. No cross-window event
synchronization needed.

### A1 — Backend: remove main window, add workspace-mode command
- [x] In `src-tauri/tauri.conf.json`: remove the `"main"` window entry from
  `app.windows`. The overlay window is created dynamically by
  `ambient::build_overlay_window` at startup.
- [x] In `src-tauri/src/ambient.rs`: add `Workspace` variant to the `OverlayMode`
  enum alongside `Collapsed` and `Expanded`.
- [x] In `src-tauri/src/ambient.rs`: add constants
  `OVERLAY_WORKSPACE_WIDTH = 960.0` and `OVERLAY_WORKSPACE_HEIGHT = 700.0`.
- [x] In `src-tauri/src/ambient.rs`: extend `resize_overlay_for_mode` to handle
  `OverlayMode::Workspace` — resize to 960×700, call `set_always_on_top(false)`,
  center on screen. (decorations kept frameless per plan fallback note)
- [x] In `src-tauri/src/ambient.rs`: handle the inverse transition (workspace back
  to `Collapsed`/`Expanded`) — call `set_always_on_top(true)`,
  reposition to top-right corner.
- [x] In `src-tauri/src/ambient.rs`: add new pub command
  `ambient_set_workspace_mode(open: bool, app)`.
- [x] In `src-tauri/src/ambient.rs`: replace all calls to `show_workspace(app)` and
  `hide_workspace(app)`. Updated `open_privacy_center` and `"tray:workspace"` handler.
- [x] In `src-tauri/src/ambient.rs`: remove functions `show_workspace`,
  `hide_workspace`, `ambient_show_workspace`, `ambient_hide_workspace`.
- [x] In `src-tauri/src/ambient.rs`: remove constant `MAIN_WINDOW_LABEL`.
- [x] In `src-tauri/src/commands.rs`: remove the `main_visible` check from
  `should_notify_when_backgrounded()`. Only `overlay_visible` matters.
- [x] In `src-tauri/src/chat_streaming.rs`: same — remove `main_visible`.
- [x] In `src-tauri/src/main.rs`: remove the block that hides the main window.
- [x] In `src-tauri/src/main.rs`: remove `ambient_show_workspace` and
  `ambient_hide_workspace` from `invoke_handler!`.
- [x] In `src-tauri/src/main.rs`: add `ambient_set_workspace_mode` to
  `invoke_handler!`.
- [x] Run `cargo check` — clean (20.65s, zero errors).
- [ ] Run `cargo build` — confirm clean build (deferred to B section end).

### A2 — Frontend: single render root
- [x] In `src/main.tsx`: remove the `isOverlayWindow()` branch. Always renders `<Root />`.
- [x] In `src/main.tsx`: keep `document.body.classList.add("overlay-body")`.
- [x] Created `src/Root.tsx` — owns `workspaceOpen` state, passes `openWorkspace`/
  `closeWorkspace` callbacks, syncs with `ambient://state-changed`.

### A3 — Frontend: wire workspace open/close into existing components
- [x] In `src/ambientClient.ts`: removed `showWorkspace()` export.
- [x] In `src/ambientClient.ts`: added `setWorkspaceMode(open: boolean)`.
- [x] In `src/ambientClient.ts`: removed `isOverlayWindow()` export.
- [x] In `src/ambientClient.ts`: added `"workspace"` to the `OverlayMode` union type.
- [x] In `src/Overlay.tsx`: added `onOpenWorkspace: () => void` prop.
- [x] In `src/Overlay.tsx`: `handleOpenWorkspace` calls `props.onOpenWorkspace()`.
- [x] In `src/App.tsx`: added `onCloseWorkspace?: () => void` prop.
- [x] In `src/App.tsx` header: "back to companion" button when prop present.
- [x] No calls to `hideWorkspace`/`ambientHideWorkspace` remain.

### A4 — Tray menu update
- [x] In `src-tauri/src/ambient.rs` tray handler: `"tray:workspace"` calls
  `set_workspace_mode(app, true)`.

### A5 — Cleanup and verification
- [x] `showWorkspace` removed from `ambientClient.ts`.
- [x] `isOverlayWindow()` removed from `ambientClient.ts`.
- [x] No `#overlay`-hash-based routing remains.
- [x] `npx tsc --noEmit` — zero errors.
- [x] `npx vitest run` — 30/30 tests pass (fixed test mocks and missing props).
- [ ] Manual smoke test:
  - [ ] Companion bar appears at top-right on app launch.
  - [ ] Click "open full workspace" → same window resizes to 960×700, `App.tsx` renders.
  - [ ] Click "back to companion" → window returns to 420×520, `Overlay.tsx` renders.
  - [ ] Task state consistent — both components read from DB on mount.
  - [ ] Tray "Open Full Workspace" triggers the same resize.
  - [ ] Hotkey and collapsed/expanded mode still work in companion.
  - [ ] Closing the window hides it; Quit only via tray.

---

## B. Visual Redesign — Dark Ambient Aesthetic

**The problem:** The current visual language is a generic light-grey web app
(white backgrounds, blue borders, green accents). It communicates "productivity
tool." The vision is Jarvis — ambient, present, cool. The overlay looks like a
browser extension panel. The workspace looks like a CRUD app.

**What done looks like:** Dark near-black base. Deep purple accent for active
states, streaming, and Jeff's voice. Subtle blur / translucency on the overlay.
Clean monospace or geometric sans type. Status indicators that pulse softly
rather than static dots. The whole surface feels like it belongs alongside your
work rather than on top of it.

### B1 — Design tokens (CSS custom properties)
- [x] In `src/styles.css`: define the full design token set under `:root`:
  ```css
  --bg-base: #0a0a0f;
  --bg-surface: #111118;
  --bg-elevated: #1a1a24;
  --bg-input: #16161f;
  --border-subtle: rgba(255,255,255,0.07);
  --border-default: rgba(255,255,255,0.11);
  --border-active: rgba(139,92,246,0.5);
  --accent-purple: #8b5cf6;
  --accent-purple-dim: rgba(139,92,246,0.18);
  --accent-purple-glow: rgba(139,92,246,0.35);
  --accent-blue: #3b82f6;
  --text-primary: rgba(255,255,255,0.92);
  --text-secondary: rgba(255,255,255,0.5);
  --text-dim: rgba(255,255,255,0.28);
  --status-idle: rgba(255,255,255,0.22);
  --status-listening: #3b82f6;
  --status-working: #8b5cf6;
  --danger: #f87171;
  --success: #34d399;
  --warn: #fbbf24;
  --radius-sm: 6px;
  --radius-md: 10px;
  --radius-lg: 16px;
  --shadow-overlay: 0 24px 60px rgba(0,0,0,0.7), 0 0 0 1px rgba(255,255,255,0.06);
  --font-sans: "Inter", "SF Pro Display", system-ui, sans-serif;
  --font-mono: "SF Mono", "Fira Code", "JetBrains Mono", monospace;
  ```
- [x] Add Inter font import at the top of `styles.css` via Google Fonts or local
  reference (check what is available in the Tauri webview):
  `@import url('https://fonts.googleapis.com/css2?family=Inter:wght@300;400;500;600&display=swap');`

### B2 — Overlay (companion bar): full dark restyle
- [x] `.overlay-root`: dark translucent background using `backdrop-filter: blur(24px)`.
  Background `rgba(10,10,18,0.92)`. Border `1px solid var(--border-subtle)`.
  Box shadow `var(--shadow-overlay)`. Border-radius `var(--radius-lg)`.
  Font family `var(--font-sans)`. Color `var(--text-primary)`.
- [x] `.overlay-collapsed`: reduce padding, keep height compact. Background slightly
  more transparent so it almost disappears into the screen corner.
- [x] `.overlay-header`: no background change (inherits overlay). Text
  `var(--text-secondary)` for labels. Controls smaller, ghost style.
- [x] `.overlay-status-dot.overlay-status-idle`: color `var(--status-idle)`.
  No animation.
- [x] `.overlay-status-dot.overlay-status-listening`: color `var(--status-listening)`.
  Add `animation: pulse-blue 1.5s ease-in-out infinite` — a gentle scale pulse.
- [x] `.overlay-status-dot.overlay-status-working`: color `var(--status-working)`.
  Add `animation: pulse-purple 1s ease-in-out infinite` — slightly faster pulse.
- [x] Add keyframes: `@keyframes pulse-blue` (scale 1 → 1.3 → 1, subtle glow).
  `@keyframes pulse-purple` (same with purple glow). Use `box-shadow` for glow.
- [x] `.overlay-controls button`: ghost style — no border, no background, color
  `var(--text-dim)`. Hover: color `var(--text-secondary)`. Transition `0.15s`.
- [x] `.overlay-quiet-on`: tinted background `rgba(251,191,36,0.12)`, text `var(--warn)`.
- [x] `.overlay-messages`: scrollbar styled dark (webkit-scrollbar background dark,
  thumb `rgba(255,255,255,0.12)`).
- [x] `.overlay-message`: background `var(--bg-elevated)`, border `var(--border-subtle)`.
  Border-radius `var(--radius-md)`.
- [x] `.overlay-message-user`: border-left `2px solid var(--accent-blue)`.
  Background `rgba(59,130,246,0.06)`.
- [x] `.overlay-message-assistant` (Jeff's messages): border-left
  `2px solid var(--accent-purple)`. Background `rgba(139,92,246,0.06)`.
- [x] `.overlay-message-role`: color `var(--text-dim)`. Font `var(--font-mono)`.
  Font-size `10px`. Letter-spacing `0.08em`.
- [x] `.overlay-input`: background `var(--bg-input)`. Border `1px solid var(--border-default)`.
  Color `var(--text-primary)`. Border-radius `var(--radius-sm)`.
  Focus: border-color `var(--border-active)`, box-shadow
  `0 0 0 3px var(--accent-purple-dim)`. Transition `0.15s`.
- [x] `.overlay-send`: background `var(--accent-purple)`. Color `#fff`. Border `none`.
  Border-radius `var(--radius-sm)`. Hover: background slightly lighter.
  Transition `0.15s`.
- [x] `.overlay-mic`: background `var(--bg-elevated)`. Border `var(--border-default)`.
  Color `var(--text-secondary)`. Border-radius `var(--radius-sm)`.
- [x] `.overlay-mic-active`: border-color `var(--danger)`. Background
  `rgba(248,113,113,0.1)`. Color `var(--danger)`. Pulse animation — same
  scale pulse pattern.
- [x] `.overlay-banner-info`: background `rgba(59,130,246,0.1)`. Border
  `1px solid rgba(59,130,246,0.22)`. Color `rgba(147,197,253,1)`.
  Border-radius `var(--radius-sm)`.
- [x] `.overlay-banner-warn`: background `rgba(251,191,36,0.1)`. Border
  `1px solid rgba(251,191,36,0.22)`. Color `var(--warn)`.
- [x] `.overlay-error`: color `var(--danger)`. Font-size `12px`.
- [x] `.overlay-task-label`: color `var(--text-primary)`. Font-weight `500`.
- [x] `.overlay-workspace-link`: removed by section F. The companion no longer
  exposes a visible "open full workspace" button.
- [x] `.overlay-watcher-line`: color `var(--text-dim)`. Font `var(--font-mono)`.
  Font-size `10px`.
- [x] `.overlay-watcher-indexed`: color `var(--success)`.
- [x] `.overlay-context-line`: color `var(--text-dim)`. Font-size `11px`.
  Border-left `2px solid var(--border-subtle)`. Padding-left `6px`.
- [x] `.overlay-hotkey-hint`: font `var(--font-mono)`. Color `var(--text-dim)`.
  Font-size `11px`.
- [x] `.overlay-onboarding`: background `var(--bg-elevated)`. Border
  `var(--border-default)`. Border-radius `var(--radius-md)`.
- [x] `.overlay-onboarding-step h3`: color `var(--text-primary)`. Font-weight `600`.
- [x] `.overlay-onboarding-step p`: color `var(--text-secondary)`.
- [x] Onboarding continue button: use `var(--accent-purple)` as primary fill.
- [x] `.overlay-collapsed-summon`: color `var(--text-primary)`. Font-weight `500`.
- [x] `.overlay-context-hint` (collapsed bar context): color `var(--text-dim)`.
  Font-size `10px`. Font `var(--font-mono)`.
- [x] The collapsed bar should feel like a whisper — very subtle, no border visible
  unless on hover. Background `rgba(10,10,18,0.75)`. On hover: border
  `var(--border-subtle)` fades in. Transition `0.2s`.

### B3 — Workspace (App.tsx) view: dark restyle
- [x] `.shell`: background `var(--bg-base)`. Color `var(--text-primary)`.
  Font `var(--font-sans)`.
- [x] `.panel`: background `var(--bg-surface)`. Border `var(--border-default)`.
  Border-radius `var(--radius-md)`.
- [x] `h1, h2, h3`: color `var(--text-primary)`. Font-weight `500`.
- [x] `p, .subtitle`: color `var(--text-secondary)`.
- [x] `input, select, textarea`: background `var(--bg-input)`. Border
  `var(--border-default)`. Color `var(--text-primary)`. Border-radius
  `var(--radius-sm)`. Focus: same purple focus ring as overlay input.
- [x] `button` (default): background `var(--bg-elevated)`. Border
  `var(--border-default)`. Color `var(--text-secondary)`. Border-radius
  `var(--radius-sm)`. Hover: border `var(--border-active)`, color
  `var(--text-primary)`. Transition `0.15s`.
- [x] `button` primary variant (add `.btn-primary` class in JSX where needed):
  background `var(--accent-purple)`. Color `#fff`. Border `none`.
- [x] `.chat-bubble`: background `var(--bg-elevated)`. Border `var(--border-subtle)`.
- [x] `.chat-user`: border-left `2px solid var(--accent-blue)`. Background
  `rgba(59,130,246,0.05)`.
- [x] `.chat-assistant`: border-left `2px solid var(--accent-purple)`. Background
  `rgba(139,92,246,0.05)`.
- [x] `.chat-role`: color `var(--text-dim)`. Font `var(--font-mono)`. Font-size `10px`.
- [x] `.active-pill`: background `rgba(139,92,246,0.15)`. Color `var(--accent-purple)`.
  Border `1px solid var(--accent-purple-dim)`. Border-radius `999px`.
- [x] `.revision-item, .subtask-item, .version-item`: background `var(--bg-elevated)`.
  Border `var(--border-subtle)`.
- [x] `.subtask-suggestion`: background `rgba(139,92,246,0.07)`. Border
  `1px solid var(--accent-purple-dim)`.
- [x] `.companion-header`: background `rgba(139,92,246,0.08)`. Border-color
  `var(--accent-purple-dim)`.
- [x] `.companion-card`: background `var(--bg-elevated)`. Border `var(--border-subtle)`.
- [x] `.privacy-center-panel`: background `var(--bg-elevated)`. Border
  `var(--border-default)`.
- [x] `pre, code`: background `rgba(0,0,0,0.3)`. Border `var(--border-subtle)`.
  Color `rgba(167,139,250,1)` (purple-tinted mono text). Font `var(--font-mono)`.
- [x] `.artifact-editor`: background `var(--bg-input)`. Border `var(--border-default)`.
  Color `var(--text-primary)`. Font `var(--font-mono)`.
- [x] `scrollbars` globally: webkit-scrollbar width `4px`. Track background
  `transparent`. Thumb `rgba(255,255,255,0.1)`. Hover thumb
  `rgba(255,255,255,0.18)`.

### B4 — Status indicator upgrade
- [x] In `Overlay.tsx`: replace the plain status dot `<span>` with a small
  component that renders the dot plus a breathing ring when active. The breathing
  ring is a `::after` pseudo-element with `animation: ring-breathe` — a radial
  scale that fades out. This is the primary "Jeff is alive" signal.
- [x] When `status === "working"`: the status label in the header reads just "jeff"
  (not "Jeff · working") with the purple pulsing dot. The dot is the signal;
  words are noise.
- [x] When `status === "idle"`: label reads "jeff" with dim dot.
- [x] When `status === "listening"`: label reads "jeff" with blue pulse.
- [x] Streaming text rendering: add a cursor blink at the end of the streaming
  content using `::after` pseudo-element with `animation: blink 0.7s step-end infinite`.
  This is the "Jeff is writing" signal inside the message bubble.

### B5 — Typography and spacing tightening
- [x] Set `--font-sans` as the body font in `:root`. Remove Segoe UI, Tahoma,
  Verdana from the font stack.
- [x] Line heights: body text `1.5`, monospace text `1.4`, headers `1.2`.
- [x] Remove all `h1` / `h2` / `h3` margin overrides set to `0` — replace with
  deliberate spacing using gap in parent grid containers.
- [x] The overlay font size stays `13px` for density. Workspace can use `14px` base.
- [x] All uppercase labels (`.chat-role`, `.overlay-message-role`) use
  `font-variant: small-caps` instead of `text-transform: uppercase` for a cleaner
  typographic feel.

### B6 — Verify visual coherence
- [ ] Launch the app in dev mode. The companion bar is dark, nearly invisible in the
  corner. The status dot pulses purple when Jeff responds.
- [ ] Type a message. Jeff's response appears with purple left-border and the cursor
  blinks at the end while streaming.
- [ ] Open full workspace. Dark background, all panels consistent, no white anywhere.
- [ ] Voice recording active: mic button glows red with pulse animation.
- [ ] Quiet mode on: the quiet indicator turns amber.
- [ ] Screenshots of each state to confirm aesthetic coherence before moving on.

---

## C. Proactive System — Real Autonomy

**The problem:** `trigger_task_resume` is called from `ambient://overlay-shown`
only when `event.payload?.interactive === true`. This means Jeff orients you only
when you explicitly summon the overlay with the hotkey. Drift detection runs only
after a message is sent. There is no autonomous monitoring loop that runs in the
background and decides to surface something without user initiative.

**What done looks like:** A background Rust task polls every 60 seconds. It checks
context observer state, task focus timestamps, and recent chat activity. When
conditions are met — user has been away 5+ minutes, or drift is detected from
content changes, or user has been silent 10+ minutes on a task — Jeff fires a
native notification or, if the overlay is visible, surfaces a banner. Jeff speaks
first, sometimes. The user never has to open Jeff to be oriented.

### C1 — Background autonomous monitor loop in Rust
- [x] In `src-tauri/src/main.rs`: add a background Tokio task that spawns at startup
  and runs every 60 seconds. Call it `spawn_ambient_monitor`. It holds a reference
  to `AppState`, `AmbientState`, and the `AppHandle`.
- [x] The monitor loop body executes the following checks in sequence:
  1. `check_reorientation_from_background`
  2. `check_stuck_from_background`
  3. `check_stale_task_notifications` (already exists in `workload.rs`)
- [x] `check_reorientation_from_background`: reads the active task from state.
  Reads `task_focus_log` to find the last focus timestamp. If `unix_now() - last_focus > REORIENTATION_MIN_ABSENCE_SECONDS (300)` AND context_observer shows
  the user's frontmost app has been active for the last 60 seconds (user is working,
  not idle), AND the reorientation cooldown has not fired in the last 300 seconds,
  call `generate_reorientation` and dispatch as a native notification
  (if overlay is hidden) or emit `proactive://reorientation` event to the frontend
  (if overlay is visible). Record the trigger in `proactive_trigger_log`.
- [x] `check_stuck_from_background`: if the active task has chat history but no new
  message in the last `STUCK_SILENCE_THRESHOLD_SECONDS (600)` seconds, and
  `context_observer` shows the user is actively in a relevant app, and
  `STUCK_COOLDOWN_SECONDS (1200)` has passed since last stuck trigger, call
  `propose_speculative_subtask` and dispatch the result.
- [x] The monitor must check `AmbientState.is_quiet_mode()` before any dispatch.
  If quiet mode is on, the monitor still runs its checks (to maintain cooldown
  state) but suppresses all output.
- [x] The monitor must not block the main thread. All LLM calls inside the monitor
  use `spawn_blocking` or are already async.
- [x] Add a test flag: `JEFF_DISABLE_AMBIENT_MONITOR=1` env var that skips the
  monitor loop. Used in CI to avoid flaky LLM calls.

### C2 — Drift detection from background (not only post-message)
- [x] In the ambient monitor loop: add `check_drift_from_background`. This runs
  after reorientation check.
- [x] `check_drift_from_background`: calls `evaluate_drift` with the current
  workspace context (recently indexed file content) against the active task's
  stated goal. Only runs if: the active task has a goal set, the watcher has
  indexed at least one file in the last 10 minutes (content is fresh), and
  `DRIFT_COOLDOWN_SECONDS (900)` has passed since last drift trigger.
- [x] If drift is detected, emit `proactive://drift` event to frontend (if overlay
  visible) or fire a native notification (if overlay hidden). Use the existing
  `dispatch_notification` path.
- [x] The drift check should NOT run if there is an active streaming LLM turn (Jeff
  is already responding). Add an `is_streaming_active()` check to `AmbientState`
  or read from the streaming registry.

### C3 — Frontend: handle autonomous proactive events
- [x] In `src/Overlay.tsx`: subscribe to `proactive://reorientation` event. On
  receive, show the reorientation banner (same as current interactive path). The
  banner shows regardless of whether the user opened the overlay — if the overlay
  is hidden and the event fires, expand the overlay first.
- [x] In `src/Overlay.tsx`: subscribe to `proactive://drift` event. Show the drift
  banner inline in the message list. Do not expand the overlay for drift alone —
  if the overlay is collapsed, show a pulsing indicator on the collapsed bar
  that there is something to see when expanded.
- [x] In `src/Overlay.tsx`: subscribe to `proactive://speculative_subtask` event.
  Show an offer card: "Started [subtask title]. Want to keep it?" with Accept and
  Dismiss buttons. This card appears in the message stream.
- [x] Remove the existing call to `triggerTaskResume` inside `ambient://overlay-shown`.
  The autonomous monitor handles reorientation now. The overlay-shown path should
  only call `recordTaskFocus`. This prevents double-firing when the user opens the
  overlay right after the monitor fires.
- [x] The `taskFocus` timestamp update (`recordTaskFocus`) still runs on
  `ambient://overlay-shown` interactive. This is correct — it feeds the monitor's
  cooldown logic.

### C4 — Proactive notification content quality
- [x] Audit the `REORIENTATION_SYSTEM_PROMPT` in `proactive.rs`. Current version:
  "Write one short sentence (max 25 words) summarizing where they left off. Be
  specific to the content. No commands. No filler phrases." Keep this but add:
  "Sound like a coworker who has been watching, not a system status message. First
  person. Direct."
- [x] Native notification title should be "jeff" (lowercase, no colon). Body should
  be the reorientation sentence. Sound: default system notification sound (no
  custom sound in v1).
- [x] When Jeff fires a proactive notification and the user clicks it, the overlay
  expands and shows the full reorientation context, not just "opened from
  notification." Remove the generic `"opened from notification · [kind]"` banner
  and replace with the actual proactive content card.

### C5 — Verify autonomous proactive behavior
- [ ] Unit test: `check_reorientation_from_background` with a mock task focus
  timestamp 10 minutes ago returns a reorientation trigger. With a timestamp
  2 minutes ago, returns no trigger.
- [ ] Unit test: `check_reorientation_from_background` with quiet mode on
  returns no dispatch (but records the would-have-fired state).
- [ ] Manual test: set active task, interact, then leave Jeff alone for 6 minutes.
  Without opening the overlay, receive a native notification orienting you to where
  you left off. Click it — overlay expands and shows the context.
- [ ] Manual test: quiet mode on, wait 6 minutes — no notification fires.

---

## D. Voice — Natural Interruption

**The problem:** Voice is push-to-talk only. To interrupt Jeff while it is speaking,
the sequence is: (1) click mic button, (2) Jeff stops, (3) speak, (4) click stop.
That is four interactions. The vision is "mid-sentence, either direction, not via
button presses." Jeff also cannot interrupt the user because it is not listening
unless the user has already clicked record.

**What done looks like:** While Jeff is speaking (TTS active), any new keystroke or
new typed character in the input box stops the audio immediately — no click
required. A single keyboard shortcut (e.g. Escape or Cmd+Shift+J again) both stops
Jeff and opens the input ready to type. Voice recording itself still requires a
button click or keyboard shortcut (always-on mic is a future phase — out of scope
here), but the barge-in path from typing is zero friction.

### D1 — Keystroke barge-in while TTS is playing
- [x] In `src/Overlay.tsx`: add a `keydown` event listener on the `window` object
  that fires `stopAndBargeIn()` if: (a) `ttsActiveTurnIdRef.current !== null`
  (TTS is actively playing), AND (b) the key pressed is a printable character
  (not a modifier, arrow, or function key). After stopping TTS, focus
  `messageInputRef.current` so the character appears in the input.
- [x] The above listener must be cleaned up on unmount (return an unlisten in the
  `useEffect` that registers it).
- [x] In `src/Overlay.tsx`: when Jeff's TTS is active (`ttsActiveTurnIdRef.current
  !== null`) and the user begins typing in the input field (`onChange` fires),
  call `stopStreamingTtsPlayback()` immediately — before the character is
  submitted. This is the "start typing = Jeff stops talking" behavior.
- [x] Do not stop TTS when the user presses modifier keys (Cmd, Ctrl, Shift, Alt),
  arrow keys, Tab, or Escape. Only stop on printable character input or mic button.
- [x] Add a visual affordance: while TTS is playing, show a subtle indicator in the
  input area: "jeff is speaking — type to interrupt" with a very dim style so it
  does not distract but is discoverable. Dismiss this hint after the user
  interrupts once (persist the dismissal in memory for the session).

### D2 — Single hotkey to stop Jeff mid-response
- [x] The global hotkey (Cmd+Shift+J) currently toggles the overlay. Change its
  behavior when the overlay is already visible and expanded: if TTS is active,
  pressing the hotkey stops TTS and focuses the input (barge-in). If TTS is not
  active, pressing the hotkey hides the overlay (current behavior).
- [x] This requires a change in `src-tauri/src/ambient.rs` in the hotkey handler.
  The handler should emit a `ambient://hotkey-pressed` event with a payload of
  `{ overlay_visible: bool, tts_active: bool }`. The frontend handles the
  distinction.
- [x] In `src/Overlay.tsx`: subscribe to `ambient://hotkey-pressed`. If
  `tts_active: true` (the backend can check `ttsActiveTurnIdRef` ... actually the
  backend does not know about frontend TTS state, so instead: the frontend simply
  calls `stopAndBargeIn()` if `ttsActiveTurnIdRef.current !== null` when the
  hotkey fires).

### D3 — Mic button keyboard shortcut
- [x] Add a keyboard shortcut to toggle the microphone: `Cmd+Shift+M` (or similar,
  configurable). This allows voice recording to start and stop without touching
  the mouse. Wire into the existing `handleStartVoiceRecording` /
  `handleStopVoiceRecording` path.
- [x] Register this as a second global shortcut in `ambient.rs`, emitting
  `ambient://mic-shortcut`. Listen for it in `Overlay.tsx`.

### D4 — Voice naturalness: typing delay is wired correctly
- [x] Verify `typing_activity.rs` is correctly feeding `user_is_typing: bool` into
  `AmbientState`. If not, wire it in.
- [x] Verify `voice_naturalness.rs` brevity filter is applied to all TTS output.
  Write a unit test: a string containing "Certainly," is processed → "Certainly,"
  is removed from the output. (`certainly_prefix_is_stripped_from_tts_text`)
- [x] Verify natural interjections are prepended for responses under 15 words.
  Write a unit test: a 12-word response gets a random interjection prepended.
  (`twelve_word_response_gets_interjection`)
- [ ] Verify the TTS delay path: when `user_is_typing: true`, TTS is deferred
  up to 3 seconds. After 3 seconds of continued typing, the response is delivered
  as text-only (no audio). Confirm this end-to-end with a manual test.

### D5 — Verify interruption behavior
- [ ] Manual test: Jeff is speaking a long response. Without touching the mouse,
  start typing a new question. TTS stops before you finish the first word.
- [ ] Manual test: Jeff is speaking. Press Cmd+Shift+J. TTS stops. Input is
  focused.
- [ ] Manual test: Jeff is speaking. Click mic button. TTS stops. Recording starts.
- [ ] Manual test: User is actively typing. Jeff's response finishes generating.
  TTS does not play while the user is still typing.

---

## E. Parallel Work Visibility in the Companion

**The problem:** Subtask chain infrastructure exists and works, but all progress
is only visible in `App.tsx` (the full workspace view). The companion bar shows
no indication that Jeff is doing something in parallel. The user must open the
workspace to know what Jeff is working on, to see approval cards for file writes,
or to cancel a running subtask. This defeats the "you never stopped" promise.

**What done looks like:** A persistent, minimal status row in the companion bar
shows when a subtask is running: "working on: [title]" with a cancel button.
When a file write is proposed, an approval card appears in the companion message
stream — not a panel buried in the workspace. The workspace retains the full
step-by-step history as an audit view but is not the only way to interact with
active parallel work.

### E1 — Rust: emit companion-visible subtask events
- [x] In `src-tauri/src/subtask.rs`: when `run_subtask_chain` starts, emit a new
  event `subtask://companion-started` with payload
  `{ subtask_id, task_id, title }`. The overlay subscribes to this.
- [x] In `src-tauri/src/subtask.rs`: when a subtask chain completes (all steps done
  or cancelled), emit `subtask://companion-complete` with
  `{ subtask_id, task_id, final_status }`.
- [x] In `src-tauri/src/subtask.rs`: when a file write proposal is created
  (`status=pending_approval`), emit `subtask://companion-write-proposal` with the
  full `FileWriteProposalDto`. This is in addition to the existing workspace
  polling path.

### E2 — Overlay: running subtask indicator
- [x] In `src/Overlay.tsx`: add state `activeSubtask: { id: number; title: string } | null`.
  Subscribe to `subtask://companion-started` to set it.
  Subscribe to `subtask://companion-complete` to clear it.
- [x] In `src/Overlay.tsx`: render a running subtask indicator row below the
  watcher line when `activeSubtask !== null`. The row shows:
  `jeff is working on: [title]` with a `cancel` button.
  Style: small, dim text (`var(--text-dim)`), with a tiny purple spinner on the
  left (CSS border-radius spin animation, 12px, `var(--accent-purple)`).
- [x] The cancel button calls `cancelSubtask(activeSubtask.id)` from `tauriClient`.
  On success, clear `activeSubtask`.
- [x] On mount, call `listSubtasks(activeTask.id)` and filter for
  `status === "running"` — restore the active subtask indicator if one is
  already running when the overlay opens.

### E3 — Overlay: file write approval cards in companion
- [x] In `src/Overlay.tsx`: add state `pendingWriteProposals: FileWriteProposalDto[]`.
  Subscribe to `subtask://companion-write-proposal` to push new proposals.
- [x] On mount and on task switch, call `listFileWriteProposals(taskId)` and filter
  for `status === "pending_approval"` — restore any proposals waiting for review.
- [x] Render approval cards inside `overlay-messages` list (above the input row).
  Each card shows:
  - File path (relative, just the filename unless path is meaningful)
  - A small before/after diff excerpt (first 80 chars of proposed content)
  - `approve` button (purple filled) and `reject` button (ghost)
- [x] `approve` calls `approveSubtaskFileWrite(proposalId)`. On success, remove
  from `pendingWriteProposals` and show a one-line confirmation inline:
  `[filename] written`.
- [x] `reject` calls `rejectSubtaskFileWrite(proposalId)`. On success, remove from
  `pendingWriteProposals`.
- [x] These cards should also appear in the workspace `App.tsx` view (they already
  do) — the companion path is additive, not replacing.

### E4 — Overlay: speculative subtask offer card
- [x] When `proactive://speculative_subtask` arrives (from section C3), render an
  offer card in the companion message stream:
  - "I started [subtask description] in the background."
  - `keep it` button: accepts the speculative result (calls `acceptSubtaskResult`)
  - `dismiss` button: rejects it (calls `rejectSubtaskResult`)
- [x] Style the offer card with the purple left-border style (same as Jeff's
  messages) to visually distinguish it from user messages.

### E5 — Verify parallel work visibility
- [ ] Manual test: say "draft the intro section while I work on the conclusion."
  Without opening the workspace, see the subtask indicator appear in the companion
  bar. Keep interacting in the companion. When the subtask completes, see the
  offer card appear. Accept or reject — no workspace visit required.
- [ ] Manual test: subtask proposes a file write. The approval card appears in the
  companion. Approve from the companion. Verify the file was written.
- [ ] Manual test: running subtask + cancel button in companion. Verify cancellation
  stops the chain.
- [x] Automated regression coverage added in `src/Overlay.test.tsx` for restoring
  running subtasks, event-driven subtask visibility, companion file-write approval,
  approval confirmation, and task switching.

---

## F. Remove the "Open Full Workspace" Mindset

**The problem:** The "open full workspace" button is the second thing visible in the
expanded companion bar. It invites the user to leave the companion and enter a
dashboard. The vision says: "Not a workspace / app / dashboard. Not a chat interface
you visit." The workspace must exist but must not be a primary navigation target.

**What done looks like:** "Open full workspace" is removed from the companion header
and moved to the tray menu only, where it is an advanced escape hatch. The tray menu
retains it. Everything a user does in 95% of sessions — chatting, voice, approving
writes, viewing context, switching tasks — works entirely from the companion.

### F1 — Remove the workspace link from the companion header
- [x] In `src/Overlay.tsx`: remove the `overlay-workspace-link` button
  (`"open full workspace"`) from the task row in the expanded companion view.
  This is around line 1319-1329 in the current file.
- [x] The tray menu retains the "Open Full Workspace" item — that is the only entry
  point for non-technical users who never need to touch it.
- [x] The `onOpenWorkspace` prop in Overlay.tsx is still wired (for Root.tsx) but
  not exposed as a visible button.

### F2 — Task switching without the workspace
- [x] In `src/Overlay.tsx`: add a minimal task switcher to the companion.
  When the user has more than one task, show a chevron or `·` button next to the
  task label. Clicking it shows an inline dropdown of up to 5 recent tasks.
  Selecting one calls `setActiveTask(taskId)` and refreshes messages.
- [x] The task dropdown uses the `tasks` state already loaded in `Overlay.tsx`.
  Style: dark surface dropdown, `var(--bg-elevated)` background, items use
  `var(--text-secondary)` with hover `var(--text-primary)`. Active task has a
  purple dot indicator.
- [x] Creating a new task: typing into the companion input when no active task
  already creates one (this already works). No "Create Task" form in the companion.

### F3 — Audit App.tsx for developer-only panels
- [x] In `src/App.tsx`: identify all panels that exist purely for developer
  debugging (retrieval debug, flow debug, subtask debug, action center / event log,
  session mode debug). These panels should be hidden behind a `?debug=1` URL
  parameter or a dev-only toggle, not visible by default.
- [x] Specifically hide by default: `retrievalDebugChunks`, `retrievalDebugMeta`,
  `revisionDebug`, `subtaskDebug`, `flowDebug`, `recentEvents`, `sessionModeState`
  debug sections.
- [x] Keep visible in the workspace: artifacts, revisions, subtask steps, write
  audit log, privacy center, workload summary, user profile signals. These are
  genuinely useful for a non-technical user.
- [x] Add a `show debug panels` toggle at the very bottom of the workspace (small
  link text, `var(--text-dim)`). Enabling it shows all the debug sections. This
  preference persists to `localStorage` for the session.

### F4 — Companion is the complete interaction surface
- [ ] Verify that after F1-F3: a user can complete a full work session —
  chat, voice, approve writes, switch tasks, see proactive messages —
  without ever opening the workspace or the tray menu.
- [ ] Manual test: fresh session. Use only the companion bar. Never open the
  workspace. Verify that nothing essential is missing.
- [x] Automated regression coverage added for no visible workspace button in the
  companion, inline task switching, active subtask cancellation, and companion
  write approval.

---

## G. Context Without Forced Setup

**The problem:** Jeff knows your task at three tiers: (1) active window title
(automatic, works), (2) file content from a watched folder (requires explicit
folder connection), (3) what you're currently writing (requires Cmd+Shift+V).
The vision is that Jeff already knows — no setup, no context pasting.

This section does not eliminate the watched folder concept (it is genuinely useful)
but reduces the friction and makes the first-use experience require zero setup to
get useful context.

### G1 — Make active window title immediately useful
- [x] In `src-tauri/src/chat.rs` (or wherever the system prompt is assembled):
  when `active_window_context` is available (app name + document title), prepend
  it to the system prompt in a natural way:
  "The user currently has [app_name] open with [document_title]."
  This already exists per the architecture doc but verify it is live in the chat
  path, not just in the reorientation path.
- [x] In `src-tauri/src/proactive.rs`: the reorientation prompt should include
  the active window context when available. Verify this is implemented.
- [x] Test: open Pages with a document named "Research Paper Draft". Ask Jeff
  "What am I working on?" without any folder connection. Jeff should reference
  "Research Paper Draft" in its response.

### G2 — Zero-setup first message
- [x] In `src/Overlay.tsx`: when `activeTask` is null (first session), the input
  placeholder should read: "What are you working on right now?" — not "Tell me
  what you're working on" (existing text is fine but verify it is friendly).
- [x] When the user sends their first message with no active task, Jeff's response
  should be grounded in whatever active window context is available. The grounding
  system prompt should include: "The user just told you what they're working on
  for the first time. Their current document is [active_window_context if available].
  Orient yourself as a coworker who is now joining this work."
- [x] Do not require the user to "create a task" before chatting. The first message
  auto-creates a task (this already works — verify it is correct).

### G3 — Surface the "no folder connected" soft prompt clearly
- [x] In `src/Overlay.tsx`: when onboarding is complete, accessibility is granted
  (so Jeff can see the window title), but no workspace folder is connected, show
  a one-line soft prompt (not a blocking banner) after the first message: "Jeff
  can see [document_title] is open. Connect a folder to give Jeff full context."
  with a `connect folder` inline link that opens the folder picker.
- [x] This prompt fires once per session and only after the first successful message
  exchange. It does not appear during onboarding or on startup.
- [x] The folder picker flow in `Overlay.tsx` (`handleChooseWorkspaceFolder`) already
  exists — just surface an entry point here.

### G4 — Reduce the context gap for file-based work
- [x] In `src-tauri/src/watcher.rs`: when the user sets a workspace folder (via
  onboarding or the companion prompt), immediately trigger a full folder scan in
  the background without requiring any user action. This already happens on
  `start_watcher` — verify the full initial scan is implemented and runs quickly.
- [x] The `workspace://file-indexed` event already fires per file — verify the
  companion bar shows "indexed: [filename]" for each file as the initial scan
  completes. The user should see Jeff ingesting their work in real time.

### G5 — Update the testing guide context tests (see section H)
The testing guide steps that use terminal commands to create test files must be
replaced with Finder-based steps. See section H.

---

## H. Testing Guide Overhaul

**The problem:** The current `TESTING_GUIDE.md` starts with "Open a terminal" and
includes shell commands for file creation, API key manipulation, and process killing.
It is a developer acceptance test, not a user experience test. It does not test any
of the five felt properties. No test verifies that Jeff initiates anything
unprompted. No test verifies barge-in without button presses. No test verifies that
parallel work is visible without opening the workspace.

**What done looks like:** A two-part guide. Part 1 is a user experience test — no
terminal required, tests the felt properties as scenarios. Part 2 is a developer
verification appendix with the existing technical tests, labeled clearly as
developer-only.

### H1 — Remove terminal commands from user-facing steps
- [x] Replace Part 0 (Full Reset) terminal commands with instructions using:
  - System Settings for permissions
  - The "Set up Jeff again" tray menu item for re-running onboarding
  - The "Clear all Jeff data" option in the Privacy Center for full reset
  (keep a developer-only appendix with the shell commands for CI use)
- [x] Replace Part 1 Step 3 (`npm run tauri dev`) with instructions for the
  production `.dmg` install. If a dev build must be used, clearly label it
  "Developer build only — non-technical users should use the .dmg."
- [x] Replace Part 4 Step 12 (echo to create file) with: "Open Finder. Create a
  new text file inside the connected folder. Write: 'My test note: the project
  deadline is next Friday.' Save it."

### H2 — Add five felt-property scenario tests
- [x] Add a new **Part A: Already Present** test section:
  - Enable Launch at Login from tray menu.
  - Log out of macOS and log back in.
  - Expected: Jeff is in the tray within 5 seconds of login. The overlay does not
    appear unless you invoke the hotkey. No window steals focus.
  - Invoke hotkey. Expected: overlay appears in under 200ms.

- [x] Add a new **Part B: Already Knows Your Task** test section:
  - Open Pages and create a document named "My Vision Document".
  - Wait 5 seconds.
  - Open Jeff via hotkey. Expected: companion header shows "Pages — My Vision
    Document" without any action from the user.
  - Type "What am I currently working on?" Expected: Jeff references "My Vision
    Document" in its response without any folder connection.
  - (Optional, requires folder) Connect the folder containing the document. Wait
    for indexed confirmation. Ask "What's in my document?" Expected: Jeff
    references actual content from the file.

- [x] Add a new **Part C: Interruption** test section:
  - Send a long message (ask Jeff to write a 5-paragraph essay). Wait for TTS to
    start playing.
  - Without clicking anything, start typing a new message. Expected: TTS stops
    before you finish your first word.
  - Complete the message and send. Expected: Jeff responds to the new message.
  - Second test: while Jeff is speaking, press the hotkey (Cmd+Shift+J). Expected:
    TTS stops. Input is focused.

- [x] Add a new **Part D: Parallel Work** test section:
  - Say (or type): "Draft a short intro paragraph for my task while I keep
    chatting."
  - Immediately send another message: "How's my day looking?"
  - Expected: Jeff responds to the second message AND the subtask indicator appears
    in the companion bar ("working on: [intro draft]").
  - Wait for subtask to complete. Expected: an offer card appears in the companion
    without opening the workspace.
  - Approve or reject from the companion. Expected: done. Workspace was never
    opened.

- [x] Add a new **Part E: Jeff Initiates** test section:
  - Set an active task. Chat with Jeff for a minute.
  - Close the overlay and switch to another app. Work for 6 minutes.
  - Expected: A native macOS notification appears from Jeff, telling you where you
    left off on your task. No action was required from you.
  - Click the notification. Expected: overlay expands and shows the reorientation
    context.
  - Second test: quiet mode on. Wait 6 minutes. Expected: no notification.

### H3 — Update error testing to use UI-only paths
- [x] Replace Step 20 (terminal keychain manipulation) with: go to tray → "Set up
  Jeff again" → enter an invalid API key on purpose → verify error message appears.
- [x] Replace Step 21 (turn off WiFi while testing) with: same WiFi-off test but
  document it as a user action, not a terminal action.

### H4 — Add a voice naturalness test
- [x] Add a test: ask Jeff a question via voice. While Jeff is responding via TTS,
  start typing. Expected: TTS stops. Your text appears in the input.
- [x] Add a test: ask Jeff a very short question. Expected: Jeff's TTS response
  begins with a natural interjection ("got it," "here you go," etc.), not a filler
  phrase ("Certainly," "Of course,").

### H5 — Add a no-workspace-visit test
- [x] Add an explicit test step: complete a full interaction session (send 5 messages,
  trigger a subtask, approve a file write, switch tasks) without ever clicking
  "open full workspace" or using the tray to open the workspace window.
- [x] Expected: everything works. The workspace link exists in the tray but was not
  needed.

### H6 — Label the developer appendix
- [x] Move all shell-command-based tests (phase check scripts, keychain manipulation,
  process management, npm commands) to an appendix: "Developer Verification
  Appendix."
- [x] Label the appendix: "These steps require developer tools. They verify that
  features exist in the code. Run them before a release, not as part of regular
  user testing."

---

## Completion Criteria

All six sections above are complete when:

- [ ] The app launches from a `.dmg` (or dev build) and the first visible surface is
  a small, dark, barely-there companion bar in the top-right corner.
- [ ] The companion bar is dark with a pulsing purple status dot. No white anywhere.
- [ ] Jeff orients the user to their task after a 5+ minute absence, without the
  user opening the overlay.
- [ ] Starting to type while Jeff speaks stops the audio immediately.
- [x] A running subtask is visible and cancellable from the companion bar, without
  opening any other window.
- [x] The "open full workspace" button is not visible in the companion bar.
- [x] The testing guide has no terminal commands in user-facing steps.
- [x] The testing guide has a scenario for each of the five felt properties.
- [x] A non-technical user can complete a 20-minute work session — chatting,
  having Jeff run parallel work, approving a write, being proactively oriented —
  without ever opening a terminal or navigating to a separate workspace window.
