// apex f1b-2: local IPC transport for the headless daemon.
//
// a unix-domain-socket transport with a framed json request/response protocol
// plus a server->client event stream. this is the wire the f1b-3 split runs on:
// the daemon owns the core and serves requests + pushes core events; the tauri
// app becomes a client. built and tested here in isolation, off the app's
// critical path -- nothing in the running app depends on it yet.
//
// framing: each message is a 4-byte big-endian length prefix followed by that
// many bytes of json. writes on a single connection are serialized by a mutex so
// response frames and broadcast event frames never interleave.

use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use serde::{Deserialize, Serialize};

// bumped whenever the wire protocol changes; the handshake exchanges it so a
// mismatched app and daemon fail fast instead of corrupting state.
pub const PROTOCOL_VERSION: u32 = 1;

// a frame larger than this is treated as a protocol error rather than an
// allocation the peer can use to exhaust memory.
const MAX_FRAME: usize = 16 * 1024 * 1024;

pub const SOCKET_FILE_NAME: &str = "jeff-daemon.sock";

// the daemon socket lives beside the store in the app support directory.
pub fn default_socket_path(app_support_dir: &Path) -> PathBuf {
    app_support_dir.join(SOCKET_FILE_NAME)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcRequest {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponse {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// an unsolicited server->client message (a core event re-emitted to the app).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcEvent {
    pub event: String,
    pub payload: serde_json::Value,
}

pub fn write_frame<W: Write>(writer: &mut W, bytes: &[u8]) -> io::Result<()> {
    let len = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame too large to encode"))?;
    writer.write_all(&len.to_be_bytes())?;
    writer.write_all(bytes)?;
    writer.flush()
}

// reads one frame. returns Ok(None) on a clean end-of-stream (peer closed).
pub fn read_frame<R: Read>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame exceeds maximum size"));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    Ok(Some(buf))
}

// the daemon implements this to answer requests. handlers must be cheap and
// non-blocking-ish; long work belongs in the core, not the dispatch path.
pub trait IpcHandler: Send + Sync + 'static {
    fn handle(&self, request: &IpcRequest) -> Result<serde_json::Value, String>;
}

// registry of connected clients' serialized write halves, used to broadcast
// events. cloneable so the core can hold a sink while the server loop owns the
// listener.
#[derive(Clone, Default)]
pub struct EventSink {
    clients: Arc<Mutex<Vec<Arc<Mutex<UnixStream>>>>>,
}

impl EventSink {
    pub fn client_count(&self) -> usize {
        self.clients.lock().map(|c| c.len()).unwrap_or(0)
    }

    // push an event to every connected client, pruning any that have gone away.
    pub fn broadcast(&self, event: &IpcEvent) {
        let bytes = match serde_json::to_vec(event) {
            Ok(bytes) => bytes,
            Err(_) => return,
        };
        let mut clients = match self.clients.lock() {
            Ok(clients) => clients,
            Err(_) => return,
        };
        clients.retain(|client| match client.lock() {
            Ok(mut stream) => write_frame(&mut *stream, &bytes).is_ok(),
            Err(_) => false,
        });
    }

    fn register(&self, stream: Arc<Mutex<UnixStream>>) {
        if let Ok(mut clients) = self.clients.lock() {
            clients.push(stream);
        }
    }
}

pub struct IpcServer {
    listener: UnixListener,
    socket_path: PathBuf,
    sink: EventSink,
}

impl IpcServer {
    // bind the socket, clearing any stale file from a previous run.
    pub fn bind<P: AsRef<Path>>(socket_path: P) -> io::Result<Self> {
        let socket_path = socket_path.as_ref().to_path_buf();
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path)?;
        Ok(Self {
            listener,
            socket_path,
            sink: EventSink::default(),
        })
    }

    // a handle for pushing events to connected clients.
    pub fn event_sink(&self) -> EventSink {
        self.sink.clone()
    }

    // accept connections forever, dispatching each on its own thread. blocks.
    pub fn serve<H: IpcHandler>(&self, handler: Arc<H>) -> io::Result<()> {
        for stream in self.listener.incoming() {
            let stream = stream?;
            let read_half = stream.try_clone()?;
            let write_half = Arc::new(Mutex::new(stream));
            self.sink.register(write_half.clone());
            let handler = handler.clone();
            thread::spawn(move || {
                let _ = serve_connection(read_half, write_half, handler);
            });
        }
        Ok(())
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

fn serve_connection<H: IpcHandler>(
    mut read_half: UnixStream,
    write_half: Arc<Mutex<UnixStream>>,
    handler: Arc<H>,
) -> io::Result<()> {
    while let Some(frame) = read_frame(&mut read_half)? {
        let response = match serde_json::from_slice::<IpcRequest>(&frame) {
            Ok(request) => match handler.handle(&request) {
                Ok(result) => IpcResponse {
                    id: request.id,
                    result: Some(result),
                    error: None,
                },
                Err(err) => IpcResponse {
                    id: request.id,
                    result: None,
                    error: Some(err),
                },
            },
            Err(err) => IpcResponse {
                id: 0,
                result: None,
                error: Some(format!("malformed request: {err}")),
            },
        };
        let bytes = serde_json::to_vec(&response)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        let mut stream = write_half
            .lock()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "write lock poisoned"))?;
        write_frame(&mut *stream, &bytes)?;
    }
    Ok(())
}

