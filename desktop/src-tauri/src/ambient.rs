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

// shortcut chosen to avoid common OS-reserved combos on macos/windows/linux.
// cmd/ctrl + shift + j reads as "summon jeff" and is not bound by default on
// any of the three platforms as of this writing.
pub const DEFAULT_HOTKEY: &str = "CmdOrCtrl+Shift+J";
// d3: mic toggle shortcut — registered as a secondary global shortcut.
pub const MIC_SHORTCUT: &str = "CmdOrCtrl+Shift+M";

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
    // workspace mode: single window resizes to full surface rather than
    // opening a second os window. always-on-top is disabled and the window
    // is centered so it behaves like a normal application surface.
    Workspace,
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
    pub wake_word_armed: bool,
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
    wake_word_armed: bool,
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
                wake_word_armed: false,
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
            wake_word_armed: guard.wake_word_armed,
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

    pub fn set_wake_word_armed(&self, armed: bool) {
        let mut guard = self.inner.lock().expect("ambient state lock poisoned");
        guard.wake_word_armed = armed;
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

#[derive(Debug, Clone, Serialize)]
struct OverlayShownPayload {
    interactive: bool,
}

// overlay window sizing. collapsed is a compact bar; expanded is the full
// companion surface. width stays stable so anchoring does not shift.
// workspace mode resizes the same window to a full app surface.
const OVERLAY_WIDTH: f64 = 420.0;
const OVERLAY_COLLAPSED_HEIGHT: f64 = 72.0;
const OVERLAY_EXPANDED_HEIGHT: f64 = 520.0;
const OVERLAY_MARGIN: f64 = 24.0;
pub const OVERLAY_WORKSPACE_WIDTH: f64 = 960.0;
pub const OVERLAY_WORKSPACE_HEIGHT: f64 = 700.0;

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

fn show_overlay_inner<R: Runtime>(app: &AppHandle<R>, interactive: bool) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(OVERLAY_WINDOW_LABEL) {
        window.show()?;
        if interactive {
            window.set_focus()?;
        }
    } else {
        build_overlay_window(app)?;
        if let Some(window) = app.get_webview_window(OVERLAY_WINDOW_LABEL) {
            window.show()?;
            if interactive {
                window.set_focus()?;
            }
        }
    }
    if let Some(state) = app.try_state::<AmbientState>() {
        state.set_overlay_visible(true);
    }
    let _ = app.emit(
        "ambient://overlay-shown",
        OverlayShownPayload { interactive },
    );
    Ok(())
}

pub fn show_overlay<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    // Passive overlay display is used by background context events such as
    // selection capture. It must not steal focus from the user's current app.
    show_overlay_inner(app, false)
}

pub fn show_overlay_interactive<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    // Explicit user summons (hotkey/tray/onboarding) should leave Jeff ready
    // for immediate typing.
    show_overlay_inner(app, true)
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

pub fn toggle_overlay_interactive<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let currently_visible = app
        .get_webview_window(OVERLAY_WINDOW_LABEL)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if currently_visible {
        hide_overlay(app)
    } else {
        show_overlay_interactive(app)
    }
}

fn reposition_overlay_top_right<R: Runtime>(window: &tauri::WebviewWindow<R>) {
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
}

