// apex f1b-3b: the app supervises the daemon.
//
// the app owns the daemon's lifecycle (no launchd, no login item): it starts the
// daemon when the setting is on, notices when it is already up, and can stop it.
// a spawned daemon outlives the app -- that is the whole point, it keeps the
// background schedulers running when the app is closed or crashes -- so it must
// be explicitly killable, which is what stop() is for.
//
// controls precede capability: the setting defaults to OFF. an always-running
// background process only exists once the user turns it on in the Privacy
// Center, and turning it off actually terminates it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::daemon_client::{self, DaemonStatus};
use crate::daemon_ipc::IpcClient;
use crate::store::TaskStore;

pub const DAEMON_ENABLED_KEY: &str = "daemon_background_enabled";
const DAEMON_BINARY: &str = "jeff_daemon";

// off unless the user turned it on.
pub fn is_enabled(store: &TaskStore) -> bool {
    store
        .get_app_setting_bool(DAEMON_ENABLED_KEY)
        .ok()
        .flatten()
        .unwrap_or(false)
}

pub fn set_enabled(store: &TaskStore, enabled: bool) -> anyhow::Result<()> {
    store.set_app_setting(DAEMON_ENABLED_KEY, if enabled { "true" } else { "false" })?;
    Ok(())
}

// the daemon ships beside the app binary (dev: target/debug; bundled: the same
// MacOS/ directory as a sidecar).
pub fn daemon_binary_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidate = dir.join(DAEMON_BINARY);
    candidate.exists().then_some(candidate)
}

// start the daemon if the user enabled it and it is not already running.
// returns the resulting status; never fails the app -- if the daemon cannot be
// started, the app simply keeps running the full core itself.
pub fn ensure_running(store: &TaskStore, socket_path: &Path) -> DaemonStatus {
    if !is_enabled(store) {
        return DaemonStatus::unreachable();
    }

    let existing = daemon_client::probe(socket_path);
    if existing.owns_background_schedulers() {
        return existing;
    }

    let Some(binary) = daemon_binary_path() else {
        eprintln!("[jeff] daemon enabled but the jeff_daemon binary was not found beside the app");
        return DaemonStatus::unreachable();
    };

    // spawn detached: the daemon must outlive this app process.
    let spawned = Command::new(&binary)
        .env("JEFF_DAEMON_RUN_CORE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Err(err) = spawned {
        eprintln!("[jeff] failed to start the daemon: {err}");
        return DaemonStatus::unreachable();
    }

    // wait briefly for it to bind and host the core.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = daemon_client::probe(socket_path);
        if status.owns_background_schedulers() {
            eprintln!("[jeff] daemon started; it owns the background schedulers");
            return status;
        }
        if Instant::now() > deadline {
            eprintln!("[jeff] daemon did not come up in time; running the full core in-app");
            return DaemonStatus::unreachable();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

// the kill switch. asks the daemon to exit, so turning the setting off actually
// stops the background process rather than just ignoring it next launch.
pub fn stop(socket_path: &Path) {
    let Ok(mut client) = IpcClient::connect(socket_path) else {
        return;
    };
    let _ = client.set_read_timeout(Some(Duration::from_secs(2)));
    // the daemon exits on this; a closed connection / error is fine.
    let _ = client.call("shutdown", serde_json::Value::Null);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, TaskStore) {
        let dir = TempDir::new().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn f1b3b_background_daemon_is_off_until_the_user_turns_it_on() {
        // controls precede capability: no always-on process by default.
        let (_dir, store) = store();
        assert!(!is_enabled(&store), "daemon must default to off");

        set_enabled(&store, true).unwrap();
        assert!(is_enabled(&store));

        set_enabled(&store, false).unwrap();
        assert!(!is_enabled(&store), "turning it off must stick");
    }

    #[test]
    fn f1b3b_disabled_daemon_is_never_started() {
        // with the setting off, ensure_running must not probe or spawn anything.
        let (_dir, store) = store();
        let socket = std::env::temp_dir().join("jeff-supervisor-disabled.sock");
        let _ = std::fs::remove_file(&socket);
        let status = ensure_running(&store, &socket);
        assert!(!status.reachable);
        assert!(!status.owns_background_schedulers());
        assert!(!socket.exists(), "no daemon should have been started");
    }
}
