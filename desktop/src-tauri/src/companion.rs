// apex f3a: the end-to-end encrypted companion channel.
//
// a companion (a phone/earbud client -- F3c; a reference client here) talks to the
// process that owns the store-backed core -- the daemon when present, the app
// otherwise -- over a Noise-encrypted session. the session carries only the small
// store-backed surface the earbud scenes need:
//   turn   -- an utterance, answered by Jeff through the same pipeline and memory
//             as the desktop ("remind me what Sarah said about the timeline")
//   recall -- a direct memory lookup
//   jobs   -- background job status
// it deliberately does NOT expose the full command table (the F1b-3 scope note).
//
// end-to-end by construction: the transport under this session (a relay -- F3b)
// only ever sees Noise ciphertext, so the world model never leaves the Mac. only a
// PAIRED device can open a session: the handshake is Noise_XXpsk3, gated by the
// pairing secret (the psk) AND mutually authenticating static keys, and the daemon
// additionally authorizes the remote static key against its paired-device list.
//
// controls precede capability: companion access is OFF until the user enables it
// and pairs a device; unpairing/disabling refuses new sessions.

use std::io::{Read, Write};

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::chat::send_message_for_task;
use crate::memory;
use crate::state::JeffState;
use crate::store::TaskStore;

const ENABLED_KEY: &str = "companion_enabled";
const IDENTITY_PRIV_KEY: &str = "companion_static_private_b64";
const IDENTITY_PUB_KEY: &str = "companion_static_public_b64";
const PAIRING_PSK_KEY: &str = "companion_pairing_psk_b64";
const PAIRING_OPEN_UNTIL_KEY: &str = "companion_pairing_open_until";
// how long a pairing window stays open after the user asks to pair a device.
pub const PAIRING_WINDOW_SECONDS: i64 = 5 * 60;

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

// mutual static-key auth + forward secrecy, gated by the pairing pre-shared key.
const NOISE_PARAMS: &str = "Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s";
pub const COMPANION_PROTOCOL: u32 = 1;

// noise transport messages are capped at 65535 bytes including the 16-byte tag.
// companion requests/responses are small json; cap plaintext well under that and
// reject anything larger rather than silently truncating.
const MAX_PLAINTEXT: usize = 63 * 1024;
const MAX_FRAME: usize = 65535;

// ---- keys and pairing --------------------------------------------------------

// a curve25519 static keypair. the private half never leaves this machine.
#[derive(Debug, Clone)]
pub struct StaticKeypair {
    pub private: Vec<u8>,
    pub public: Vec<u8>,
}

pub fn generate_static_keypair() -> Result<StaticKeypair> {
    let params = NOISE_PARAMS.parse().map_err(|err| anyhow!("noise params: {err}"))?;
    let keypair = snow::Builder::new(params)
        .generate_keypair()
        .context("failed to generate companion keypair")?;
    Ok(StaticKeypair {
        private: keypair.private,
        public: keypair.public,
    })
}

// the pairing secret shared out-of-band (a QR/code shown on the Mac). 32 bytes of
// entropy, used as the Noise psk so an unpaired party cannot complete a handshake.
pub fn generate_pairing_secret() -> Result<[u8; 32]> {
    let mut psk = [0u8; 32];
    getrandom::getrandom(&mut psk).map_err(|err| anyhow!("failed to sample pairing secret: {err}"))?;
    Ok(psk)
}

// ---- session -----------------------------------------------------------------

// an established, encrypted companion session over some byte transport T.
pub struct CompanionSession<T: Read + Write> {
    transport: snow::TransportState,
    io: T,
    // the peer's authenticated static public key, learned during the handshake.
    remote_static: Vec<u8>,
}

impl<T: Read + Write> CompanionSession<T> {
    pub fn remote_static(&self) -> &[u8] {
        &self.remote_static
    }

