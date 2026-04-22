// phase 12: cancellation token primitives and stream event payload types.
// every streaming turn creates an InteractionToken. all async tasks for
// that turn hold a child token; dropping or cancelling the root propagates
// to children within one tokio scheduling tick (<30ms typically).

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

// turn ids are short, monotonic strings. no uuid dep needed.
static TURN_COUNTER: AtomicU64 = AtomicU64::new(1);

pub type TurnId = String;

pub fn new_turn_id() -> TurnId {
    format!("t{}", TURN_COUNTER.fetch_add(1, Ordering::SeqCst))
}

// ---- InteractionToken -------------------------------------------------------

#[derive(Clone)]
pub struct InteractionToken {
    pub turn_id: TurnId,
    pub cancel: CancellationToken,
    cancel_reason: Arc<Mutex<String>>,
}

impl InteractionToken {
    pub fn new(turn_id: TurnId) -> Self {
        Self {
            cancel: CancellationToken::new(),
            cancel_reason: Arc::new(Mutex::new("explicit".to_string())),
            turn_id,
        }
    }

    // child token shares the parent's turn_id and is cancelled when the
    // parent is cancelled. used to propagate cancel into spawned sub-tasks.
    #[allow(dead_code)]
    pub fn child(&self) -> Self {
        Self {
            turn_id: self.turn_id.clone(),
            cancel: self.cancel.child_token(),
            cancel_reason: self.cancel_reason.clone(),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    #[allow(dead_code)]
    pub fn cancel(&self) {
        self.cancel_with_reason("explicit");
    }

    pub fn cancel_with_reason(&self, reason: &str) {
        let mut guard = self
            .cancel_reason
            .lock()
            .expect("interaction token reason lock poisoned");
        *guard = normalize_cancel_reason(reason);
        self.cancel.cancel();
    }

    pub fn cancellation_reason(&self) -> String {
        self.cancel_reason
            .lock()
            .expect("interaction token reason lock poisoned")
            .clone()
    }
}

// ---- InteractionRegistry ----------------------------------------------------

// maps active turn_ids to their root cancellation tokens. used by
// cancel_streaming_turn and barge-in handlers to abort in-flight turns.
#[derive(Default)]
pub struct InteractionRegistry {
    active: Mutex<HashMap<TurnId, InteractionToken>>,
}

impl InteractionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, token: &InteractionToken) {
        let mut map = self.active.lock().expect("registry lock poisoned");
        map.insert(token.turn_id.clone(), token.clone());
    }

    // returns true if the turn was found and cancelled.
    pub fn cancel(&self, turn_id: &str, reason: Option<&str>) -> bool {
        let map = self.active.lock().expect("registry lock poisoned");
        if let Some(token) = map.get(turn_id) {
            token.cancel_with_reason(reason.unwrap_or("explicit"));
            true
        } else {
            false
        }
    }

    pub fn remove(&self, turn_id: &str) {
        let mut map = self.active.lock().expect("registry lock poisoned");
        map.remove(turn_id);
    }

    // cancels all active turns. used on app shutdown and barge-in-all paths.
    #[allow(dead_code)]
    pub fn cancel_all(&self) {
        let map = self.active.lock().expect("registry lock poisoned");
        for token in map.values() {
            token.cancel_with_reason("explicit");
        }
    }

    #[allow(dead_code)]
    pub fn active_count(&self) -> usize {
        self.active.lock().expect("registry lock poisoned").len()
    }
}

