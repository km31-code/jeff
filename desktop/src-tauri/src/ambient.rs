// ambient presence: tray, overlay window, global hotkey, native notifications.
// phase 11 adds no backend capability beyond phase 10. this module is a
// presence layer that routes existing events to native surfaces and keeps
// the overlay window lifecycle distinct from the full workspace window.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, Runtime, State, WebviewUrl,
    WebviewWindowBuilder,
};

// labels used across the window graph. keep stable: the frontend branches on
// these strings and the tray/hotkey logic looks them up by label.
pub const OVERLAY_WINDOW_LABEL: &str = "overlay";
pub const MAIN_WINDOW_LABEL: &str = "main";

// shortcut chosen to avoid common OS-reserved combos on macos/windows/linux.
// cmd/ctrl + shift + j reads as "summon jeff" and is not bound by default on
// any of the three platforms as of this writing.
pub const DEFAULT_HOTKEY: &str = "CmdOrCtrl+Shift+J";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrayStatus {
    Idle,
    Listening,
    Working,
}

impl Default for TrayStatus {
    fn default() -> Self {
        Self::Idle
    }
}

impl TrayStatus {
    pub fn tooltip(&self) -> &'static str {
        match self {
            TrayStatus::Idle => "Jeff — idle",
            TrayStatus::Listening => "Jeff — listening",
            TrayStatus::Working => "Jeff — working",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OverlayMode {
    Collapsed,
    Expanded,
}

impl Default for OverlayMode {
    fn default() -> Self {
        Self::Collapsed
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AmbientStateDto {
    pub tray_status: TrayStatus,
    pub overlay_mode: OverlayMode,
    pub overlay_visible: bool,
    pub hotkey: String,
    pub hotkey_registered: bool,
    pub notification_permission: String,
    pub single_instance: bool,
    pub quiet_mode: bool,
}

// ambient state is independent from the rest of JeffState on purpose:
// phase 11 is a presence layer and must not couple to backend capability.
#[derive(Default)]
pub struct AmbientState {
    inner: Mutex<AmbientStateInner>,
}

#[derive(Default)]
struct AmbientStateInner {
    tray_status: TrayStatus,
    overlay_mode: OverlayMode,
    overlay_visible: bool,
    hotkey: String,
    hotkey_registered: bool,
    notification_permission: String,
    quiet_mode: bool,
}

impl AmbientState {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(AmbientStateInner {
                tray_status: TrayStatus::Idle,
                overlay_mode: OverlayMode::Collapsed,
                overlay_visible: false,
                hotkey: DEFAULT_HOTKEY.to_string(),
                hotkey_registered: false,
                notification_permission: "unknown".to_string(),
                quiet_mode: false,
            }),
        }
    }

    pub fn snapshot(&self) -> AmbientStateDto {
        let guard = self.inner.lock().expect("ambient state lock poisoned");
        AmbientStateDto {
            tray_status: guard.tray_status,
            overlay_mode: guard.overlay_mode,
            overlay_visible: guard.overlay_visible,
            hotkey: guard.hotkey.clone(),
            hotkey_registered: guard.hotkey_registered,
            notification_permission: guard.notification_permission.clone(),
            single_instance: true,
            quiet_mode: guard.quiet_mode,
        }
    }

    pub fn set_tray_status(&self, status: TrayStatus) {
        let mut guard = self.inner.lock().expect("ambient state lock poisoned");
        guard.tray_status = status;
    }

    pub fn set_overlay_visible(&self, visible: bool) {
        let mut guard = self.inner.lock().expect("ambient state lock poisoned");
        guard.overlay_visible = visible;
    }

    pub fn set_overlay_mode(&self, mode: OverlayMode) {
        let mut guard = self.inner.lock().expect("ambient state lock poisoned");
        guard.overlay_mode = mode;
    }

    pub fn set_hotkey_registered(&self, registered: bool) {
        let mut guard = self.inner.lock().expect("ambient state lock poisoned");
        guard.hotkey_registered = registered;
    }

    pub fn set_notification_permission(&self, permission: &str) {
        let mut guard = self.inner.lock().expect("ambient state lock poisoned");
        guard.notification_permission = permission.to_string();
    }

    pub fn set_quiet_mode(&self, quiet: bool) {
        let mut guard = self.inner.lock().expect("ambient state lock poisoned");
        guard.quiet_mode = quiet;
    }

    pub fn is_quiet_mode(&self) -> bool {
        self.inner
            .lock()
            .expect("ambient state lock poisoned")
            .quiet_mode
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationPayload {
    pub title: String,
    pub body: String,
    // optional deep-link context for click handling. passed through to the
    // overlay via the ambient://notification-click event.
    pub context_kind: Option<String>,
    pub context_id: Option<i64>,
}

// overlay window sizing. collapsed is a compact bar; expanded is the full
// companion surface. width stays stable so anchoring does not shift.
const OVERLAY_WIDTH: f64 = 420.0;
const OVERLAY_COLLAPSED_HEIGHT: f64 = 72.0;
const OVERLAY_EXPANDED_HEIGHT: f64 = 520.0;
const OVERLAY_MARGIN: f64 = 24.0;

pub fn build_overlay_window<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if app.get_webview_window(OVERLAY_WINDOW_LABEL).is_some() {
        return Ok(());
    }

    let builder = WebviewWindowBuilder::new(
        app,
        OVERLAY_WINDOW_LABEL,
        WebviewUrl::App("index.html#overlay".into()),
    )
    .title("Jeff")
    .inner_size(OVERLAY_WIDTH, OVERLAY_COLLAPSED_HEIGHT)
    .decorations(false)
    .always_on_top(true)
    .resizable(false)
    .skip_taskbar(true)
    .focused(false)
    .visible(false);

    let window = builder.build()?;

    // anchor near the top-right of the primary monitor. monitor lookup is
    // best-effort: if the platform refuses, fall back to a reasonable
    // logical anchor and let the user move via the tray menu later.
    let placed = match window.primary_monitor() {
        Ok(Some(monitor)) => {
            let size: tauri::PhysicalSize<u32> = *monitor.size();
            let scale = monitor.scale_factor();
            let logical_width = size.width as f64 / scale;
            let x = logical_width - OVERLAY_WIDTH - OVERLAY_MARGIN;
            let y = OVERLAY_MARGIN;
            window
                .set_position(PhysicalPosition::new(
                    (x * scale) as i32,
                    (y * scale) as i32,
                ))
                .is_ok()
        }
        _ => false,
    };
    if !placed {
        let _ = window.set_position(PhysicalPosition::new(900_i32, 60_i32));
    }

    // closing the overlay window hides to tray rather than quitting.
    let handle = app.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            if let Some(overlay) = handle.get_webview_window(OVERLAY_WINDOW_LABEL) {
                let _ = overlay.hide();
            }
            if let Some(state) = handle.try_state::<AmbientState>() {
                state.set_overlay_visible(false);
            }
        }
    });