// a request/response client. events pushed by the server are read with
// read_event on a separate connection dedicated to listening.
pub struct IpcClient {
    stream: UnixStream,
    next_id: u64,
}

impl IpcClient {
    pub fn connect<P: AsRef<Path>>(socket_path: P) -> io::Result<Self> {
        let stream = UnixStream::connect(socket_path)?;
        Ok(Self { stream, next_id: 1 })
    }

    pub fn call(&mut self, method: &str, params: serde_json::Value) -> io::Result<IpcResponse> {
        let id = self.next_id;
        self.next_id += 1;
        let request = IpcRequest {
            id,
            method: method.to_string(),
            params,
        };
        let bytes = serde_json::to_vec(&request)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        write_frame(&mut self.stream, &bytes)?;
        let frame = read_frame(&mut self.stream)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "daemon closed connection"))?;
        serde_json::from_slice(&frame).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }

    // read the next server-pushed event on this connection (for a listen-only
    // client). blocks until a frame arrives or the peer closes.
    pub fn read_event(&mut self) -> io::Result<Option<IpcEvent>> {
        match read_frame(&mut self.stream)? {
            Some(frame) => serde_json::from_slice(&frame)
                .map(Some)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err)),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    struct SkeletonHandler;

    impl IpcHandler for SkeletonHandler {
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

    fn temp_socket_path(tag: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let unique = format!(
            "jeff-ipc-{tag}-{}-{}.sock",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
                ^ (Instant::now().elapsed().as_nanos().rotate_left(17))
        );
        path.push(unique);
        path
    }

    fn spawn_server(path: &Path) -> EventSink {
        let server = IpcServer::bind(path).expect("bind server");
        let sink = server.event_sink();
        thread::spawn(move || {
            let _ = server.serve(Arc::new(SkeletonHandler));
        });
        // wait for the socket to be connectable.
        let deadline = Instant::now() + Duration::from_secs(2);
        while UnixStream::connect(path).is_err() {
            if Instant::now() > deadline {
                panic!("server never became connectable");
            }
            thread::sleep(Duration::from_millis(5));
        }
        sink
    }

    #[test]
    fn f1b2_request_response_round_trips_over_the_socket() {
        let path = temp_socket_path("reqresp");
        let _sink = spawn_server(&path);
        let mut client = IpcClient::connect(&path).expect("connect");

        let handshake = client.call("handshake", serde_json::Value::Null).unwrap();
        assert_eq!(handshake.id, 1);
        assert_eq!(handshake.error, None);
        assert_eq!(handshake.result.unwrap()["protocol"], PROTOCOL_VERSION);

        let ping = client.call("ping", serde_json::Value::Null).unwrap();
        assert_eq!(ping.id, 2);
        assert_eq!(ping.result.unwrap(), serde_json::json!("pong"));
    }

    #[test]
    fn f1b2_unknown_method_returns_a_structured_error() {
        let path = temp_socket_path("err");
        let _sink = spawn_server(&path);
        let mut client = IpcClient::connect(&path).expect("connect");
        let response = client.call("nope", serde_json::Value::Null).unwrap();
        assert!(response.result.is_none());
        assert!(response.error.unwrap().contains("unknown method"));
    }

    #[test]
    fn f1b2_server_pushes_events_to_a_listening_client() {
        let path = temp_socket_path("events");
        let sink = spawn_server(&path);
        let mut listener = IpcClient::connect(&path).expect("connect listener");

        // wait until the server has registered our connection for broadcast.
        let deadline = Instant::now() + Duration::from_secs(2);
        while sink.client_count() == 0 {
            if Instant::now() > deadline {
                panic!("client never registered for events");
            }
            thread::sleep(Duration::from_millis(5));
        }

        sink.broadcast(&IpcEvent {
            event: "context://context-updated".to_string(),
            payload: serde_json::json!({ "app_name": "Pages" }),
        });

        let event = listener.read_event().unwrap().expect("event frame");
        assert_eq!(event.event, "context://context-updated");
        assert_eq!(event.payload["app_name"], "Pages");
    }

    #[test]
    fn f1b2_frame_round_trips_preserve_bytes() {
        let mut buf = Vec::new();
        let payload = b"{\"hello\":\"world\"}";
        write_frame(&mut buf, payload).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let read = read_frame(&mut cursor).unwrap().unwrap();
        assert_eq!(read, payload);
        // a clean end-of-stream yields None, not an error.
        assert!(read_frame(&mut cursor).unwrap().is_none());
    }
}
