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
use std::net::TcpStream;

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
const RENDEZVOUS_TOKEN_KEY: &str = "companion_rendezvous_token";
const RELAY_URL_KEY: &str = "companion_relay_url";

// apex f3b: the rendezvous relay protocol. the daemon and the client both dial the
// relay and announce a role and an opaque rendezvous token on a single header
// line; the relay matches the pair by token and then forwards raw bytes both ways,
// never parsing the ciphertext that follows. the token is a random per-daemon
// routing id -- it identifies a meeting point, not the user.
pub const RENDEZVOUS_MAGIC: &str = "JEFFRDV1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendezvousRole {
    // the daemon/app that owns the core waits to serve a session.
    Host,
    // the companion that connects to it.
    Guest,
}

impl RendezvousRole {
    fn as_str(self) -> &'static str {
        match self {
            RendezvousRole::Host => "host",
            RendezvousRole::Guest => "guest",
        }
    }
}

// the one header line a party sends the relay before any ciphertext.
pub fn write_rendezvous_header<W: Write>(w: &mut W, role: RendezvousRole, token: &str) -> Result<()> {
    if token.is_empty() || token.contains(char::is_whitespace) {
        bail!("invalid rendezvous token");
    }
    write!(w, "{RENDEZVOUS_MAGIC} {} {}\n", role.as_str(), token)?;
    w.flush()?;
    Ok(())
}

// dial the relay and announce our role + token; the returned stream is ready for
// the Noise handshake once the relay pairs us with the peer.
pub fn dial_relay(addr: &str, role: RendezvousRole, token: &str) -> Result<TcpStream> {
    let mut stream = TcpStream::connect(addr)
        .with_context(|| format!("failed to connect to companion relay at {addr}"))?;
    write_rendezvous_header(&mut stream, role, token)?;
    Ok(stream)
}
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

// apex f3c: audio remoting. a voice turn streams AudioFrames (opus/pcm) over the
// same Noise session, so audio is encrypted and relayed as opaque ciphertext like
// every other message. bound memory so a peer cannot stream unbounded audio: cap a
// single frame's raw payload, and cap the whole turn by frame count and byte total.
const MAX_AUDIO_PAYLOAD: usize = 16 * 1024;
const MAX_AUDIO_FRAMES_PER_TURN: usize = 4096;
const MAX_AUDIO_TURN_BYTES: usize = 8 * 1024 * 1024;
// the live voice bridge (remoting C4's realtime session) is reached only with this
// explicit opt-in, never merely because a key is present -- so tests never touch
// the network regardless of ambient credentials.
const COMPANION_VOICE_LIVE_ENV: &str = "JEFF_COMPANION_VOICE_LIVE";

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

// apex f3c: one chunk of a voice turn's audio stream. the payload is base64 so the
// frame rides the exact same json-over-Noise framing (and therefore the same
// ciphertext-only relay) as every other message; `end` marks the last frame in one
// direction. this is the whole audio wire format -- a real phone client only has to
// produce and consume these frames through the paired session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFrame {
    pub seq: u64,
    pub codec: String,
    #[serde(default)]
    pub sample_rate: u32,
    // base64 of the raw codec payload (opus packet or pcm16 chunk).
    pub payload: String,
    #[serde(default)]
    pub end: bool,
}

impl AudioFrame {
    pub fn new(seq: u64, codec: &str, sample_rate: u32, raw: &[u8], end: bool) -> Self {
        Self { seq, codec: codec.to_string(), sample_rate, payload: b64().encode(raw), end }
    }

    pub fn decoded_payload(&self) -> Result<Vec<u8>> {
        b64().decode(self.payload.trim()).context("decode audio frame payload")
    }
}

