// apex f1b-3b / f1c: the app supervises the daemon.
//
// f1b-3b: the app starts the daemon when the setting is on, notices when it is
// already up, and can stop it. a spawned daemon outlives the app -- that is the
// whole point, it keeps the background schedulers running when the app is closed
// -- so it must be explicitly killable, which is what stop() is for.
//
// f1c: supervision is handed to launchd when available. a per-user LaunchAgent
// (see daemon_launchd) gives relaunch-on-crash and start-after-OS-restart with no
// app open -- neither of which a direct spawn can do, since a spawned daemon only
// ever came back when the app did. the direct spawn remains as a fallback for
// environments where launchd is unavailable (dev on non-macos, no gui session).
//
// controls precede capability: the setting defaults to OFF. an always-running
// background process only exists once the user turns it on in the Privacy
// Center, and turning it off actually terminates it. because launchd KeepAlive
// would resurrect a killed daemon, the kill switch unloads the agent first --
// unloading is the real off switch, not the IPC shutdown.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::daemon_client::{self, DaemonStatus};
use crate::daemon_ipc::IpcClient;
use crate::daemon_launchd;
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

    // already up? adopt it. launchd may have started it at login (f1c), or a
    // prior app run spawned it. we do not touch launchd here: bootstrapping a
    // second daemon while one already holds the socket would only crash-loop.
    let existing = daemon_client::probe(socket_path);
    if existing.owns_background_schedulers() {
        return existing;
    }

    let Some(binary) = daemon_binary_path() else {
        eprintln!("[jeff] daemon enabled but the jeff_daemon binary was not found beside the app");
        return DaemonStatus::unreachable();
    };

    // apex f1c: prefer launchd. installing the LaunchAgent starts the daemon
    // (RunAtLoad) and keeps it alive across crashes and OS restarts. only when
    // launchd is unavailable do we fall back to the f1b-3b detached spawn, which
    // outlives the app but cannot survive the app's own death or a reboot.
    let store_dir = socket_path.parent().unwrap_or_else(|| Path::new("."));
    let launchd_started = match daemon_launchd::install(&binary, socket_path, store_dir) {
        Ok(()) => {
            eprintln!("[jeff] daemon under launchd supervision (relaunch-on-crash, start-at-login)");
            true
        }
        Err(err) => {
            eprintln!("[jeff] launchd supervision unavailable ({err}); spawning the daemon directly");
            false
        }
    };

    if !launchd_started {
        // fallback: spawn detached so the daemon at least outlives this app process.
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

// the kill switch. turning the setting off must actually stop the background
// process, and keep it stopped.
pub fn stop(socket_path: &Path) {
    // apex f1c: unload launchd FIRST. with KeepAlive, launchd would relaunch the
    // daemon the instant an IPC shutdown killed it -- so unloading the agent (and
    // removing its plist, so it does not return at next login) is the real off
    // switch. bootout also terminates the running daemon. tolerant of no agent.
    let _ = daemon_launchd::uninstall();

    // also ask a directly-spawned (non-launchd fallback) daemon to exit.
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
