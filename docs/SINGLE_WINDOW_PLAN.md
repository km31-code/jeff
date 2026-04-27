# Single-Window Refactor Plan

## Why

The two-window design (overlay bar + full workspace as a separate OS window) requires every
state change to travel as a Tauri event between separate React trees. Task switches, new
messages, watcher events — each needs explicit emission and reception in both windows. It is
inherently leaky and fights the vision ("Not a workspace / app / dashboard. Not a chat
interface you visit.").

The fix: one Tauri window, one React tree. The overlay window becomes the only window. It
grows into "workspace mode" when the user needs the full surface, shrinks back to a companion
bar when done. The `main` window is removed entirely.

---

## Architecture After the Change

```
process (single instance)
└── overlay window  [label: "overlay", only window]
      mode=collapsed  → 420×72  px, always-on-top, frameless, top-right corner
      mode=expanded   → 420×520 px, always-on-top, frameless, top-right corner
      mode=workspace  → 960×700 px, normal window, decorations, centered
```

`main.tsx` renders `<Root />` always. `Root` holds a `workspaceOpen: boolean` state,
resizes the window via a new backend command, then mounts either `<Overlay>` (companion bar)
or `<App>` (workspace). Both components fetch their own state from SQLite on mount — no
shared React state needed because everything persists to the DB.

---

## Milestone 1 — Backend: remove main window, add workspace-mode command

### `tauri.conf.json`
Remove the `"main"` window entry from `app.windows`. The overlay window is created
dynamically by `ambient::build_overlay_window` at startup and needs no config entry.

### `ambient.rs`
- Add `Workspace` variant to the `OverlayMode` enum (alongside `Collapsed` and `Expanded`).
- Add constants: `OVERLAY_WORKSPACE_WIDTH = 960.0`, `OVERLAY_WORKSPACE_HEIGHT = 700.0`.
- Extend `resize_overlay_for_mode` to handle `OverlayMode::Workspace`: resize to `960×700`,
  call `set_always_on_top(false)`, call `set_decorations(true)`, center on screen.
- For the inverse (closing workspace back to `Collapsed`/`Expanded`): call
  `set_always_on_top(true)`, `set_decorations(false)`, reposition to top-right corner.
- Add new pub command `ambient_set_workspace_mode(open: bool, app)`:
  - `open=true` → `set_overlay_mode(Workspace)`, call resize, emit `ambient://state-changed`.
  - `open=false` → `set_overlay_mode(Expanded)`, call resize, emit `ambient://state-changed`.
- Replace `show_workspace(app)` and `hide_workspace(app)` throughout `ambient.rs` with calls
  to `ambient_set_workspace_mode`. Specifically in `open_privacy_center` and the tray
  `"tray:workspace"` handler.
- Remove functions: `show_workspace`, `hide_workspace`, `ambient_show_workspace`,
  `ambient_hide_workspace`.
- Remove constant `MAIN_WINDOW_LABEL` (keep `OVERLAY_WINDOW_LABEL`).

### `commands.rs` and `chat_streaming.rs`
Both have a `should_send_notification()` helper that checks `main_visible || overlay_visible`.
After the change, remove the `main_visible` check and the `MAIN_WINDOW_LABEL` reference in
each file. Only `overlay_visible` matters.

### `main.rs`
- Remove the block that hides the main window and attaches `on_window_event` to it (the block
  starting with `if let Some(main_window) = handle.get_webview_window(MAIN_WINDOW_LABEL)`).
- Remove `ambient::ambient_show_workspace` and `ambient::ambient_hide_workspace` from the
  `invoke_handler!` list.
- Add `ambient::ambient_set_workspace_mode` to the `invoke_handler!` list.

### `open_privacy_center` in `ambient.rs`
Remove the `show_workspace(app)?` call. App.tsx receives `privacy://open` naturally when it
is already mounted (workspace mode is active), so the show is no longer needed.

---

## Milestone 2 — Frontend entry point: single render root

### `main.tsx`
Remove the `isOverlayWindow()` branch entirely. Always render:
```tsx
root.render(<React.StrictMode><Root /></React.StrictMode>);
```
Keep `document.body.classList.add("overlay-body")` — the overlay window always uses the
overlay body class.

### New file: `Root.tsx`
Owns `workspaceOpen: boolean` state (default `false`). Exposes two callbacks:

```tsx
function openWorkspace() {
  await setWorkspaceMode(true);   // new ambientClient call
  setWorkspaceOpen(true);
}

function closeWorkspace() {
  await setWorkspaceMode(false);
  setWorkspaceOpen(false);
}
```

Renders:
```tsx
workspaceOpen
  ? <App onCloseWorkspace={closeWorkspace} />
  : <Overlay onOpenWorkspace={openWorkspace} />
```

Also listens to `ambient://state-changed`: if `overlay_mode === "workspace"` arrives (e.g.
from a tray menu click), sets `workspaceOpen = true` to keep React state in sync.

### `ambientClient.ts`
- Remove `showWorkspace()` (called the deleted `ambient_show_workspace`).
- Add `setWorkspaceMode(open: boolean): Promise<void>` invoking `ambient_set_workspace_mode`.
- Remove `isOverlayWindow()` export (no longer used after main.tsx is updated).
- Add `"workspace"` to the `OverlayMode` union type.

---

## Milestone 3 — Overlay.tsx: wire open callback, remove window open

### `Overlay.tsx`
- Add prop: `onOpenWorkspace: () => void`.
- In `handleOpenWorkspace`, replace `await showWorkspace()` with `props.onOpenWorkspace()`.
- Remove the import of `showWorkspace` from `ambientClient`.

No other changes needed. The overlay bar is self-contained.

---

## Milestone 4 — App.tsx: wire close callback, remove window close

### `App.tsx`
- Add prop: `onCloseWorkspace: () => void`.
- In the header, add a "back to companion" button (alongside the existing "Back to Home"
  button). Clicking it calls `props.onCloseWorkspace()`.
- Remove any calls to `ambientHideWorkspace` / `hideWorkspace` if present (currently none).
- The `privacy://open` listener that opens the privacy panel stays unchanged — it fires from
  within the same window now that workspace mode IS the active content.

---

## Milestone 5 — Tray menu update

### `ambient.rs` tray handler
The `"tray:workspace"` item currently calls `show_workspace(app)`. Change it to call
`ambient_set_workspace_mode(true, app)` which resizes the overlay and emits
`ambient://state-changed`. `Root.tsx` reacts to that event and sets `workspaceOpen = true`.
No separate event needed.

---

## Milestone 6 — Cleanup and test pass

- Delete `showWorkspace` export from `ambientClient.ts`.
- Delete `isOverlayWindow()` export from `ambientClient.ts`.
- Delete any `#overlay`-hash-based routing remnants.
- Run `npx tsc --noEmit` — fix any import errors from removed functions.
- Run `cargo check` — confirm no dangling `MAIN_WINDOW_LABEL` references.
- Run `npx vitest run` — update mocks that reference `ambient_show_workspace`,
  `isOverlayWindow`, or the old `showWorkspace` wrapper.
- Manual smoke test:
  1. Companion bar appears at top-right.
  2. Click "open full workspace" → window resizes to 960×700 with decorations, App.tsx renders.
  3. Click "back to companion" → window returns to 420×520, Overlay.tsx renders.
  4. Task state is consistent because both components read from the DB on mount.
  5. Tray "Open Full Workspace" menu item triggers the same resize.
  6. Hotkey and collapsed/expanded mode still work in companion.

---

## What Does Not Change

- All backend capability: chat, watcher, TTS, voice, retrieval — untouched.
- `App.tsx` internal logic — only one new prop added.
- `Overlay.tsx` internal logic — only the `handleOpenWorkspace` one-liner changes.
- Companion bar collapsed/expanded mode, hotkey, tray icon, always-on-top — same.
- Database schema, all Tauri commands, streaming pipeline — unchanged.

---

## Known Risk

The highest-risk step is Milestone 1: toggling `set_decorations` and `set_always_on_top` on
the overlay window at runtime. On macOS under Tauri v2 these calls exist but can behave
unexpectedly (window flicker, incorrect z-order). Worth verifying those two calls work in a
minimal test before committing to the full flow. If `set_decorations` causes problems, a
fallback is to keep the workspace mode frameless and add a custom drag/close bar in the
App.tsx header instead.

---

## File Change Summary

| File | Change |
|---|---|
| `src-tauri/tauri.conf.json` | Remove `main` window entry |
| `src-tauri/src/ambient.rs` | Add `Workspace` mode, add `ambient_set_workspace_mode`, remove show/hide workspace functions and `MAIN_WINDOW_LABEL` |
| `src-tauri/src/commands.rs` | Remove `main_visible` from `should_send_notification` |
| `src-tauri/src/chat_streaming.rs` | Same as commands.rs |
| `src-tauri/src/main.rs` | Remove main window setup block, swap command registrations |
| `src/main.tsx` | Remove branch, always render `<Root />` |
| `src/Root.tsx` | New file — workspace mode state, mounts Overlay or App |
| `src/ambientClient.ts` | Remove `showWorkspace`, `isOverlayWindow`; add `setWorkspaceMode` |
| `src/Overlay.tsx` | Add `onOpenWorkspace` prop, remove `showWorkspace` call |
| `src/App.tsx` | Add `onCloseWorkspace` prop, add back-to-companion button |