// accept one inbound frame into a running voice turn, enforcing the per-frame and
// per-turn budgets. returns the new running byte total, or an error if a cap is hit.
// pure so the budget logic is unit-testable without a socket.
fn accept_audio_frame(frame: &AudioFrame, frames_so_far: usize, bytes_so_far: usize) -> Result<usize> {
    let raw = frame.decoded_payload()?;
    if raw.len() > MAX_AUDIO_PAYLOAD {
        bail!("audio frame payload {} exceeds cap {}", raw.len(), MAX_AUDIO_PAYLOAD);
    }
    if frames_so_far + 1 > MAX_AUDIO_FRAMES_PER_TURN {
        bail!("voice turn exceeded its frame budget ({MAX_AUDIO_FRAMES_PER_TURN})");
    }
    let total = bytes_so_far + raw.len();
    if total > MAX_AUDIO_TURN_BYTES {
        bail!("voice turn exceeded its byte budget ({MAX_AUDIO_TURN_BYTES})");
    }
    Ok(total)
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
        // apex f3c: opening a voice turn. the ack tells the client to start streaming
        // audio frames; serve() then hands the session to the audio-frame loop.
        "voice" => CompanionResponse::ok(
            req.id,
            serde_json::json!({ "voice": "ready", "max_payload": MAX_AUDIO_PAYLOAD }),
        ),
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
// apex f3-hardening: what identity() should do, given the current state of the
// keychain (private key) and the DB (public key + any legacy private key). a pure
// decision so the migration is unit-testable without touching the live Keychain.
#[derive(Debug, PartialEq, Eq)]
enum IdentityAction {
    // the private key is already in the keychain and the public key is in the DB.
    UseKeychain,
    // legacy state: the private key is still in app_settings. move it to the
    // keychain and clear the DB copy, keeping the public key.
    Migrate,
    // no usable identity anywhere -- generate one.
    Generate,
}

fn decide_identity(
    keychain_private: &Option<String>,
    db_private: &Option<String>,
    db_public: &Option<String>,
) -> IdentityAction {
    let has = |v: &Option<String>| v.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);
    if has(keychain_private) && has(db_public) {
        IdentityAction::UseKeychain
    } else if has(db_private) && has(db_public) {
        // the private key lives in the DB from a pre-hardening build. migrate it.
        IdentityAction::Migrate
    } else {
        // includes the corner where the keychain has a private key but the DB lost
        // the public one: without the public half we cannot reconstruct the pair,
        // so regenerate rather than serve a half-identity.
        IdentityAction::Generate
    }
}

// the daemon's Noise static keypair. the private key is resolved from the Keychain;
// the public key from app_settings. on a pre-hardening store the private key is
// migrated out of app_settings into the Keychain on first read.
pub fn identity(store: &TaskStore) -> Result<StaticKeypair> {
    identity_with_key_store(store, &crate::secrets::SystemCompanionKeyStore)
}

fn identity_with_key_store(
    store: &TaskStore,
    keys: &dyn crate::secrets::CompanionKeyStore,
) -> Result<StaticKeypair> {
    let keychain_private = keys.get_companion_private_key()?;
    let db_private = store.get_app_setting(IDENTITY_PRIV_KEY)?;
    let db_public = store.get_app_setting(IDENTITY_PUB_KEY)?;

    match decide_identity(&keychain_private, &db_private, &db_public) {
        IdentityAction::UseKeychain => {
            let private = b64()
                .decode(keychain_private.unwrap().trim())
                .context("decode companion private key")?;
            let public = b64()
                .decode(db_public.unwrap().trim())
                .context("decode companion public key")?;
            Ok(StaticKeypair { private, public })
        }
        IdentityAction::Migrate => {
            let priv_b64 = db_private.unwrap();
            let pub_b64 = db_public.unwrap();
            let private = b64().decode(priv_b64.trim()).context("decode companion private key")?;
            let public = b64().decode(pub_b64.trim()).context("decode companion public key")?;
            // move the secret to the keychain, then clear the DB copy. keep public.
            keys.set_companion_private_key(priv_b64.trim())?;
            store.delete_app_setting(IDENTITY_PRIV_KEY)?;
            Ok(StaticKeypair { private, public })
        }
        IdentityAction::Generate => {
            let keypair = generate_static_keypair()?;
            keys.set_companion_private_key(&b64().encode(&keypair.private))?;
            store.set_app_setting(IDENTITY_PUB_KEY, &b64().encode(&keypair.public))?;
            // best-effort: never leave a stale private key behind in the DB.
            let _ = store.delete_app_setting(IDENTITY_PRIV_KEY);
            Ok(keypair)
        }
    }
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

// open a pairing window from `now`. the only mutation begin_pairing makes to the
// pairing state, extracted so callers (and tests) can open a window without
// generating a keychain-backed identity.
pub fn open_pairing_window(store: &TaskStore, now: i64) -> Result<()> {
    store.set_app_setting(PAIRING_OPEN_UNTIL_KEY, &(now + PAIRING_WINDOW_SECONDS).to_string())
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
    // apex f3b: the code also carries the rendezvous token so the device knows the
    // relay meeting point. still public routing material only -- never the private key.
    let token = rendezvous_token(store)?;
    open_pairing_window(store, now)?;
    let code = format!("{}.{}.{}", b64().encode(&keys.public), b64().encode(psk), token);
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
        relay_configured: relay_url(store).is_some(),
    })
}