    // encrypt `plaintext` into a single Noise message and write it framed.
    pub fn send(&mut self, plaintext: &[u8]) -> Result<()> {
        if plaintext.len() > MAX_PLAINTEXT {
            bail!("companion message too large ({} bytes)", plaintext.len());
        }
        let mut buf = vec![0u8; plaintext.len() + 16];
        let n = self
            .transport
            .write_message(plaintext, &mut buf)
            .map_err(|err| anyhow!("companion encrypt failed: {err}"))?;
        write_frame(&mut self.io, &buf[..n])
    }

    // read one framed Noise message and decrypt it.
    pub fn recv(&mut self) -> Result<Vec<u8>> {
        let frame = read_frame(&mut self.io)?;
        let mut out = vec![0u8; frame.len()];
        let n = self
            .transport
            .read_message(&frame, &mut out)
            .map_err(|err| anyhow!("companion decrypt failed: {err}"))?;
        out.truncate(n);
        Ok(out)
    }
}

// drive the Noise_XXpsk3 handshake as the initiator (the companion/client).
pub fn establish_initiator<T: Read + Write>(
    mut io: T,
    local_private: &[u8],
    psk: &[u8; 32],
) -> Result<CompanionSession<T>> {
    let params = NOISE_PARAMS.parse().map_err(|err| anyhow!("noise params: {err}"))?;
    let mut hs = snow::Builder::new(params)
        .local_private_key(local_private)
        .psk(3, psk)
        .build_initiator()
        .context("failed to build companion initiator")?;

    let mut buf = vec![0u8; MAX_FRAME];
    // XX: -> e
    let n = hs.write_message(&[], &mut buf).map_err(handshake_err)?;
    write_frame(&mut io, &buf[..n])?;
    // <- e, ee, s, es
    let msg = read_frame(&mut io)?;
    hs.read_message(&msg, &mut buf).map_err(handshake_err)?;
    // -> s, se, psk
    let n = hs.write_message(&[], &mut buf).map_err(handshake_err)?;
    write_frame(&mut io, &buf[..n])?;

    finalize(hs, io)
}

// drive the handshake as the responder (the daemon/app that owns the core).
pub fn establish_responder<T: Read + Write>(
    mut io: T,
    local_private: &[u8],
    psk: &[u8; 32],
) -> Result<CompanionSession<T>> {
    let params = NOISE_PARAMS.parse().map_err(|err| anyhow!("noise params: {err}"))?;
    let mut hs = snow::Builder::new(params)
        .local_private_key(local_private)
        .psk(3, psk)
        .build_responder()
        .context("failed to build companion responder")?;

    let mut buf = vec![0u8; MAX_FRAME];
    // -> e
    let msg = read_frame(&mut io)?;
    hs.read_message(&msg, &mut buf).map_err(handshake_err)?;
    // <- e, ee, s, es
    let n = hs.write_message(&[], &mut buf).map_err(handshake_err)?;
    write_frame(&mut io, &buf[..n])?;
    // -> s, se, psk
    let msg = read_frame(&mut io)?;
    hs.read_message(&msg, &mut buf).map_err(handshake_err)?;

    finalize(hs, io)
}

fn finalize<T: Read + Write>(hs: snow::HandshakeState, io: T) -> Result<CompanionSession<T>> {
    let remote_static = hs
        .get_remote_static()
        .ok_or_else(|| anyhow!("handshake produced no peer identity"))?
        .to_vec();
    let transport = hs
        .into_transport_mode()
        .map_err(|err| anyhow!("failed to enter transport mode: {err}"))?;
    Ok(CompanionSession {
        transport,
        io,
        remote_static,
    })
}

// a failed handshake (wrong psk, tampered message, wrong identity) must never be
// reported as anything but a failure -- there is no session, no plaintext.
fn handshake_err(err: snow::Error) -> anyhow::Error {
    anyhow!("companion handshake failed: {err}")
}

// ---- framing -----------------------------------------------------------------