    Ok(())
}

pub fn show_overlay<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(OVERLAY_WINDOW_LABEL) {
        window.show()?;
        // do not call set_focus: summoning jeff must not steal focus from
        // the user's active app (see m11.3 focus preservation).
    } else {
        build_overlay_window(app)?;
        if let Some(window) = app.get_webview_window(OVERLAY_WINDOW_LABEL) {
            window.show()?;
        }
    }
    if let Some(state) = app.try_state::<AmbientState>() {
        state.set_overlay_visible(true);
    }
    let _ = app.emit("ambient://overlay-shown", ());
    Ok(())
}

pub fn hide_overlay<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(OVERLAY_WINDOW_LABEL) {
        window.hide()?;
    }
    if let Some(state) = app.try_state::<AmbientState>() {
        state.set_overlay_visible(false);
    }
    let _ = app.emit("ambient://overlay-hidden", ());
    Ok(())
}

pub fn toggle_overlay<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let currently_visible = app
        .get_webview_window(OVERLAY_WINDOW_LABEL)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if currently_visible {
        hide_overlay(app)
    } else {
        show_overlay(app)
    }
}

pub fn resize_overlay_for_mode<R: Runtime>(
    app: &AppHandle<R>,
    mode: OverlayMode,
) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(OVERLAY_WINDOW_LABEL) {
        let height = match mode {
            OverlayMode::Collapsed => OVERLAY_COLLAPSED_HEIGHT,
            OverlayMode::Expanded => OVERLAY_EXPANDED_HEIGHT,
        };
        window.set_size(LogicalSize::new(OVERLAY_WIDTH, height))?;
    }
    Ok(())
}