fn center_window<R: Runtime>(window: &tauri::WebviewWindow<R>) {
    let placed = match window.primary_monitor() {
        Ok(Some(monitor)) => {
            let size: tauri::PhysicalSize<u32> = *monitor.size();
            let scale = monitor.scale_factor();
            let logical_width = size.width as f64 / scale;
            let logical_height = size.height as f64 / scale;
            let x = (logical_width - OVERLAY_WORKSPACE_WIDTH) / 2.0;
            let y = (logical_height - OVERLAY_WORKSPACE_HEIGHT) / 2.0;
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
        let _ = window.set_position(PhysicalPosition::new(160_i32, 80_i32));
    }
}

pub fn resize_overlay_for_mode<R: Runtime>(
    app: &AppHandle<R>,
    mode: OverlayMode,
) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(OVERLAY_WINDOW_LABEL) {
        match mode {
            OverlayMode::Workspace => {
                window.set_size(LogicalSize::new(
                    OVERLAY_WORKSPACE_WIDTH,
                    OVERLAY_WORKSPACE_HEIGHT,
                ))?;
                let _ = window.set_always_on_top(false);
                center_window(&window);
            }
            OverlayMode::Collapsed => {
                let _ = window.set_always_on_top(true);
                window.set_size(LogicalSize::new(OVERLAY_WIDTH, OVERLAY_COLLAPSED_HEIGHT))?;
                reposition_overlay_top_right(&window);
            }
            OverlayMode::Expanded => {
                let _ = window.set_always_on_top(true);
                window.set_size(LogicalSize::new(OVERLAY_WIDTH, OVERLAY_EXPANDED_HEIGHT))?;
                reposition_overlay_top_right(&window);
            }
        }
    }
    Ok(())
}

// set_workspace_mode switches the single overlay window between companion
// (collapsed/expanded) and full workspace (960×700) modes. open=true enters
// workspace mode; open=false returns to expanded companion mode.
pub fn set_workspace_mode<R: Runtime>(app: &AppHandle<R>, open: bool) -> tauri::Result<()> {
    let mode = if open {
        OverlayMode::Workspace
    } else {
        OverlayMode::Expanded
    };
    if let Some(state) = app.try_state::<AmbientState>() {
        state.set_overlay_mode(mode);
    }
    resize_overlay_for_mode(app, mode)?;
    if open {
        if let Some(window) = app.get_webview_window(OVERLAY_WINDOW_LABEL) {
            window.show()?;
            window.set_focus()?;
        }
    }
    if let Some(state) = app.try_state::<AmbientState>() {
        let _ = app.emit("ambient://state-changed", &state.snapshot());
    }
    Ok(())
}

pub fn open_privacy_center<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    // privacy center is now rendered inside the workspace mode of the single window.
    // switch to workspace mode first so app.tsx is mounted, then emit the event.
    set_workspace_mode(app, true)?;
    let _ = app.emit("privacy://open", serde_json::json!({}));
    Ok(())
}

pub fn open_onboarding_flow<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    open_onboarding_flow_at_step(app, 1)
}

// opens the onboarding wizard at a specific step. step=1 is the normal entry
// (first run or tray "Set up Jeff again"). step=2 jumps directly to API key
// setup, used when the error recovery CTA is clicked in the full workspace.
pub fn open_onboarding_flow_at_step<R: Runtime>(app: &AppHandle<R>, step: u8) -> tauri::Result<()> {
    show_overlay_interactive(app)?;
    if let Some(state) = app.try_state::<AmbientState>() {
        state.set_overlay_mode(OverlayMode::Expanded);
        let _ = resize_overlay_for_mode(app, OverlayMode::Expanded);
        let _ = app.emit("ambient://state-changed", &state.snapshot());
    }
    let _ = app.emit(
        "ambient://open-onboarding",
        serde_json::json!({ "step": step }),
    );
    Ok(())
}

// ---- tauri commands ---------------------------------------------------------