fn write_frame<W: Write>(w: &mut W, bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_FRAME {
        bail!("companion frame too large");
    }
    w.write_all(&(bytes.len() as u32).to_be_bytes())?;
    w.write_all(bytes)?;
    w.flush()?;
    Ok(())
}

fn read_frame<R: Read>(r: &mut R) -> Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        bail!("companion frame length {len} exceeds max");
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

// ---- protocol ----------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompanionRequest {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompanionResponse {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl CompanionResponse {
    fn ok(id: u64, value: serde_json::Value) -> Self {
        Self { id, ok: Some(value), error: None }
    }
    fn err(id: u64, message: impl Into<String>) -> Self {
        Self { id, ok: None, error: Some(message.into()) }
    }
}

// answer one companion request against the store-backed core. this is the entire
// remote surface: a turn, a recall, and job status -- nothing that can mutate the
// world beyond what a desktop chat turn already does, and nothing perception-side.
pub fn dispatch(state: &JeffState, req: &CompanionRequest) -> CompanionResponse {
    match req.method.as_str() {
        "hello" => CompanionResponse::ok(
            req.id,
            serde_json::json!({ "service": "jeff-companion", "protocol": COMPANION_PROTOCOL }),
        ),
        "turn" => match dispatch_turn(state, &req.params) {
            Ok(value) => CompanionResponse::ok(req.id, value),
            Err(err) => CompanionResponse::err(req.id, format!("{err:#}")),
        },
        "recall" => match dispatch_recall(state, &req.params) {
            Ok(value) => CompanionResponse::ok(req.id, value),
            Err(err) => CompanionResponse::err(req.id, format!("{err:#}")),
        },
        "jobs" => match dispatch_jobs(state, &req.params) {
            Ok(value) => CompanionResponse::ok(req.id, value),
            Err(err) => CompanionResponse::err(req.id, format!("{err:#}")),
        },
        other => CompanionResponse::err(req.id, format!("unknown companion method: {other}")),
    }
}

fn resolve_task_id(state: &JeffState, params: &serde_json::Value) -> Result<i64> {
    if let Some(task_id) = params.get("task_id").and_then(|v| v.as_i64()) {
        return Ok(task_id);
    }
    state
        .store
        .get_active_task()?
        .map(|task| task.id)
        .ok_or_else(|| anyhow!("no active task"))
}

// the flagship scene: an utterance answered by Jeff with the same memory, pipeline
// and character as the desktop -- reusing send_message_for_task exactly.
fn dispatch_turn(state: &JeffState, params: &serde_json::Value) -> Result<serde_json::Value> {
    let utterance = params
        .get("utterance")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("turn requires a non-empty utterance"))?;
    let task_id = resolve_task_id(state, params)?;
    let response = send_message_for_task(
        &state.store,
        state.embeddings.as_ref(),
        state.reasoning.as_ref(),
        task_id,
        utterance,
        "companion",
        None,
        None,
        || false,
    )?;
    Ok(serde_json::json!({
        "reply": response.assistant_response,
        "task_id": task_id,
    }))
}

fn dispatch_recall(state: &JeffState, params: &serde_json::Value) -> Result<serde_json::Value> {
    let query = params
        .get("query")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("recall requires a non-empty query"))?;
    let k = params.get("k").and_then(|v| v.as_u64()).unwrap_or(5).clamp(1, 20) as usize;
    if !state
        .store
        .get_privacy_user_profile_memory_enabled()
        .unwrap_or(false)
    {
        return Ok(serde_json::json!({ "items": [] }));
    }
    let embedding = state.embeddings.embed_text(query).unwrap_or_default();
    let items = memory::recall(&state.store, &embedding, k)
        .into_iter()
        .map(|item| {
            serde_json::json!({ "text": item.text, "kind": item.kind, "score": item.score })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::json!({ "items": items }))
}

