// apex f1b-2/f1b-2c: the headless daemon.
//
// binds the local IPC socket, answers a versioned handshake, and -- when asked
// to -- HOSTS THE CORE: it builds the world model and runs core_runtime's
// schedulers with a DaemonHost, entirely headless (no AppHandle, no webview).
// that is the process which will, in f1b-3, own the core outright while the
// tauri app becomes an IPC client.
//
// core hosting is opt-in (JEFF_DAEMON_RUN_CORE=1) and NOT the default, on
// purpose: today the app still runs its own core, so a daemon also running the
// core against the same database would double-run the mutating schedulers
// (standing jobs, speculation) -- split brain. flipping the app to a client is
// f1b-3 and carries product decisions. until then this flag proves the daemon
// can host the core, without changing what ships.
//
// env:
//   JEFF_DAEMON_SOCKET     override the socket path (default: app support dir)
//   JEFF_DAEMON_STORE_DIR  override the store dir (default: app support dir)
//   JEFF_DAEMON_RUN_CORE=1 host the core (world model + schedulers) headless

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use jeff_desktop::core_runtime;
use jeff_desktop::daemon_host::DaemonHost;
use jeff_desktop::daemon_ipc::{
    default_socket_path, EventSink, IpcEvent, IpcHandler, IpcRequest, IpcServer, PROTOCOL_VERSION,
};
use jeff_desktop::local_runtime::LocalRuntime;
use jeff_desktop::model_router::ModelRouter;
use jeff_desktop::providers::VoiceProvider;
use jeff_desktop::retrieval::default_embeddings_provider;
use jeff_desktop::state::JeffState;
use jeff_desktop::store::TaskStore;
use jeff_desktop::voice::OpenAiVoiceProvider;

struct DaemonHandler {
    core_running: bool,
}

impl IpcHandler for DaemonHandler {
    fn handle(&self, request: &IpcRequest) -> Result<serde_json::Value, String> {
        match request.method.as_str() {
            "handshake" => Ok(serde_json::json!({
                "protocol": PROTOCOL_VERSION,
                "service": "jeff-daemon",
                "core_running": self.core_running,
            })),
            "ping" => Ok(serde_json::json!("pong")),
            "status" => Ok(serde_json::json!({
                "core_running": self.core_running,
                "pid": std::process::id(),
            })),
            // the app's kill switch: turning the Privacy Center control off must
            // actually terminate the background process, not just ignore it.
            "shutdown" => {
                eprintln!("[jeff-daemon] shutdown requested; exiting");
                std::thread::spawn(|| {
                    // let the response flush before the process goes away.
                    std::thread::sleep(Duration::from_millis(100));
                    std::process::exit(0);
                });
                Ok(serde_json::json!("shutting down"))
            }
            other => Err(format!("unknown method: {other}")),
        }
    }
}

fn app_support_dir() -> PathBuf {
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

fn socket_path() -> PathBuf {
    if let Ok(path) = std::env::var("JEFF_DAEMON_SOCKET") {
        return PathBuf::from(path);
    }
    default_socket_path(&app_support_dir())
}

fn store_dir() -> PathBuf {
    if let Ok(path) = std::env::var("JEFF_DAEMON_STORE_DIR") {
        let path = PathBuf::from(path);
        let _ = std::fs::create_dir_all(&path);
        return path;
    }
    app_support_dir()
}

// build the same world model the app builds, from the same store.
fn build_state(dir: &PathBuf) -> Result<JeffState, String> {
    let store = TaskStore::initialize(dir).map_err(|err| format!("store init failed: {err}"))?;
    let local_runtime = Arc::new(LocalRuntime::new(dir));
    let embeddings = Arc::new(default_embeddings_provider(local_runtime.clone()));
    let model_router = Arc::new(ModelRouter::from_store_with_local_runtime(
        &store,
        Some(local_runtime.clone()),
    ));
    let voice: Arc<dyn VoiceProvider> = Arc::new(OpenAiVoiceProvider::from_env());
    Ok(JeffState::new(
        store,
        embeddings,
        local_runtime,
        model_router,
        voice,
    ))
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

    let sink = server.event_sink();
    let run_core = std::env::var("JEFF_DAEMON_RUN_CORE").as_deref() == Ok("1");

    // hold the core handle for the process lifetime.
    let _core = if run_core {
        let dir = store_dir();
        match build_state(&dir) {
            Ok(state) => {
                let host = Arc::new(DaemonHost::new(state, sink.clone()));
                let handle = core_runtime::start(host, core_runtime::CoreProfile::DaemonBackground);
                eprintln!(
                    "[jeff-daemon] core running headless (store {})",
                    dir.display()
                );
                Some(handle)
            }
            Err(err) => {
                eprintln!("[jeff-daemon] core failed to start: {err}");
                std::process::exit(1);
            }
        }
    } else {
        eprintln!("[jeff-daemon] core not hosted (set JEFF_DAEMON_RUN_CORE=1)");
        None
    };

    // heartbeat so a connected client can observe liveness over the event stream.
    heartbeat(sink);

    if let Err(err) = server.serve(Arc::new(DaemonHandler {
        core_running: run_core,
    })) {
        eprintln!("[jeff-daemon] serve loop ended: {err}");
        std::process::exit(1);
    }
}

fn heartbeat(sink: EventSink) {
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
}