pub fn show_workspace<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        window.show()?;
        window.set_focus()?;
    }
    Ok(())
}

pub fn open_onboarding_flow<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    open_onboarding_flow_at_step(app, 1)
}

// opens the onboarding wizard at a specific step. step=1 is the normal entry
// (first run or tray "Set up Jeff again"). step=2 jumps directly to API key
// setup, used when the error recovery CTA is clicked in the full workspace.
pub fn open_onboarding_flow_at_step<R: Runtime>(app: &AppHandle<R>, step: u8) -> tauri::Result<()> {
    show_overlay(app)?;
    if let Some(state) = app.try_state::<AmbientState>() {
        state.set_overlay_mode(OverlayMode::Expanded);
        let _ = resize_overlay_for_mode(app, OverlayMode::Expanded);
        let _ = app.emit("ambient://state-changed", &state.snapshot());
    }
    let _ = app.emit("ambient://open-onboarding", serde_json::json!({ "step": step }));
    Ok(())
}

pub fn hide_workspace<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        window.hide()?;
    }
    Ok(())
}

// ---- tauri commands ---------------------------------------------------------

#[tauri::command]
pub fn ambient_toggle_overlay<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    toggle_overlay(&app).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ambient_show_overlay<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    show_overlay(&app).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ambient_hide_overlay<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    hide_overlay(&app).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ambient_show_workspace<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    show_workspace(&app).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ambient_open_onboarding<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    open_onboarding_flow(&app).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ambient_open_onboarding_at_step<R: Runtime>(
    app: AppHandle<R>,
    step: u8,
) -> Result<(), String> {
    open_onboarding_flow_at_step(&app, step).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ambient_hide_workspace<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    hide_workspace(&app).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ambient_set_overlay_mode<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AmbientState>,
    jeff_state: State<'_, crate::state::JeffState>,
    mode: OverlayMode,
) -> Result<AmbientStateDto, String> {
    state.set_overlay_mode(mode);
    // persist so the mode survives process restart (phase 19)
    let _ = jeff_state
        .store
        .set_overlay_expanded(mode == OverlayMode::Expanded);
    resize_overlay_for_mode(&app, mode).map_err(|e| e.to_string())?;
    let snapshot = state.snapshot();
    let _ = app.emit("ambient://state-changed", &snapshot);
    Ok(snapshot)
}

#[tauri::command]
pub fn ambient_set_tray_status<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AmbientState>,
    status: TrayStatus,
) -> Result<AmbientStateDto, String> {
    state.set_tray_status(status);
    apply_tray_tooltip(&app, status);
    let snapshot = state.snapshot();
    let _ = app.emit("ambient://state-changed", &snapshot);
    Ok(snapshot)
}

#[tauri::command]
pub fn ambient_set_quiet_mode<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AmbientState>,
    jeff_state: State<'_, crate::state::JeffState>,
    quiet: bool,
) -> Result<AmbientStateDto, String> {
    state.set_quiet_mode(quiet);
    // persist so quiet mode survives process restart (phase 19)
    let _ = jeff_state.store.set_quiet_mode(quiet);
    let snapshot = state.snapshot();
    let _ = app.emit("ambient://state-changed", &snapshot);
    Ok(snapshot)
}

#[tauri::command]
pub fn ambient_get_state(state: State<'_, AmbientState>) -> Result<AmbientStateDto, String> {
    Ok(state.snapshot())
}

#[tauri::command]
pub fn ambient_quit_app<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    app.exit(0);
    Ok(())
}

#[tauri::command]
pub fn ambient_mark_notification_permission(
    state: State<'_, AmbientState>,
    permission: String,
) -> Result<AmbientStateDto, String> {
    state.set_notification_permission(&permission);
    Ok(state.snapshot())
}

// ---- tray -------------------------------------------------------------------

fn apply_tray_tooltip<R: Runtime>(app: &AppHandle<R>, status: TrayStatus) {
    if let Some(tray) = app.tray_by_id("jeff-tray") {
        let _ = tray.set_tooltip(Some(status.tooltip()));
    }
}

