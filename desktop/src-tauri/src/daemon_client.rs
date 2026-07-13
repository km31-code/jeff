// apex f1b-3: the app's view of the daemon.
//
// the app asks the daemon, over the local IPC socket, whether it is up and
// whether it is hosting the core. that answer decides the app's CoreProfile:
//
//   daemon hosting the core  -> CoreProfile::AppClient  (app skips the mutating
//                                background schedulers; the daemon owns them)
//   no daemon / not hosting  -> CoreProfile::Full       (app runs everything,
//                                exactly as it did before f1b-3)
//
// the fallback is the point: if the daemon is missing, crashed, or a version
// mismatch, the app degrades to standalone and nothing is lost.

use std::path::Path;
use std::time::Duration;

use crate::daemon_ipc::{IpcClient, PROTOCOL_VERSION};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonStatus {
    pub reachable: bool,
    pub protocol_matches: bool,
    pub core_running: bool,
}

impl DaemonStatus {
    pub fn unreachable() -> Self {
        Self {
            reachable: false,
            protocol_matches: false,
            core_running: false,
        }
    }

    // the app may hand the background schedulers to the daemon only when it is
    // reachable, speaks our protocol, and is actually hosting the core.
    pub fn owns_background_schedulers(&self) -> bool {
        self.reachable && self.protocol_matches && self.core_running
    }
}

// handshake with the daemon. never panics and never blocks for long: any error
// (no socket, stale socket, wrong version, timeout) is reported as unreachable
// so the app falls back to running everything itself.
pub fn probe(socket_path: &Path) -> DaemonStatus {
    let Ok(mut client) = IpcClient::connect(socket_path) else {
        return DaemonStatus::unreachable();
    };
    // a daemon that does not answer promptly is treated as absent.
    let _ = client.set_read_timeout(Some(Duration::from_secs(2)));

    let Ok(response) = client.call("handshake", serde_json::Value::Null) else {
        return DaemonStatus::unreachable();
    };
    let Some(result) = response.result else {
        return DaemonStatus::unreachable();
    };

    let protocol_matches = result
        .get("protocol")
        .and_then(|value| value.as_u64())
        .map(|value| value == PROTOCOL_VERSION as u64)
        .unwrap_or(false);
    let core_running = result
        .get("core_running")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);

    DaemonStatus {
        reachable: true,
        protocol_matches,
        core_running,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f1b3_absent_daemon_leaves_the_app_owning_everything() {
        // no socket at all: the app must fall back to running the full core.
        let missing = std::env::temp_dir().join("jeff-daemon-does-not-exist.sock");
        let _ = std::fs::remove_file(&missing);
        let status = probe(&missing);
        assert!(!status.reachable);
        assert!(
            !status.owns_background_schedulers(),
            "a missing daemon must never take the schedulers"
        );
    }

    #[test]
    fn f1b3_daemon_only_takes_schedulers_when_hosting_a_matching_core() {
        // reachable but not hosting the core -> app keeps the schedulers.
        let idle = DaemonStatus {
            reachable: true,
            protocol_matches: true,
            core_running: false,
        };
        assert!(!idle.owns_background_schedulers());

        // reachable and hosting, but a protocol mismatch -> app keeps them.
        let mismatched = DaemonStatus {
            reachable: true,
            protocol_matches: false,
            core_running: true,
        };
        assert!(!mismatched.owns_background_schedulers());

        // reachable, matching, hosting -> the daemon owns them.
        let owning = DaemonStatus {
            reachable: true,
            protocol_matches: true,
            core_running: true,
        };
        assert!(owning.owns_background_schedulers());
    }
}