#[tauri::command]
pub fn ambient_toggle_overlay<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    toggle_overlay_interactive(&app).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ambient_show_overlay<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    show_overlay_interactive(&app).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ambient_hide_overlay<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    hide_overlay(&app).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ambient_set_workspace_mode<R: Runtime>(app: AppHandle<R>, open: bool) -> Result<(), String> {
    set_workspace_mode(&app, open).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ambient_open_privacy_center<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    open_privacy_center(&app).map_err(|e| e.to_string())
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
    let snapshot = state.snapshot();
    apply_tray_tooltip(&app, snapshot.tray_status, snapshot.wake_word_armed);
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

fn apply_tray_tooltip<R: Runtime>(app: &AppHandle<R>, status: TrayStatus, wake_word_armed: bool) {
    if let Some(tray) = app.tray_by_id("jeff-tray") {
        let _ = tray.set_tooltip(Some(tray_tooltip(status, wake_word_armed)));
    }
}

fn tray_tooltip(status: TrayStatus, wake_word_armed: bool) -> String {
    if wake_word_armed {
        format!("{} - wake word armed", status.tooltip())
    } else {
        status.tooltip().to_string()
    }
}

pub fn update_wake_word_armed<R: Runtime>(app: &AppHandle<R>, armed: bool) {
    if let Some(state) = app.try_state::<AmbientState>() {
        state.set_wake_word_armed(armed);
        let snapshot = state.snapshot();
        apply_tray_tooltip(app, snapshot.tray_status, armed);
        let _ = app.emit("ambient://state-changed", &snapshot);
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
    let setup_item = MenuItem::with_id(app, "tray:setup", "Set up Jeff again", true, None::<&str>)?;
    let privacy_item =
        MenuItem::with_id(app, "tray:privacy", "What Jeff Knows", true, None::<&str>)?;
    let voice_item = MenuItem::with_id(app, "tray:voice", "Voice Settings", true, None::<&str>)?;
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
            &privacy_item,
            &voice_item,
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
                let _ = show_overlay_interactive(app);
            }
            "tray:workspace" => {
                let _ = set_workspace_mode(app, true);
            }
            "tray:setup" => {
                let _ = open_onboarding_flow(app);
            }
            "tray:privacy" => {
                let _ = open_privacy_center(app);
            }
            "tray:voice" => {
                let _ = open_privacy_center(app);
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
                    // sync through SMAppService first; persist only after the
                    // OS state accepts the request or reports pending approval.
                    match crate::login_item::set_login_item_enabled(new_value) {
                        Ok(status) => {
                            let persisted = new_value && status.is_enabled_or_pending();
                            let _ = jeff.store.set_launch_at_login(persisted);
                        }
                        Err(err) => {
                            eprintln!("[jeff login-item] tray toggle failed: {err}");
                            let _ = jeff.store.set_launch_at_login(current);
                        }
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
                let _ = toggle_overlay_interactive(&tray_handle);
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

    let default_result = match app.global_shortcut().register(shortcut) {
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
    };

    // d3: register mic shortcut (non-fatal if already taken).
    if let Ok(mic_shortcut) = MIC_SHORTCUT.parse::<Shortcut>() {
        if let Err(err) = app.global_shortcut().register(mic_shortcut) {
            let _ = app.emit(
                "ambient://hotkey-conflict",
                &serde_json::json!({ "hotkey": MIC_SHORTCUT, "error": err.to_string() }),
            );
        }
    }

    let selection_shortcut: Result<Shortcut, _> =
        crate::selection_capture::SELECTION_CAPTURE_HOTKEY.parse();
    match selection_shortcut {
        Ok(selection_shortcut) => {
            if let Err(err) = app.global_shortcut().register(selection_shortcut) {
                let _ = app.emit(
                    "selection://hotkey-conflict",
                    &serde_json::json!({
                        "hotkey": crate::selection_capture::SELECTION_CAPTURE_HOTKEY,
                        "error": err.to_string()
                    }),
                );
            }
        }
        Err(err) => {
            let _ = app.emit(
                "selection://hotkey-conflict",
                &serde_json::json!({
                    "hotkey": crate::selection_capture::SELECTION_CAPTURE_HOTKEY,
                    "error": err.to_string()
                }),
            );
        }
    }

    default_result
}

pub fn shortcut_matches(shortcut: &tauri_plugin_global_shortcut::Shortcut, spec: &str) -> bool {
    spec.parse::<tauri_plugin_global_shortcut::Shortcut>()
        .map(|expected| shortcut == &expected)
        .unwrap_or(false)
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
    show_overlay_interactive(&app).map_err(|e| e.to_string())?;
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
        assert_eq!(
            crate::selection_capture::SELECTION_CAPTURE_HOTKEY,
            "CmdOrCtrl+Shift+V"
        );
    }
}
