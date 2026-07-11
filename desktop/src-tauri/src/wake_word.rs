// apex c5: wake word sidecar lifecycle. the detector owns microphone audio in
// its own process; jeff only receives a tiny wake event token, never raw audio.

use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::Mutex,
};

use anyhow::{anyhow, Context, Result};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::{ambient, models::WakeWordStatusDto, store::TaskStore};

pub const WAKE_WORD_ENABLED_KEY: &str = "wake_word_enabled";
pub const WAKE_WORD_COMMAND_ENV: &str = "JEFF_WAKE_WORD_COMMAND";
pub const WAKE_WORD_PHRASE: &str = "hey jeff";
pub const WAKE_WORD_DETECTED_EVENT: &str = "wake_word://detected";
pub const WAKE_WORD_STATUS_EVENT: &str = "wake_word://status";

#[derive(Default)]
pub struct WakeWordManager {
    inner: Mutex<WakeWordInner>,
}

#[derive(Default)]
struct WakeWordInner {
    child: Option<Child>,
    last_error: Option<String>,
    last_detected_at: Option<i64>,
}

impl WakeWordManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn status(&self, store: &TaskStore) -> WakeWordStatusDto {
        self.status_with_enabled(load_enabled(store))
    }

    pub fn status_with_enabled(&self, enabled: bool) -> WakeWordStatusDto {
        let configured = configured_command().is_some();
        let mut running = false;
        let mut pid = None;
        let mut last_error = None;
        let mut last_detected_at = None;
        if let Ok(mut inner) = self.inner.lock() {
            running = child_is_running(&mut inner.child);
            pid = inner.child.as_ref().map(|child| child.id());
            last_error = inner.last_error.clone();
            last_detected_at = inner.last_detected_at;
        }
        WakeWordStatusDto {
            enabled,
            configured,
            armed: enabled && running,
            running,
            sidecar_pid: pid,
            phrase: WAKE_WORD_PHRASE.to_string(),
            last_detected_at,
            last_error,
            no_raw_audio_ipc: true,
        }
    }

    pub fn set_enabled<R: Runtime + 'static>(
        &self,
        store: &TaskStore,
        app: &AppHandle<R>,
        enabled: bool,
    ) -> Result<WakeWordStatusDto> {
        store.set_app_setting(WAKE_WORD_ENABLED_KEY, if enabled { "1" } else { "0" })?;
        if enabled {
            self.start(app.clone())?;
        } else {
            self.stop()?;
        }
        Ok(self.status(store))
    }

    pub fn start<R: Runtime + 'static>(&self, app: AppHandle<R>) -> Result<()> {
        let command_line = configured_command().ok_or_else(|| {
            let message = "wake-word detector command is not configured".to_string();
            self.set_last_error(message.clone());
            anyhow!(message)
        })?;
        self.start_with_command(command_line, Some(app))
    }

    pub fn start_with_command<R: Runtime + 'static>(
        &self,
        command_line: String,
        app: Option<AppHandle<R>>,
    ) -> Result<()> {
        let parts = split_command(&command_line).map_err(|err| {
            let message = err.to_string();
            self.set_last_error(message.clone());
            anyhow!(message)
        })?;
        self.stop().ok();
        let mut command = Command::new(&parts[0]);
        command
            .args(&parts[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => {
                let message = format!("failed to start wake-word detector '{}': {err}", parts[0]);
                self.set_last_error(message.clone());
                return Err(anyhow!(message));
            }
        };
        let stdout = child.stdout.take();
        {
            let mut inner = match self.inner.lock() {
                Ok(inner) => inner,
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(anyhow!("wake-word manager lock poisoned"));
                }
            };
            inner.child = Some(child);
            inner.last_error = None;
        }

        if let (Some(stdout), Some(app_handle)) = (stdout, app) {
            std::thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines().map_while(|line| line.ok()) {
                    if is_wake_word_signal(&line) {
                        handle_detected(&app_handle);
                    }
                }
                refresh_status(&app_handle);
            });
        }
        Ok(())
    }

    pub fn stop(&self) -> Result<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow!("wake-word manager lock poisoned"))?;
        if let Some(mut child) = inner.child.take() {
            if child
                .try_wait()
                .context("failed to inspect wake-word detector")?
                .is_none()
            {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        Ok(())
    }

    pub fn mark_detected(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.last_detected_at = Some(chrono::Utc::now().timestamp());
        }
    }

    fn set_last_error(&self, message: String) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.last_error = Some(message);
        }
    }
}