// apex f3b: the daemon's opaque rendezvous token, generated once and persisted.
// the relay uses it only to match a host with a guest; it carries no identity.
pub fn rendezvous_token(store: &TaskStore) -> Result<String> {
    if let Some(token) = store.get_app_setting(RENDEZVOUS_TOKEN_KEY)? {
        if !token.trim().is_empty() {
            return Ok(token.trim().to_string());
        }
    }
    let mut raw = [0u8; 16];
    getrandom::getrandom(&mut raw).map_err(|err| anyhow!("failed to sample rendezvous token: {err}"))?;
    let token = raw.iter().map(|b| format!("{b:02x}")).collect::<String>();
    store.set_app_setting(RENDEZVOUS_TOKEN_KEY, &token)?;
    Ok(token)
}

// the configured relay address, if the user pointed the companion channel at one.
// none -> remote access is off; the channel only works over a direct/local path.
pub fn relay_url(store: &TaskStore) -> Option<String> {
    store
        .get_app_setting(RELAY_URL_KEY)
        .ok()
        .flatten()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn set_relay_url(store: &TaskStore, url: Option<&str>) -> Result<()> {
    store.set_app_setting(RELAY_URL_KEY, url.unwrap_or("").trim())?;
    Ok(())
}

// apex f3b: serve companion sessions THROUGH the relay. the host dials the relay,
// is matched with a guest, serves exactly one Noise session, and re-dials for the
// next -- so the daemon keeps a presence at the relay without holding the core
// open to it. every gate from F3a still applies: disabled -> no dialing; only a
// paired device that completes the psk handshake is served. the relay only ever
// sees ciphertext.
pub fn serve_one_via_relay(state: &JeffState, relay_addr: &str) -> Result<()> {
    if !is_enabled(&state.store) {
        return Ok(());
    }
    let token = rendezvous_token(&state.store)?;
    let identity = identity(&state.store)?;
    let psk = pairing_psk(&state.store)?;
    let stream = dial_relay(relay_addr, RendezvousRole::Host, &token)?;
    let now = chrono::Utc::now().timestamp();
    serve(stream, state, &identity.private, &psk, |peer| {
        store_authorize(&state.store, peer, now)
    })
}

// apex f3c: the seam between the audio transport (proven in-repo) and whatever turns
// the caller's audio into Jeff's audio. the transport -- AudioFrames over the Noise
// session -- is complete and tested; the live brain is a realtime voice model,
// hardware/key gated exactly like C4.
pub trait VoiceBridge {
    // consume the caller's utterance frames, produce Jeff's reply frames. the last
    // returned frame should carry end=true (serve_voice_turn terminates the stream
    // defensively if it does not).
    fn respond(&self, inbound: &[AudioFrame]) -> Result<Vec<AudioFrame>>;
}

// the in-repo default: deterministic, no network. it proves the pipe by echoing the
// caller's audio back frame for frame, in order, re-stamping the sequence and the
// end marker from the responder's side -- so a test can assert exact round-trip
// integrity and ordering with no audio hardware. (a real deployment replaces this
// with the realtime bridge; the transport underneath is identical.)
pub struct LoopbackVoiceBridge;

impl VoiceBridge for LoopbackVoiceBridge {
    fn respond(&self, inbound: &[AudioFrame]) -> Result<Vec<AudioFrame>> {
        if inbound.is_empty() {
            // an empty utterance still gets a clean end-marked reply so the turn closes.
            return Ok(vec![AudioFrame::new(0, "pcm16", 24000, &[], true)]);
        }
        let n = inbound.len();
        inbound
            .iter()
            .enumerate()
            .map(|(i, frame)| {
                let raw = frame.decoded_payload()?;
                Ok(AudioFrame::new(i as u64, &frame.codec, frame.sample_rate, &raw, i + 1 == n))
            })
            .collect()
    }
}

// the live bridge: remotes C4's realtime voice session over the companion channel.
// its audio path is realtime-audio + key gated AND behind an explicit opt-in, so a
// test never reaches it; absent the opt-in, serve() uses the loopback bridge. the
// full implementation streams inbound frames into a RealtimeVoiceSession and streams
// the model's audio deltas back out -- the same posture as C4, not exercised in-repo.
pub struct RealtimeVoiceBridge;

impl VoiceBridge for RealtimeVoiceBridge {
    fn respond(&self, _inbound: &[AudioFrame]) -> Result<Vec<AudioFrame>> {
        bail!(
            "live companion voice requires a realtime session (set {COMPANION_VOICE_LIVE_ENV}=1 with a configured key)"
        )
    }
}

// pick the bridge for a voice turn. loopback by default; the live bridge only under
// an explicit opt-in, so ambient credentials never pull a test onto the network.
fn voice_bridge_for(_state: &JeffState) -> Box<dyn VoiceBridge> {
    if std::env::var(COMPANION_VOICE_LIVE_ENV).as_deref() == Ok("1") {
        Box::new(RealtimeVoiceBridge)
    } else {
        Box::new(LoopbackVoiceBridge)
    }
}

// apex f3c: serve one voice turn after its ack. read the caller's utterance frames
// (bounded by the per-frame and per-turn budgets) until an end-marked frame, hand
// them to the bridge, then stream Jeff's reply frames back. the session returns to
// the request/response loop when this finishes. a budget or decode error ends the
// turn; the caller (serve) logs it and moves on, or the next recv() ends the session.
fn serve_voice_turn<T: Read + Write>(
    session: &mut CompanionSession<T>,
    bridge: &dyn VoiceBridge,
) -> Result<()> {
    let mut inbound: Vec<AudioFrame> = Vec::new();
    let mut turn_bytes = 0usize;
    loop {
        let bytes = session.recv()?;
        let frame: AudioFrame = serde_json::from_slice(&bytes).context("malformed audio frame")?;
        turn_bytes = accept_audio_frame(&frame, inbound.len(), turn_bytes)?;
        let end = frame.end;
        inbound.push(frame);
        if end {
            break;
        }
    }

    let mut reply = bridge.respond(&inbound)?;
    // never send a frame that would blow the peer's own per-frame cap.
    for frame in &reply {
        if frame.decoded_payload()?.len() > MAX_AUDIO_PAYLOAD {
            bail!("reply audio frame exceeds payload cap");
        }
    }
    // guarantee the stream is terminated even if the bridge forgot an end marker.
    if reply.last().map(|f| f.end) != Some(true) {
        reply.push(AudioFrame::new(reply.len() as u64, "pcm16", 24000, &[], true));
    }
    for frame in &reply {
        session.send(&serde_json::to_vec(frame)?)?;
    }
    Ok(())
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
        let is_voice_open = request.method == "voice" && response.error.is_none();
        session.send(&serde_json::to_vec(&response)?)?;
        // apex f3c: a successful "voice" ack opens an audio-frame turn. hand the
        // session to the audio loop for the duration of the turn, then resume the
        // request/response loop. a mid-turn error ends the turn only; the next recv
        // either serves the next call or ends the session on a closed socket.
        if is_voice_open {
            let bridge = voice_bridge_for(state);
            if let Err(err) = serve_voice_turn(&mut session, bridge.as_ref()) {
                eprintln!("[jeff companion] voice turn ended: {err:#}");
            }
        }
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

    // apex f3c: drive one voice turn. open it (the responder acks on the normal
    // request/response channel), stream the caller's frames up with the last marked
    // end=true, then read Jeff's reply frames until an end-marked frame. this proves
    // the audio transport end to end and runs unchanged through the F3b relay. a real
    // phone client is exactly this loop over live mic/speaker audio.
    pub fn voice_turn(&mut self, inbound: &[AudioFrame]) -> Result<Vec<AudioFrame>> {
        let ack = self.call("voice", serde_json::json!({}))?;
        if ack.get("voice").and_then(|v| v.as_str()) != Some("ready") {
            bail!("companion did not open a voice turn");
        }

        // stream the utterance up. always send at least one end-marked frame so the
        // responder's inbound loop terminates, even for an empty utterance.
        if inbound.is_empty() {
            let f = AudioFrame::new(0, "pcm16", 24000, &[], true);
            self.session.send(&serde_json::to_vec(&f)?)?;
        } else {
            let n = inbound.len();
            for (i, frame) in inbound.iter().enumerate() {
                if frame.decoded_payload()?.len() > MAX_AUDIO_PAYLOAD {
                    bail!("outbound audio frame exceeds payload cap");
                }
                let mut f = frame.clone();
                f.end = i + 1 == n;
                self.session.send(&serde_json::to_vec(&f)?)?;
            }
        }

        // read the reply stream until an end-marked frame, bounded like the responder.
        let mut out = Vec::new();
        loop {
            let bytes = self.session.recv()?;
            let frame: AudioFrame = serde_json::from_slice(&bytes)?;
            let end = frame.end;
            out.push(frame);
            if end {
                break;
            }
            if out.len() > MAX_AUDIO_FRAMES_PER_TURN {
                bail!("reply stream exceeded its frame budget");
            }
        }
        Ok(out)
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
        // open a pairing window -> the unknown device enrolls. (open the window
        // directly rather than via begin_pairing, which mints a keychain-backed
        // identity; this test is about the authorization policy, not pairing codes.)
        open_pairing_window(&store, now).unwrap();
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

    // ---- f3b: through the relay -------------------------------------------------

    use std::net::{Shutdown, TcpListener, TcpStream};

    // a rendezvous relay that matches a host and a guest by token, forwards raw
    // bytes both ways, and RECORDS everything it forwards -- so a test can prove the
    // relay only ever carried ciphertext. this is the in-process stand-in for the
    // deployed Node relay; both speak the same JEFFRDV1 header.
    fn recording_relay() -> (String, std::sync::Arc<std::sync::Mutex<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let recorded = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let rec = recorded.clone();
        std::thread::spawn(move || {
            let mut parties = Vec::new();
            for _ in 0..2 {
                let (mut s, _) = listener.accept().unwrap();
                let token = read_header(&mut s);
                parties.push((s, token));
            }
            // the test uses a single token, so the two parties are the pair.
            assert_eq!(parties[0].1, parties[1].1, "relay matched mismatched tokens");
            let a = parties.remove(0).0;
            let b = parties.remove(0).0;
            let a2 = a.try_clone().unwrap();
            let b2 = b.try_clone().unwrap();
            let rec1 = rec.clone();
            let rec2 = rec.clone();
            let t1 = std::thread::spawn(move || copy_record(a, b2, rec1));
            let t2 = std::thread::spawn(move || copy_record(b, a2, rec2));
            let _ = t1.join();
            let _ = t2.join();
        });
        (addr, recorded)
    }

    // read exactly the one JEFFRDV1 header line (one byte at a time so the peer's
    // first ciphertext bytes are left in the socket for forwarding). returns token.
    fn read_header(s: &mut TcpStream) -> String {
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            if s.read_exact(&mut byte).is_err() {
                break;
            }
            if byte[0] == b'\n' {
                break;
            }
            line.push(byte[0]);
        }
        let header = String::from_utf8_lossy(&line);
        let parts: Vec<&str> = header.split(' ').collect();
        assert_eq!(parts.first().copied(), Some(RENDEZVOUS_MAGIC), "bad rendezvous magic");
        parts.get(2).copied().unwrap_or_default().to_string()
    }

    fn copy_record(
        mut from: TcpStream,
        mut to: TcpStream,
        recorded: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    ) {
        let mut buf = [0u8; 4096];
        loop {
            match from.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    recorded.lock().unwrap().extend_from_slice(&buf[..n]);
                    if to.write_all(&buf[..n]).is_err() {
                        break;
                    }
                    let _ = to.flush();
                }
                Err(_) => break,
            }
        }
        let _ = to.shutdown(Shutdown::Write);
    }

    #[test]
    fn f3b_session_works_through_the_relay_which_sees_only_ciphertext() {
        // the Pillar 14 exit criterion, end to end: a companion session completed
        // through the relay works, and everything the relay forwarded is ciphertext
        // -- a secret marker in the request never appears on the relay's wire.
        let (relay_addr, recorded) = recording_relay();
        let psk = generate_pairing_secret().unwrap();
        let server_keys = generate_static_keypair().unwrap();
        let client_keys = generate_static_keypair().unwrap();
        let token = "rdv-test-token".to_string();

        let server_priv = server_keys.private.clone();
        let server_psk = psk;
        let host_addr = relay_addr.clone();
        let host_token = token.clone();
        let server = std::thread::spawn(move || {
            let (_dir, state) = test_state();
            let stream = dial_relay(&host_addr, RendezvousRole::Host, &host_token).unwrap();
            let _ = serve(stream, &state, &server_priv, &server_psk, |_p| AuthDecision::Enroll);
        });

        let client_stream = dial_relay(&relay_addr, RendezvousRole::Guest, &token).unwrap();
        let mut client = CompanionClient::connect(client_stream, &client_keys.private, &psk).unwrap();
        assert_eq!(client.remote_static(), server_keys.public.as_slice());

        let secret_marker = "WHAT-DID-SARAH-SAY-ABOUT-THE-TIMELINE";
        // recall over memory-disabled store returns no items, but the marker rides
        // through the relay inside the encrypted request.
        let result = client
            .call("recall", serde_json::json!({ "query": secret_marker }))
            .unwrap();
        assert!(result["items"].is_array());

        drop(client);
        server.join().unwrap();

        let wire = recorded.lock().unwrap().clone();
        assert!(!wire.is_empty(), "no bytes were forwarded through the relay");
        assert!(
            !contains_subslice(&wire, secret_marker.as_bytes()),
            "the relay must only ever see ciphertext"
        );
    }

    // ---- apex f3-hardening: private key lives in the Keychain, not the DB -----

    // an in-memory CompanionKeyStore double so identity migration is testable with
    // no real Keychain (the SystemCompanionKeyStore path is never touched here,
    // exactly like the OpenAiKeyStore tests never touch the live keychain).
    #[derive(Default)]
    struct MockKeyStore {
        private: std::sync::Mutex<Option<String>>,
        read_fails: bool,
    }

    impl crate::secrets::CompanionKeyStore for MockKeyStore {
        fn get_companion_private_key(&self) -> anyhow::Result<Option<String>> {
            if self.read_fails {
                return Err(anyhow!("keychain unavailable"));
            }
            Ok(self.private.lock().unwrap().clone())
        }
        fn set_companion_private_key(&self, private_b64: &str) -> anyhow::Result<()> {
            *self.private.lock().unwrap() = Some(private_b64.to_string());
            Ok(())
        }
        fn delete_companion_private_key(&self) -> anyhow::Result<()> {
            *self.private.lock().unwrap() = None;
            Ok(())
        }
    }

    #[test]
    fn f3hardening_decides_use_migrate_or_generate() {
        let some = || Some("x".to_string());
        let none = || None::<String>;
        // private in keychain + public in DB -> use it as-is.
        assert_eq!(decide_identity(&some(), &none(), &some()), IdentityAction::UseKeychain);
        // legacy: private still in the DB alongside the public key -> migrate.
        assert_eq!(decide_identity(&none(), &some(), &some()), IdentityAction::Migrate);
        // nothing usable -> generate.
        assert_eq!(decide_identity(&none(), &none(), &none()), IdentityAction::Generate);
        // keychain private but no public half to pair it with -> regenerate.
        assert_eq!(decide_identity(&some(), &none(), &none()), IdentityAction::Generate);
        // blank strings count as absent.
        let blank = || Some("   ".to_string());
        assert_eq!(decide_identity(&blank(), &blank(), &blank()), IdentityAction::Generate);
    }

    #[test]
    fn f3hardening_migrates_db_private_key_into_the_keychain_and_clears_the_db_copy() {
        let dir = TempDir::new().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        // seed a pre-hardening store: both halves in app_settings.
        let seeded = generate_static_keypair().unwrap();
        let priv_b64 = b64().encode(&seeded.private);
        let pub_b64 = b64().encode(&seeded.public);
        store.set_app_setting(IDENTITY_PRIV_KEY, &priv_b64).unwrap();
        store.set_app_setting(IDENTITY_PUB_KEY, &pub_b64).unwrap();

        let keys = MockKeyStore::default();
        let resolved = identity_with_key_store(&store, &keys).unwrap();

        // same identity, no rotation.
        assert_eq!(resolved.private, seeded.private);
        assert_eq!(resolved.public, seeded.public);
        // the private key moved into the keychain...
        assert_eq!(keys.private.lock().unwrap().as_deref(), Some(priv_b64.as_str()));
        // ...and the DB copy is gone, while the public key stays put.
        assert_eq!(store.get_app_setting(IDENTITY_PRIV_KEY).unwrap(), None);
        assert_eq!(store.get_app_setting(IDENTITY_PUB_KEY).unwrap().as_deref(), Some(pub_b64.as_str()));

        // idempotent: a second read now takes the UseKeychain path unchanged.
        let again = identity_with_key_store(&store, &keys).unwrap();
        assert_eq!(again.private, seeded.private);
        assert_eq!(again.public, seeded.public);
        assert_eq!(store.get_app_setting(IDENTITY_PRIV_KEY).unwrap(), None);
    }

    #[test]
    fn f3hardening_generates_into_the_keychain_never_the_db() {
        let dir = TempDir::new().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        let keys = MockKeyStore::default();

        let generated = identity_with_key_store(&store, &keys).unwrap();

        // the private key is in the keychain and never written to app_settings.
        let kc = keys.private.lock().unwrap().clone().expect("private key in keychain");
        assert_eq!(b64().decode(kc.trim()).unwrap(), generated.private);
        assert_eq!(store.get_app_setting(IDENTITY_PRIV_KEY).unwrap(), None);
        // the public key is in the DB, as routing material.
        let db_pub = store.get_app_setting(IDENTITY_PUB_KEY).unwrap().expect("public key in DB");
        assert_eq!(b64().decode(db_pub.trim()).unwrap(), generated.public);
    }

    #[test]
    fn f3hardening_uses_keychain_without_rewriting_the_db_private() {
        let dir = TempDir::new().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        let kp = generate_static_keypair().unwrap();
        // post-migration steady state: private in keychain, public in DB.
        store.set_app_setting(IDENTITY_PUB_KEY, &b64().encode(&kp.public)).unwrap();
        let keys = MockKeyStore::default();
        *keys.private.lock().unwrap() = Some(b64().encode(&kp.private));

        let resolved = identity_with_key_store(&store, &keys).unwrap();
        assert_eq!(resolved.private, kp.private);
        assert_eq!(resolved.public, kp.public);
        // no legacy private key was ever introduced into the DB.
        assert_eq!(store.get_app_setting(IDENTITY_PRIV_KEY).unwrap(), None);
    }

    #[test]
    fn f3hardening_keychain_read_failure_surfaces_rather_than_forging_an_identity() {
        // a failed keychain read must not silently mint a new identity (which would
        // orphan paired devices); it surfaces as an error the caller can handle.
        let dir = TempDir::new().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        let keys = MockKeyStore {
            private: std::sync::Mutex::new(None),
            read_fails: true,
        };
        assert!(identity_with_key_store(&store, &keys).is_err());
    }

    // ---- apex f3c: audio remoting over the companion protocol -----------------

    // build a paired server thread + connected reference client over a socketpair,
    // seeding one job so the post-turn request/response path has something to return.
    fn paired_voice_pair() -> (std::thread::JoinHandle<()>, CompanionClient<UnixStream>) {
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
                     VALUES (?1, 'check citations', '[]', '{}', 'completed', 0,
                             strftime('%Y-%m-%dT%H:%M:%fZ','now'), strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
                    rusqlite::params![task.id],
                )
                .unwrap();
            let _ = serve(server_io, &state, &server_priv, &server_psk, |_p| AuthDecision::Enroll);
        });

        let client = CompanionClient::connect(client_io, &client_keys.private, &psk).unwrap();
        (server, client)
    }

    #[test]
    fn f3c_voice_turn_round_trips_audio_frames_over_the_encrypted_session() {
        // stream a synthetic utterance up; the loopback bridge echoes it back frame
        // for frame, in order, with a responder-stamped end marker. then a normal
        // request/response call still works -- the turn returned the session cleanly.
        let (server, mut client) = paired_voice_pair();

        let utterance = vec![
            AudioFrame::new(0, "opus", 48000, b"frame-one", false),
            AudioFrame::new(1, "opus", 48000, b"frame-two", false),
            AudioFrame::new(2, "opus", 48000, b"frame-three", true),
        ];
        let reply = client.voice_turn(&utterance).unwrap();

        assert_eq!(reply.len(), 3, "one reply frame per inbound frame");
        for (i, frame) in reply.iter().enumerate() {
            assert_eq!(frame.seq, i as u64, "reply frames are re-stamped in order");
            assert_eq!(frame.codec, "opus");
            assert_eq!(
                frame.decoded_payload().unwrap(),
                utterance[i].decoded_payload().unwrap(),
                "loopback preserves the audio payload"
            );
        }
        assert!(reply.last().unwrap().end, "the reply stream is end-marked");
        assert!(!reply[0].end && !reply[1].end, "only the last frame ends the stream");

        // the session survives the turn: a normal call still answers.
        let jobs = client.call("jobs", serde_json::json!({})).unwrap();
        assert_eq!(jobs["jobs"].as_array().unwrap().len(), 1);

        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn f3c_empty_utterance_still_closes_the_turn() {
        // a voice turn with no audio must not hang: the client sends a lone end
        // frame and the bridge answers with a single end-marked frame.
        let (server, mut client) = paired_voice_pair();
        let reply = client.voice_turn(&[]).unwrap();
        assert_eq!(reply.len(), 1);
        assert!(reply[0].end);
        // and the session is still usable afterward.
        let jobs = client.call("jobs", serde_json::json!({})).unwrap();
        assert_eq!(jobs["jobs"].as_array().unwrap().len(), 1);
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn f3c_audio_frames_are_ciphertext_on_the_wire() {
        // the F3a/F3b confidentiality property, now for audio: a marker embedded in
        // an audio payload never appears in the bytes the session puts on the wire.
        let psk = generate_pairing_secret().unwrap();
        let server_keys = generate_static_keypair().unwrap();
        let client_keys = generate_static_keypair().unwrap();
        let (server_io, client_io) = UnixStream::pair().unwrap();

        let server_priv = server_keys.private.clone();
        let server_psk = psk;
        let server = std::thread::spawn(move || {
            establish_responder(server_io, &server_priv, &server_psk).map(|mut s| s.recv().unwrap())
        });
        let mut client = establish_initiator(client_io, &client_keys.private, &psk).unwrap();

        let secret_marker = b"SARAH-VOICE-NOTE-PRIVATE";
        let frame = AudioFrame::new(0, "pcm16", 24000, secret_marker, true);
        let plaintext = serde_json::to_vec(&frame).unwrap();

        client.send(&plaintext).unwrap();
        let received = server.join().unwrap().unwrap();
        assert_eq!(received, plaintext, "the frame decrypts to exactly what was sent");

        // seal the same frame and confirm the raw ciphertext carries no marker.
        let mut sealed = vec![0u8; plaintext.len() + 16];
        let n = client.transport.write_message(&plaintext, &mut sealed).unwrap();
        assert!(
            !contains_subslice(&sealed[..n], secret_marker),
            "audio payload marker must never appear on the wire"
        );
    }

    #[test]
    fn f3c_audio_budget_rejects_oversized_and_overlong_turns() {
        // the per-frame and per-turn caps that bound the responder's memory.
        let ok = AudioFrame::new(0, "pcm16", 24000, &[0u8; 1024], false);
        assert_eq!(accept_audio_frame(&ok, 0, 0).unwrap(), 1024);

        // a single frame over the payload cap is refused.
        let huge = AudioFrame::new(0, "pcm16", 24000, &vec![0u8; MAX_AUDIO_PAYLOAD + 1], false);
        assert!(accept_audio_frame(&huge, 0, 0).is_err());

        // too many frames in one turn is refused.
        assert!(accept_audio_frame(&ok, MAX_AUDIO_FRAMES_PER_TURN, 0).is_err());

        // exceeding the per-turn byte budget is refused.
        assert!(accept_audio_frame(&ok, 1, MAX_AUDIO_TURN_BYTES).is_err());
    }

    #[test]
    fn f3c_live_voice_bridge_is_gated_off_by_default() {
        // without the explicit opt-in, serve() uses the deterministic loopback
        // bridge -- ambient credentials never pull a turn onto the network. and the
        // live bridge, if reached, refuses without a realtime session rather than
        // silently degrading.
        let (_dir, state) = test_state();
        // no JEFF_COMPANION_VOICE_LIVE in the test env -> loopback.
        let bridge = voice_bridge_for(&state);
        let echoed = bridge
            .respond(&[AudioFrame::new(0, "opus", 48000, b"hi", true)])
            .unwrap();
        assert_eq!(echoed.len(), 1);
        assert_eq!(echoed[0].decoded_payload().unwrap(), b"hi");
        // the live bridge itself is a gated seam.
        assert!(RealtimeVoiceBridge.respond(&[]).is_err());
    }
}
