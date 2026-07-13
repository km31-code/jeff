// apex f1b-2: the headless daemon binary skeleton.
//
// this is the process that, in f1b-3, will own the core (world model,
// schedulers, agent runtime) and serve the tauri app over local IPC. today it
// is a skeleton: it binds the daemon socket, answers a versioned handshake and a
// ping, and pushes a periodic heartbeat event to connected clients -- enough to
// prove the transport end to end and give the app something to connect to.
// nothing in the shipping app depends on it yet.
//
// socket path: the app support dir (JEFF_DAEMON_SOCKET overrides for testing).

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use jeff_desktop::daemon_ipc::{
    default_socket_path, IpcEvent, IpcHandler, IpcRequest, IpcServer, PROTOCOL_VERSION,
};

struct DaemonHandler;

impl IpcHandler for DaemonHandler {
    fn handle(&self, request: &IpcRequest) -> Result<serde_json::Value, String> {
        match request.method.as_str() {
            "handshake" => Ok(serde_json::json!({
                "protocol": PROTOCOL_VERSION,
                "service": "jeff-daemon",
            })),
            "ping" => Ok(serde_json::json!("pong")),
            other => Err(format!("unknown method: {other}")),
        }
    }
}

fn socket_path() -> PathBuf {
    if let Ok(path) = std::env::var("JEFF_DAEMON_SOCKET") {
        return PathBuf::from(path);
    }
    // mirror the app's app_local_data_dir on macOS so the app and daemon agree
    // without extra configuration.
    let base = dirs_next_app_support();
    default_socket_path(&base)
}

fn dirs_next_app_support() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let mut path = PathBuf::from(home);
        path.push("Library");
        path.push("Application Support");
        path.push("com.jeff.desktop");
        let _ = std::fs::create_dir_all(&path);
        return path;
    }
    std::env::temp_dir()
}

fn main() {
    let path = socket_path();
    let server = match IpcServer::bind(&path) {
        Ok(server) => server,
        Err(err) => {
            eprintln!("[jeff-daemon] failed to bind {}: {err}", path.display());
            std::process::exit(1);
        }
    };
    eprintln!("[jeff-daemon] listening on {}", path.display());

    // heartbeat: push a liveness event to connected clients on an interval, so a
    // connected app can observe the daemon is alive over the event stream.
    let sink = server.event_sink();
    thread::spawn(move || {
        let mut tick: u64 = 0;
        loop {
            thread::sleep(Duration::from_secs(15));
            tick += 1;
            sink.broadcast(&IpcEvent {
                event: "daemon://heartbeat".to_string(),
                payload: serde_json::json!({ "tick": tick }),
            });
        }
    });

    if let Err(err) = server.serve(Arc::new(DaemonHandler)) {
        eprintln!("[jeff-daemon] serve loop ended: {err}");
        std::process::exit(1);
    }
}