pub fn load_enabled(store: &TaskStore) -> bool {
    store
        .get_app_setting(WAKE_WORD_ENABLED_KEY)
        .ok()
        .flatten()
        .map(|value| value == "1" || value == "true")
        .unwrap_or(false)
}

pub fn maybe_start_from_settings<R: Runtime + 'static>(
    manager: &WakeWordManager,
    store: &TaskStore,
    app: &AppHandle<R>,
) {
    if load_enabled(store) {
        let _ = manager.start(app.clone());
    }
}

pub fn handle_detected<R: Runtime>(app: &AppHandle<R>) {
    if let Some(jeff) = app.try_state::<crate::state::JeffState>() {
        jeff.wake_word.mark_detected();
        if let Some(ambient_state) = app.try_state::<ambient::AmbientState>() {
            ambient_state.set_wake_word_armed(true);
        }
        let _ = app.emit(WAKE_WORD_STATUS_EVENT, &jeff.wake_word.status(&jeff.store));
    }
    let _ = app.emit(
        WAKE_WORD_DETECTED_EVENT,
        serde_json::json!({ "phrase": WAKE_WORD_PHRASE }),
    );
    let _ = ambient::show_overlay_interactive(app);
}

fn refresh_status<R: Runtime>(app: &AppHandle<R>) {
    if let Some(jeff) = app.try_state::<crate::state::JeffState>() {
        let status = jeff.wake_word.status(&jeff.store);
        ambient::update_wake_word_armed(app, status.armed);
        let _ = app.emit(WAKE_WORD_STATUS_EVENT, &status);
    }
}

pub fn is_wake_word_signal(line: &str) -> bool {
    matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "wake" | "wake_word" | "wake-word" | "hey jeff"
    )
}

fn configured_command() -> Option<String> {
    std::env::var(WAKE_WORD_COMMAND_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn split_command(command_line: &str) -> Result<Vec<String>> {
    let parts = command_line
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if parts.is_empty() {
        Err(anyhow!("wake-word detector command is empty"))
    } else {
        Ok(parts)
    }
}

fn child_is_running(child: &mut Option<Child>) -> bool {
    if let Some(child_ref) = child.as_mut() {
        match child_ref.try_wait() {
            Ok(Some(_)) => {
                *child = None;
                false
            }
            Ok(None) => true,
            Err(_) => false,
        }
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, TaskStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn c5_wake_word_defaults_off_and_no_raw_audio_ipc() {
        let (_dir, store) = store();
        let manager = WakeWordManager::new();
        let status = manager.status(&store);
        assert!(!status.enabled);
        assert!(!status.armed);
        assert!(status.no_raw_audio_ipc);
        assert_eq!(status.phrase, WAKE_WORD_PHRASE);
    }

    #[test]
    fn c5_signal_parser_only_accepts_wake_tokens() {
        assert!(is_wake_word_signal("wake"));
        assert!(is_wake_word_signal("hey jeff"));
        assert!(!is_wake_word_signal("pcm:abcdef"));
        assert!(!is_wake_word_signal("transcript hello"));
    }

    #[test]
    fn c5_start_failure_is_reported_in_status() {
        let manager = WakeWordManager::new();
        let err = manager
            .start_with_command::<tauri::Wry>("   ".to_string(), None)
            .unwrap_err();
        assert!(err.to_string().contains("empty"));
        let status = manager.status_with_enabled(true);
        assert!(status.last_error.unwrap_or_default().contains("empty"));
    }

    #[test]
    fn c5_disabling_kills_detector_process() {
        if !std::path::Path::new("/bin/sleep").exists() {
            return;
        }
        let manager = WakeWordManager::new();
        manager
            .start_with_command::<tauri::Wry>("/bin/sleep 30".to_string(), None)
            .unwrap();
        let running = manager.status_with_enabled(true);
        assert!(running.running);
        assert!(running.sidecar_pid.is_some());
        manager.stop().unwrap();
        let stopped = manager.status_with_enabled(false);
        assert!(!stopped.running);
        assert!(stopped.sidecar_pid.is_none());
    }
}
