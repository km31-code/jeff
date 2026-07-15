// apex f1c: the daemon survives crashes and OS restarts via a launchd LaunchAgent.
//
// f1b-3 had the app spawn the daemon directly, so the daemon only ever came back
// when the app did. f1c hands supervision to launchd: a per-user LaunchAgent with
//   RunAtLoad  -> start at login, after an OS restart, with no app open
//   KeepAlive  -> relaunch on crash
//   ThrottleInterval -> a crashing daemon cannot spin the machine
//
// controls still precede capability (f1b-3b): the agent only exists once the user
// turns the background daemon on. and because KeepAlive would resurrect a killed
// daemon the instant it died, the kill switch MUST unload the agent -- unloading is
// the real off switch, not an IPC shutdown. that ordering lives in daemon_supervisor.

use std::path::{Path, PathBuf};

pub const AGENT_LABEL: &str = "com.jeff.daemon";

// the label may be overridden for isolated testing so a check never touches the
// user's real agent. defaults to AGENT_LABEL.
fn agent_label() -> String {
    std::env::var("JEFF_DAEMON_AGENT_LABEL").unwrap_or_else(|_| AGENT_LABEL.to_string())
}

// where the per-user LaunchAgent plist lives.
pub fn plist_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let mut path = PathBuf::from(home);
    path.push("Library");
    path.push("LaunchAgents");
    path.push(format!("{}.plist", agent_label()));
    Some(path)
}

// minimal XML text escaping for values embedded in the plist (paths, label).
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// the plist text. RunAtLoad + KeepAlive is the whole point: start-at-login plus
// relaunch-on-crash. stderr is captured beside the store so a crash is inspectable.
pub fn render_plist(binary: &Path, socket: &Path, store_dir: &Path) -> String {
    let label = xml_escape(&agent_label());
    let binary = xml_escape(&binary.to_string_lossy());
    let socket = xml_escape(&socket.to_string_lossy());
    let store = xml_escape(&store_dir.to_string_lossy());
    let log = xml_escape(&store_dir.join("daemon.launchd.log").to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{binary}</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>JEFF_DAEMON_RUN_CORE</key>
        <string>1</string>
        <key>JEFF_DAEMON_SOCKET</key>
        <string>{socket}</string>
        <key>JEFF_DAEMON_STORE_DIR</key>
        <string>{store}</string>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ThrottleInterval</key>
    <integer>10</integer>
    <key>ProcessType</key>
    <string>Background</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#
    )
}

// launchd is only available on macos; elsewhere the supervisor uses its direct-spawn
// fallback and these are inert.
pub fn is_available() -> bool {
    cfg!(target_os = "macos") && plist_path().is_some()
}

// write the plist and (re)load the agent. idempotent: an already-loaded agent is
// booted out first so a stale plist (after an app move or update) is refreshed and
// launchd re-reads the current binary path.
pub fn install(binary: &Path, socket: &Path, store_dir: &Path) -> Result<(), String> {
    let path = plist_path().ok_or("no HOME; cannot locate ~/Library/LaunchAgents")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("create LaunchAgents dir: {err}"))?;
    }
    std::fs::write(&path, render_plist(binary, socket, store_dir))
        .map_err(|err| format!("write plist: {err}"))?;
    load(&path)
}