// builds a fresh tray menu reflecting current toggle states. called once at
// install time and again after any toggle so the checkmark updates immediately.
fn build_tray_menu<R: Runtime>(
    app: &AppHandle<R>,
    launch_at_login: bool,
    quiet_mode: bool,
) -> tauri::Result<Menu<R>> {
    let show_item = MenuItem::with_id(app, "tray:show", "Show Jeff", true, None::<&str>)?;
    let workspace_item = MenuItem::with_id(
        app,
        "tray:workspace",
        "Open Full Workspace",
        true,
        None::<&str>,
    )?;
    let setup_item =
        MenuItem::with_id(app, "tray:setup", "Set up Jeff again", true, None::<&str>)?;
    let quiet_item = CheckMenuItem::with_id(
        app,
        "tray:quiet",
        "Quiet Mode",
        true,
        quiet_mode,
        None::<&str>,
    )?;
    let launch_item = CheckMenuItem::with_id(
        app,
        "tray:launch_at_login",
        "Launch at Login",
        true,
        launch_at_login,
        None::<&str>,
    )?;
    let quit_item = MenuItem::with_id(app, "tray:quit", "Quit Jeff", true, None::<&str>)?;
    Menu::with_items(
        app,
        &[
            &show_item,
            &workspace_item,
            &setup_item,
            &quiet_item,
            &launch_item,
            &quit_item,
        ],
    )
}

// rebuilds and replaces the tray menu so check-state is immediately visible.
fn refresh_tray_menu<R: Runtime>(app: &AppHandle<R>) {
    let launch_at_login = app
        .try_state::<crate::state::JeffState>()
        .and_then(|s| s.store.get_launch_at_login().ok())
        .unwrap_or(false);
    let quiet_mode = app
        .try_state::<AmbientState>()
        .map(|s| s.is_quiet_mode())
        .unwrap_or(false);
    if let Ok(menu) = build_tray_menu(app, launch_at_login, quiet_mode) {
        if let Some(tray) = app.tray_by_id("jeff-tray") {
            let _ = tray.set_menu(Some(menu));
        }
    }
}

pub fn install_tray<R: Runtime>(
    app: &AppHandle<R>,
    launch_at_login: bool,
    quiet_mode: bool,
) -> tauri::Result<()> {
    let menu = build_tray_menu(app, launch_at_login, quiet_mode)?;

    let icon: Image<'_> = app
        .default_window_icon()
        .cloned()
        .unwrap_or_else(|| Image::new_owned(vec![0u8; 4], 1, 1));

    let tray_handle = app.clone();
    let _tray = TrayIconBuilder::with_id("jeff-tray")
        .icon(icon)
        .tooltip(TrayStatus::Idle.tooltip())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "tray:show" => {
                let _ = show_overlay(app);
            }
            "tray:workspace" => {
                let _ = show_workspace(app);
            }
            "tray:setup" => {
                let _ = open_onboarding_flow(app);
            }
            "tray:quiet" => {
                if let Some(ambient) = app.try_state::<AmbientState>() {
                    let new_value = !ambient.is_quiet_mode();
                    ambient.set_quiet_mode(new_value);
                    // persist across restarts (phase 19)
                    if let Some(jeff) = app.try_state::<crate::state::JeffState>() {
                        let _ = jeff.store.set_quiet_mode(new_value);
                    }
                    let _ = app.emit("ambient://state-changed", &ambient.snapshot());
                    refresh_tray_menu(app);
                }
            }
            "tray:launch_at_login" => {
                if let Some(jeff) = app.try_state::<crate::state::JeffState>() {
                    let current = jeff.store.get_launch_at_login().unwrap_or(false);
                    let new_value = !current;
                    let _ = jeff.store.set_launch_at_login(new_value);
                    // sync with the OS login-item registry (phase 19)
                    use tauri_plugin_autostart::ManagerExt;
                    if new_value {
                        let _ = app.autolaunch().enable();
                    } else {
                        let _ = app.autolaunch().disable();
                    }
                    refresh_tray_menu(app);
                }
            }
            "tray:quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(move |_tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let _ = toggle_overlay(&tray_handle);
            }
        })
        .build(app)?;

    Ok(())
}

// ---- hotkey -----------------------------------------------------------------