// ---- stream event payload types ---------------------------------------------
// these structs are the canonical payload shapes for `stream://` Tauri events.
// frontend streamClient.ts mirrors these types.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmTokenPayload {
    pub turn_id: TurnId,
    pub delta: String,
    pub index: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmCompletePayload {
    pub turn_id: TurnId,
    pub full_text: String,
    pub cancelled: bool,
    pub ttft_ms: Option<u64>,
    pub total_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsChunkPayload {
    pub turn_id: TurnId,
    pub phrase_id: u32,
    // base64-encoded mp3 bytes for this phrase (assembled server-side)
    pub audio_b64: String,
    pub first_audio_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnCancelledPayload {
    pub turn_id: TurnId,
    // "user_barge_in" | "jeff_barge_in" | "explicit" | "error"
    pub reason: String,
    pub partial_text: String,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnCompletePayload {
    pub turn_id: TurnId,
    pub duration_ms: u64,
    pub ttft_ms: Option<u64>,
    pub first_audio_ms: Option<u64>,
}

// event name constants. compile-time safety avoids string typos.
pub const EVENT_LLM_TOKEN: &str = "stream://llm_token";
pub const EVENT_LLM_COMPLETE: &str = "stream://llm_complete";
pub const EVENT_TTS_CHUNK: &str = "stream://tts_chunk";
pub const EVENT_TURN_CANCELLED: &str = "stream://turn_cancelled";
pub const EVENT_TURN_COMPLETE: &str = "stream://turn_complete";

// ---- Arc wrapper for easy sharing in state ----------------------------------

pub type SharedRegistry = Arc<InteractionRegistry>;

pub fn new_shared_registry() -> SharedRegistry {
    Arc::new(InteractionRegistry::new())
}

fn normalize_cancel_reason(reason: &str) -> String {
    let trimmed = reason.trim();
    if trimmed.is_empty() {
        return "explicit".to_string();
    }
    match trimmed {
        "user_barge_in" | "jeff_barge_in" | "explicit" | "error" => trimmed.to_string(),
        _ => "explicit".to_string(),
    }
}

// ---- tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_ids_are_unique_and_monotonic() {
        let a = new_turn_id();
        let b = new_turn_id();
        assert_ne!(a, b);
        // both start with 't' followed by a number
        assert!(a.starts_with('t'));
        assert!(b.starts_with('t'));
        let a_num: u64 = a[1..].parse().expect("not a number");
        let b_num: u64 = b[1..].parse().expect("not a number");
        assert!(b_num > a_num);
    }

    #[test]
    fn token_cancel_propagates_to_child() {
        let root = InteractionToken::new("t1".to_string());
        let child = root.child();

        assert!(!root.is_cancelled());
        assert!(!child.is_cancelled());

        root.cancel();

        assert!(root.is_cancelled());
        assert!(child.is_cancelled());
        assert_eq!(root.cancellation_reason(), "explicit");
        assert_eq!(child.cancellation_reason(), "explicit");
    }

    #[test]
    fn cancelling_child_does_not_cancel_parent() {
        let root = InteractionToken::new("t2".to_string());
        let child = root.child();

        child.cancel();

        assert!(child.is_cancelled());
        assert!(!root.is_cancelled());
    }

    #[test]
    fn cancel_with_reason_is_shared_with_children() {
        let root = InteractionToken::new("t3".to_string());
        let child = root.child();
        root.cancel_with_reason("user_barge_in");
        assert_eq!(root.cancellation_reason(), "user_barge_in");
        assert_eq!(child.cancellation_reason(), "user_barge_in");
    }

    #[test]
    fn registry_cancel_returns_true_for_known_turn() {
        let registry = InteractionRegistry::new();
        let token = InteractionToken::new("t10".to_string());
        registry.register(&token);

        assert_eq!(registry.active_count(), 1);
        assert!(registry.cancel("t10", None));
        assert!(token.is_cancelled());
        assert_eq!(token.cancellation_reason(), "explicit");
    }

    #[test]
    fn registry_cancel_accepts_explicit_reason() {
        let registry = InteractionRegistry::new();
        let token = InteractionToken::new("t11".to_string());
        registry.register(&token);

        assert!(registry.cancel("t11", Some("jeff_barge_in")));
        assert!(token.is_cancelled());
        assert_eq!(token.cancellation_reason(), "jeff_barge_in");
    }

    #[test]
    fn registry_cancel_returns_false_for_unknown_turn() {
        let registry = InteractionRegistry::new();
        assert!(!registry.cancel("unknown", None));
    }

    #[test]
    fn registry_remove_cleans_up_entry() {
        let registry = InteractionRegistry::new();
        let token = InteractionToken::new("t20".to_string());
        registry.register(&token);
        assert_eq!(registry.active_count(), 1);
        registry.remove("t20");
        assert_eq!(registry.active_count(), 0);
    }

    #[test]
    fn cancel_all_cancels_every_registered_token() {
        let registry = InteractionRegistry::new();
        let t1 = InteractionToken::new("t30".to_string());
        let t2 = InteractionToken::new("t31".to_string());
        registry.register(&t1);
        registry.register(&t2);

        registry.cancel_all();

        assert!(t1.is_cancelled());
        assert!(t2.is_cancelled());
    }
}