// the real off switch. unload the agent (so KeepAlive can no longer resurrect it)
// and remove the plist so it does not return at next login. tolerant of a
// not-currently-loaded agent.
pub fn uninstall() -> Result<(), String> {
    unload();
    if let Some(path) = plist_path() {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

// is the agent currently bootstrapped in the user gui domain?
pub fn is_loaded() -> bool {
    inner::print_ok()
}

#[cfg(target_os = "macos")]
mod inner {
    use super::agent_label;
    use std::path::Path;
    use std::process::Command;

    fn gui_domain() -> String {
        let uid = unsafe { libc::getuid() };
        format!("gui/{uid}")
    }

    fn service_target() -> String {
        format!("{}/{}", gui_domain(), agent_label())
    }

    fn launchctl(args: &[&str]) -> Result<(), String> {
        let output = Command::new("launchctl")
            .args(args)
            .output()
            .map_err(|err| format!("launchctl {args:?}: {err}"))?;
        if output.status.success() {
            return Ok(());
        }
        Err(format!(
            "launchctl {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }

    pub(super) fn load(plist: &Path) -> Result<(), String> {
        // refresh: boot out any prior instance so bootstrap re-reads the plist.
        // a not-loaded agent errors here; that is fine.
        let _ = launchctl(&["bootout", &service_target()]);
        let domain = gui_domain();
        let plist = plist.to_string_lossy();
        match launchctl(&["bootstrap", &domain, &plist]) {
            Ok(()) => Ok(()),
            // bootstrap races with the async bootout above and can report the
            // service as already present; treat an actually-loaded agent as success.
            Err(err) => {
                if print_ok() {
                    Ok(())
                } else {
                    Err(err)
                }
            }
        }
    }

    pub(super) fn unload() {
        let _ = launchctl(&["bootout", &service_target()]);
    }

    pub(super) fn print_ok() -> bool {
        launchctl(&["print", &service_target()]).is_ok()
    }
}

#[cfg(not(target_os = "macos"))]
mod inner {
    use std::path::Path;

    pub(super) fn load(_plist: &Path) -> Result<(), String> {
        Err("launchd supervision is only available on macOS".to_string())
    }

    pub(super) fn unload() {}

    pub(super) fn print_ok() -> bool {
        false
    }
}

use inner::{load, unload};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn f1c_plist_declares_crash_and_restart_supervision() {
        // the two properties f1c exists to add: relaunch-on-crash (KeepAlive) and
        // start-after-restart with no app open (RunAtLoad), plus a throttle so a
        // crash-loop cannot spin the machine.
        let plist = render_plist(
            Path::new("/Applications/Jeff.app/Contents/MacOS/jeff_daemon"),
            Path::new("/tmp/jeff.sock"),
            Path::new("/tmp/store"),
        );
        assert!(plist.contains("<key>RunAtLoad</key>"), "must start at login");
        assert!(
            plist.contains("<key>KeepAlive</key>"),
            "must relaunch on crash"
        );
        assert!(
            plist.contains("<key>ThrottleInterval</key>"),
            "must throttle a crash loop"
        );
        // RunAtLoad and KeepAlive must be true, not merely present.
        let after_run_at_load = plist.split("<key>RunAtLoad</key>").nth(1).unwrap();
        assert!(
            after_run_at_load.trim_start().starts_with("<true/>"),
            "RunAtLoad must be true"
        );
        let after_keep_alive = plist.split("<key>KeepAlive</key>").nth(1).unwrap();
        assert!(
            after_keep_alive.trim_start().starts_with("<true/>"),
            "KeepAlive must be true"
        );
        assert!(
            plist.contains("<key>JEFF_DAEMON_RUN_CORE</key>"),
            "the launchd daemon must host the core"
        );
    }

    #[test]
    fn f1c_plist_pins_the_exact_socket_store_and_binary() {
        // the app and daemon must meet on the same socket and store; the plist
        // pins them rather than relying on ambient HOME resolution.
        let plist = render_plist(
            Path::new("/opt/jeff/jeff_daemon"),
            Path::new("/var/app/jeff-daemon.sock"),
            Path::new("/var/app"),
        );
        assert!(plist.contains("/opt/jeff/jeff_daemon"));
        assert!(plist.contains("/var/app/jeff-daemon.sock"));
        assert!(plist.contains("<string>/var/app</string>"));
    }

    #[test]
    fn f1c_agent_plist_lives_in_user_launch_agents() {
        // per-user LaunchAgent, not a system-wide LaunchDaemon (no root, no approval).
        let path = plist_path().expect("HOME is set in tests");
        let text = path.to_string_lossy();
        assert!(text.contains("Library/LaunchAgents/"));
        assert!(text.ends_with(&format!("{AGENT_LABEL}.plist")));
    }

    #[test]
    fn f1c_plist_values_are_xml_escaped() {
        // a path with an ampersand must not corrupt the plist.
        let plist = render_plist(
            Path::new("/tmp/a&b/jeff_daemon"),
            Path::new("/tmp/a&b/jeff.sock"),
            Path::new("/tmp/a&b"),
        );
        assert!(plist.contains("a&amp;b"));
        assert!(!plist.contains("a&b/jeff_daemon"));
    }
}