pub fn register_global_hotkey<R: Runtime>(app: &AppHandle<R>) -> Result<bool, String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

    let shortcut: Shortcut = match DEFAULT_HOTKEY.parse() {
        Ok(shortcut) => shortcut,
        Err(err) => {
            if let Some(state) = app.try_state::<AmbientState>() {
                state.set_hotkey_registered(false);
            }
            return Err(format!("invalid hotkey spec '{DEFAULT_HOTKEY}': {err}"));
        }
    };

    match app.global_shortcut().register(shortcut) {
        Ok(_) => {
            if let Some(state) = app.try_state::<AmbientState>() {
                state.set_hotkey_registered(true);
            }
            Ok(true)
        }
        Err(err) => {
            if let Some(state) = app.try_state::<AmbientState>() {
                state.set_hotkey_registered(false);
            }
            // surface the conflict to the frontend rather than failing setup.
            let _ = app.emit(
                "ambient://hotkey-conflict",
                &serde_json::json!({ "hotkey": DEFAULT_HOTKEY, "error": err.to_string() }),
            );
            Err(err.to_string())
        }
    }
}

// ---- notifications ----------------------------------------------------------

pub fn dispatch_notification<R: Runtime>(
    app: &AppHandle<R>,
    payload: NotificationPayload,
) -> Result<(), String> {
    // quiet mode fully suppresses native notifications. the in-app event is
    // still emitted so the ui can log it if desired, but no os surface fires.
    if app
        .try_state::<AmbientState>()
        .map(|s| s.is_quiet_mode())
        .unwrap_or(false)
    {
        let _ = app.emit("ambient://notification-suppressed", &payload);
        return Ok(());
    }

    use tauri_plugin_notification::NotificationExt;
    app.notification()
        .builder()
        .title(&payload.title)
        .body(&payload.body)
        .show()
        .map_err(|e| e.to_string())?;
    let _ = app.emit("ambient://notification-dispatched", &payload);
    Ok(())
}

#[tauri::command]
pub fn ambient_notify<R: Runtime>(
    app: AppHandle<R>,
    payload: NotificationPayload,
) -> Result<(), String> {
    dispatch_notification(&app, payload)
}

#[tauri::command]
pub fn ambient_notification_clicked<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AmbientState>,
    context_kind: Option<String>,
    context_id: Option<i64>,
) -> Result<AmbientStateDto, String> {
    // notification click path: expand overlay, emit a context event the
    // frontend listens to in order to select the right surface.
    state.set_overlay_mode(OverlayMode::Expanded);
    resize_overlay_for_mode(&app, OverlayMode::Expanded).map_err(|e| e.to_string())?;
    show_overlay(&app).map_err(|e| e.to_string())?;
    let _ = app.emit(
        "ambient://notification-click",
        &serde_json::json!({ "context_kind": context_kind, "context_id": context_id }),
    );
    Ok(state.snapshot())
}

// ---- tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambient_state_defaults_to_idle_collapsed_hidden() {
        let state = AmbientState::new();
        let snapshot = state.snapshot();
        assert_eq!(snapshot.tray_status, TrayStatus::Idle);
        assert_eq!(snapshot.overlay_mode, OverlayMode::Collapsed);
        assert!(!snapshot.overlay_visible);
        assert_eq!(snapshot.hotkey, DEFAULT_HOTKEY);
        assert!(!snapshot.hotkey_registered);
        assert!(!snapshot.quiet_mode);
    }

    #[test]
    fn ambient_state_tracks_tray_and_overlay_changes() {
        let state = AmbientState::new();
        state.set_tray_status(TrayStatus::Working);
        state.set_overlay_mode(OverlayMode::Expanded);
        state.set_overlay_visible(true);
        state.set_quiet_mode(true);
        state.set_hotkey_registered(true);
        state.set_notification_permission("granted");

        let snapshot = state.snapshot();
        assert_eq!(snapshot.tray_status, TrayStatus::Working);
        assert_eq!(snapshot.overlay_mode, OverlayMode::Expanded);
        assert!(snapshot.overlay_visible);
        assert!(snapshot.quiet_mode);
        assert!(snapshot.hotkey_registered);
        assert_eq!(snapshot.notification_permission, "granted");
    }

    #[test]
    fn tray_status_tooltip_describes_state() {
        assert_eq!(TrayStatus::Idle.tooltip(), "Jeff — idle");
        assert_eq!(TrayStatus::Listening.tooltip(), "Jeff — listening");
        assert_eq!(TrayStatus::Working.tooltip(), "Jeff — working");
    }

    #[test]
    fn default_hotkey_is_cmd_shift_j() {
        assert_eq!(DEFAULT_HOTKEY, "CmdOrCtrl+Shift+J");
    }
}