fn dispatch_jobs(state: &JeffState, params: &serde_json::Value) -> Result<serde_json::Value> {
    let task_id = params.get("task_id").and_then(|v| v.as_i64());
    let jobs = crate::agent_runtime::list_jobs(&state.store, task_id, 20)?
        .into_iter()
        .filter(|job| !job.speculative)
        .map(|job| {
            serde_json::json!({
                "id": job.id,
                "status": job.status,
                "goal": job.goal_contract,
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::json!({ "jobs": jobs }))
}

// ---- server + reference client ----------------------------------------------

// what to do with a peer once its identity is known after the handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthDecision {
    // a known, paired device -> serve it.
    Allow,
    // an unknown device presenting a valid pairing secret during an open pairing
    // window -> enroll it (trust-on-first-use, gated by the psk) then serve.
    Enroll,
    // not permitted (companion disabled, unknown device, pairing window closed).
    Reject,
}

// ---- store-backed identity, enablement, and pairing policy -------------------

// off by default: the channel refuses every session until the user turns it on.
pub fn is_enabled(store: &TaskStore) -> bool {
    store.get_app_setting_bool(ENABLED_KEY).ok().flatten().unwrap_or(false)
}

// the kill switch. disabling also closes any open pairing window so a lingering
// window cannot enroll a device after the user turned the channel off.
pub fn set_enabled(store: &TaskStore, enabled: bool) -> Result<()> {
    store.set_app_setting(ENABLED_KEY, if enabled { "true" } else { "false" })?;
    if !enabled {
        store.set_app_setting(PAIRING_OPEN_UNTIL_KEY, "0")?;
    }
    Ok(())
}

// the daemon/app's own static identity, generated on first use and persisted so
// pairings survive restarts. the private half lives in the local store, the same
// local-first trust boundary as the rest of the world model.
pub fn identity(store: &TaskStore) -> Result<StaticKeypair> {
    if let (Some(priv_b64), Some(pub_b64)) = (
        store.get_app_setting(IDENTITY_PRIV_KEY)?,
        store.get_app_setting(IDENTITY_PUB_KEY)?,
    ) {
        let private = b64().decode(priv_b64.trim()).context("decode companion private key")?;
        let public = b64().decode(pub_b64.trim()).context("decode companion public key")?;
        return Ok(StaticKeypair { private, public });
    }
    let keypair = generate_static_keypair()?;
    store.set_app_setting(IDENTITY_PRIV_KEY, &b64().encode(&keypair.private))?;
    store.set_app_setting(IDENTITY_PUB_KEY, &b64().encode(&keypair.public))?;
    Ok(keypair)
}

// the shared pairing secret (the Noise psk), generated once and reused as the
// account secret. per-device psks are a later hardening; today device revocation
// is the allowlist below.
pub fn pairing_psk(store: &TaskStore) -> Result<[u8; 32]> {
    if let Some(psk_b64) = store.get_app_setting(PAIRING_PSK_KEY)? {
        let bytes = b64().decode(psk_b64.trim()).context("decode pairing psk")?;
        let arr: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("stored pairing psk is not 32 bytes"))?;
        return Ok(arr);
    }
    let psk = generate_pairing_secret()?;
    store.set_app_setting(PAIRING_PSK_KEY, &b64().encode(psk))?;
    Ok(psk)
}

fn pairing_open(store: &TaskStore, now: i64) -> bool {
    store
        .get_app_setting(PAIRING_OPEN_UNTIL_KEY)
        .ok()
        .flatten()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .map(|until| now < until)
        .unwrap_or(false)
}

// open a pairing window and return the material a new device needs: the daemon's
// public identity and the pairing secret, encoded as one code (the "QR"). the
// private key is never included.
pub fn begin_pairing(store: &TaskStore, now: i64) -> Result<crate::models::CompanionPairingDto> {
    let keys = identity(store)?;
    let psk = pairing_psk(store)?;
    store.set_app_setting(PAIRING_OPEN_UNTIL_KEY, &(now + PAIRING_WINDOW_SECONDS).to_string())?;
    let code = format!("{}.{}", b64().encode(&keys.public), b64().encode(psk));
    Ok(crate::models::CompanionPairingDto {
        code,
        expires_in_seconds: PAIRING_WINDOW_SECONDS,
    })
}

// the daemon's authorization policy over a peer's authenticated static key. fails
// closed: any error, a disabled channel, or an unknown device outside a pairing
// window is a rejection.
pub fn store_authorize(store: &TaskStore, remote_static: &[u8], now: i64) -> AuthDecision {
    if !is_enabled(store) {
        return AuthDecision::Reject;
    }
    let pub_b64 = b64().encode(remote_static);
    match store.companion_device_exists(&pub_b64) {
        Ok(true) => {
            let _ = store.touch_companion_device(&pub_b64, now);
            AuthDecision::Allow
        }
        Ok(false) => {
            if pairing_open(store, now) {
                if store.record_companion_device(&pub_b64, "companion", now).is_ok() {
                    AuthDecision::Enroll
                } else {
                    AuthDecision::Reject
                }
            } else {
                AuthDecision::Reject
            }
        }
        Err(_) => AuthDecision::Reject,
    }
}

pub fn status(store: &TaskStore) -> Result<crate::models::CompanionStatusDto> {
    let now = chrono::Utc::now().timestamp();
    Ok(crate::models::CompanionStatusDto {
        enabled: is_enabled(store),
        paired_device_count: store.list_companion_devices()?.len(),
        pairing_open: pairing_open(store, now),
    })
}

// run the responder side end to end: handshake, authorize the peer, then serve the
// store-backed surface until the peer disconnects. `authorize` is the daemon's
// policy over the peer's static key; it keeps this module policy-free and testable.
pub fn serve<T, A>(
    io: T,
    state: &JeffState,
    local_private: &[u8],
    psk: &[u8; 32],
    authorize: A,
) -> Result<()>
where
    T: Read + Write,
    A: Fn(&[u8]) -> AuthDecision,
{
    let mut session = establish_responder(io, local_private, psk)?;
    match authorize(session.remote_static()) {
        AuthDecision::Allow | AuthDecision::Enroll => {}
        AuthDecision::Reject => bail!("companion peer not authorized"),
    }
    loop {
        let bytes = match session.recv() {
            Ok(bytes) => bytes,
            // a closed connection ends the session cleanly.
            Err(_) => return Ok(()),
        };
        let request: CompanionRequest = match serde_json::from_slice(&bytes) {
            Ok(request) => request,
            Err(err) => {
                let response = CompanionResponse::err(0, format!("malformed request: {err}"));
                session.send(&serde_json::to_vec(&response)?)?;
                continue;
            }
        };
        let response = dispatch(state, &request);
        session.send(&serde_json::to_vec(&response)?)?;
    }
}

// the reference companion client. stands in for the phone (F3c): it proves the
// protocol end to end and is what F3b drives through the relay.
pub struct CompanionClient<T: Read + Write> {
    session: CompanionSession<T>,
    next_id: u64,
}

impl<T: Read + Write> CompanionClient<T> {
    pub fn connect(io: T, local_private: &[u8], psk: &[u8; 32]) -> Result<Self> {
        let session = establish_initiator(io, local_private, psk)?;
        let mut client = Self { session, next_id: 1 };
        // confirm the channel actually works before returning. in Noise_XXpsk3 the
        // initiator finishes locally before the responder validates the psk (it is
        // mixed in the final message), so a wrong pairing secret yields a session
        // with mismatched keys that only fails on first use. a hello round-trip
        // makes connect() authoritative: a returned client is a working, paired,
        // protocol-compatible session.
        let hello = client
            .call("hello", serde_json::json!({}))
            .context("companion handshake confirmation failed (wrong pairing secret or not authorized)")?;
        let protocol = hello.get("protocol").and_then(|v| v.as_u64()).unwrap_or(0);
        if protocol != COMPANION_PROTOCOL as u64 {
            bail!("companion protocol mismatch: peer speaks {protocol}, we speak {COMPANION_PROTOCOL}");
        }
        Ok(client)
    }

    pub fn remote_static(&self) -> &[u8] {
        self.session.remote_static()
    }

    pub fn call(&mut self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;
        let request = CompanionRequest { id, method: method.to_string(), params };
        self.session.send(&serde_json::to_vec(&request)?)?;
        let bytes = self.session.recv()?;
        let response: CompanionResponse = serde_json::from_slice(&bytes)?;
        if let Some(error) = response.error {
            bail!("companion call `{method}` failed: {error}");
        }
        response.ok.ok_or_else(|| anyhow!("companion response had no result"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_router::{ModelRouter, RouterConfig};
    use crate::retrieval::default_embeddings_provider;
    use crate::store::TaskStore;
    use crate::voice::OpenAiVoiceProvider;
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn test_state() -> (TempDir, JeffState) {
        let dir = TempDir::new().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        let local_runtime = Arc::new(crate::local_runtime::LocalRuntime::new(dir.path()));
        let embeddings = Arc::new(default_embeddings_provider(local_runtime.clone()));
        let router = Arc::new(ModelRouter::new(RouterConfig::default()));
        let voice = Arc::new(OpenAiVoiceProvider::from_env());
        let state = JeffState::new(store, embeddings, local_runtime, router, voice);
        (dir, state)
    }

    #[test]
    fn f3a_paired_client_reaches_the_same_store_over_an_encrypted_session() {
        // a companion request is answered by the same core against the same store --
        // the job it can see is the job the daemon recorded.
        let psk = generate_pairing_secret().unwrap();
        let server_keys = generate_static_keypair().unwrap();
        let client_keys = generate_static_keypair().unwrap();
        let (server_io, client_io) = UnixStream::pair().unwrap();

        let server_priv = server_keys.private.clone();
        let server_psk = psk;
        let server = std::thread::spawn(move || {
            let (_dir, state) = test_state();
            let task = state.store.create_task("Thesis").unwrap();
            state
                .store
                .connect()
                .unwrap()
                .execute(
                    "INSERT INTO jobs (task_id, goal_contract, plan_json, budget_json, status, speculative, created_at, updated_at)
                     VALUES (?1, 'check citations in chapter 2', '[]', '{}', 'completed', 0,
                             strftime('%Y-%m-%dT%H:%M:%fZ','now'), strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
                    rusqlite::params![task.id],
                )
                .unwrap();
            let _ = serve(server_io, &state, &server_priv, &server_psk, |_peer| AuthDecision::Enroll);
        });

        let mut client = CompanionClient::connect(client_io, &client_keys.private, &psk).unwrap();
        // the client authenticated the server's real static key.
        assert_eq!(client.remote_static(), server_keys.public.as_slice());
        let jobs = client.call("jobs", serde_json::json!({})).unwrap();
        let list = jobs["jobs"].as_array().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["goal"], "check citations in chapter 2");

        drop(client); // closes the socket, ending the serve loop
        server.join().unwrap();
    }

    #[test]
    fn f3a_authorization_is_off_by_default_and_gated_by_the_pairing_window() {
        // controls precede capability: no device is authorized until the channel is
        // enabled AND the device enrolled during an open pairing window. revocation
        // and the kill switch both deny a previously-trusted device.
        let dir = TempDir::new().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        let device = generate_static_keypair().unwrap();
        let now = 1000;

        assert_eq!(store_authorize(&store, &device.public, now), AuthDecision::Reject);
        set_enabled(&store, true).unwrap();
        // enabled but no open pairing window -> an unknown device is rejected.
        assert_eq!(store_authorize(&store, &device.public, now), AuthDecision::Reject);
        // open a pairing window -> the unknown device enrolls.
        begin_pairing(&store, now).unwrap();
        assert_eq!(store_authorize(&store, &device.public, now), AuthDecision::Enroll);
        // now known -> allowed even after the window closes.
        let later = now + PAIRING_WINDOW_SECONDS + 1;
        assert_eq!(store_authorize(&store, &device.public, later), AuthDecision::Allow);
        // a different unknown device after the window is rejected.
        let other = generate_static_keypair().unwrap();
        assert_eq!(store_authorize(&store, &other.public, later), AuthDecision::Reject);
        // revoking the device denies it.
        store.remove_companion_device(&b64().encode(&device.public)).unwrap();
        assert_eq!(store_authorize(&store, &device.public, later), AuthDecision::Reject);
        // and the kill switch denies even a known device.
        store.record_companion_device(&b64().encode(&device.public), "x", later).unwrap();
        set_enabled(&store, false).unwrap();
        assert_eq!(store_authorize(&store, &device.public, later), AuthDecision::Reject);
    }

    #[test]
    fn f3a_wrong_pairing_secret_cannot_open_a_session() {
        // an attacker without the pairing secret must never complete a handshake --
        // no session, therefore no access to the store.
        let good_psk = generate_pairing_secret().unwrap();
        let mut wrong_psk = generate_pairing_secret().unwrap();
        // ensure it differs.
        wrong_psk[0] ^= 0xFF;
        let server_keys = generate_static_keypair().unwrap();
        let client_keys = generate_static_keypair().unwrap();
        let (server_io, client_io) = UnixStream::pair().unwrap();

        let server_priv = server_keys.private.clone();
        let server = std::thread::spawn(move || {
            let (_dir, state) = test_state();
            serve(server_io, &state, &server_priv, &good_psk, |_p| AuthDecision::Enroll).is_ok()
        });

        let client = CompanionClient::connect(client_io, &client_keys.private, &wrong_psk);
        assert!(client.is_err(), "a wrong pairing secret must fail the handshake");
        // the server side must also have failed rather than serving.
        assert!(!server.join().unwrap(), "server must not serve an unpaired peer");
    }

    #[test]
    fn f3a_relay_would_see_only_ciphertext() {
        // the exit-criterion property, tested at the session layer: what a session
        // puts on the wire for a plaintext-carrying message contains no plaintext.
        // (F3b proves the same end to end through the actual relay.)
        let psk = generate_pairing_secret().unwrap();
        let server_keys = generate_static_keypair().unwrap();
        let client_keys = generate_static_keypair().unwrap();
        let (server_io, client_io) = UnixStream::pair().unwrap();

        let server_priv = server_keys.private.clone();
        let server_psk = psk;
        let server = std::thread::spawn(move || {
            establish_responder(server_io, &server_priv, &server_psk)
                .map(|mut s| s.recv().unwrap())
        });

        let mut client =
            establish_initiator(client_io, &client_keys.private, &psk).unwrap();
        let secret_marker = "SARAH-MOVED-THE-REVIEW-TO-1330";
        let plaintext = serde_json::json!({ "utterance": secret_marker }).to_string();

        // the real send goes over the socket; the server decrypts it back to the
        // exact plaintext (confidentiality + integrity through the transport).
        client.send(plaintext.as_bytes()).unwrap();
        let received = server.join().unwrap().unwrap();
        assert_eq!(received, plaintext.as_bytes());

        // and what the session actually put on the wire carries no plaintext: seal
        // the same message and inspect the raw ciphertext bytes for the marker.
        let mut sealed = vec![0u8; plaintext.len() + 16];
        let n = client
            .transport
            .write_message(plaintext.as_bytes(), &mut sealed)
            .unwrap();
        assert!(
            !contains_subslice(&sealed[..n], secret_marker.as_bytes()),
            "plaintext marker must never appear on the wire"
        );
    }

    fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }
}
