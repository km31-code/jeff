// apex c4: realtime voice sessions. full-duplex conversation via the OpenAI
// Realtime API. jeff (backend) mints an ephemeral session with the character +
// situational context and the tool surface; the frontend runs the WebRTC audio
// with that ephemeral secret. transcripts persist as normal chat turns; spoken
// requests route through the same command surface as typed ones; on any failure
// the existing STT/TTS pipeline handles the turn.
//
// the live socket and audio are exercised only with a key + microphone (env-
// gated); the context assembly, tool routing, transcript persistence, and
// fallback decision are pure and unit-tested.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::message_kind::MessageKind;
use crate::providers::realtime::{self, RealtimeCredentials};
use crate::store::TaskStore;

pub const VOICE_ENABLED_KEY: &str = "voice_realtime_enabled";
pub const VOICE_NAME_KEY: &str = "voice_realtime_voice";
pub const VOICE_SOURCE: &str = "voice";

// the lifecycle a voice session moves through. exposed to the frontend so the
// overlay can render a live indicator / mute / end and reflect fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VoiceSessionState {
    Idle,
    Connecting,
    Live,
    Fallback,
    Closed,
}

// the minimal session interface (open + close). the realtime adapter mints
// credentials for the frontend; context refresh and transcript/tool events flow
// back through commands.
pub trait VoiceSession: Send + Sync {
    fn open(&self, instructions: &str) -> Result<RealtimeCredentials>;
    #[allow(dead_code)]
    fn close(&self);
}

// what a spoken request maps to. today the tool surface routes spoken requests
// back through the same text command path ("fix it" spoken = "fix it" typed);
// the Action Bus (D1+) extends this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceAction {
    RouteAsText(String),
    None,
}

// map a realtime tool-call to a voice action.
pub fn route_voice_tool_call(name: &str, arguments: &serde_json::Value) -> VoiceAction {
    if name.trim() == "route_request" {
        if let Some(text) = arguments.get("text").and_then(|value| value.as_str()) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return VoiceAction::RouteAsText(trimmed.to_string());
            }
        }
    }
    VoiceAction::None
}

// assemble the realtime session instructions from the same context blocks the
// text path uses: character, live snapshot, relational context, memory recall.
pub fn build_session_instructions(
    character_block: &str,
    snapshot_summary: Option<&str>,
    relational_context: Option<&str>,
    recall_block: Option<&str>,
) -> String {
    let mut sections: Vec<String> = vec![character_block.trim().to_string()];
    sections.push(
        "You are in a spoken conversation. Keep replies short and conversational. \
When the user asks you to act, call the route_request tool with their request in their words."
            .to_string(),
    );
    if let Some(snapshot) = snapshot_summary.map(str::trim).filter(|s| !s.is_empty()) {
        sections.push(format!("Current situation:\n{snapshot}"));
    }
    if let Some(relational) = relational_context.map(str::trim).filter(|s| !s.is_empty()) {
        sections.push(relational.to_string());
    }
    if let Some(recall) = recall_block.map(str::trim).filter(|s| !s.is_empty()) {
        sections.push(recall.to_string());
    }
    sections.join("\n\n")
}

// persist one side of a voice turn as a normal chat message so memory,
// episodes, and evals see voice sessions exactly like typed ones.
pub fn persist_voice_turn(
    store: &TaskStore,
    task_id: i64,
    role: &str,
    text: &str,
) -> Result<i64> {
    let clean = text.trim();
    if clean.is_empty() {
        return Ok(0);
    }
    let kind = if role == "user" {
        MessageKind::UserStatement
    } else {
        MessageKind::AssistantAnswer
    };
    let inserted = store.append_chat_message(task_id, role, VOICE_SOURCE, kind, clean)?;
    Ok(inserted.id)
}

// decide whether to use the realtime channel or fall back to the STT/TTS
// pipeline. fallback on: voice disabled, no key, or a failed mint.
pub fn should_use_realtime(enabled: bool, has_key: bool) -> bool {
    enabled && has_key
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VoiceConfigDto {
    pub enabled: bool,
    pub voice: String,
    pub model: String,
}

pub fn load_voice_config(store: &TaskStore) -> VoiceConfigDto {
    let enabled = store
        .get_app_setting(VOICE_ENABLED_KEY)
        .ok()
        .flatten()
        .map(|value| value == "true" || value == "1")
        .unwrap_or(false);
    let voice = store
        .get_app_setting(VOICE_NAME_KEY)
        .ok()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| realtime::REALTIME_DEFAULT_VOICE.to_string());
    VoiceConfigDto {
        enabled,
        voice,
        model: realtime::REALTIME_MODEL.to_string(),
    }
}

// the concrete realtime session: mints an ephemeral credential for the frontend
// WebRTC connection. holds no socket itself (audio lives in the frontend).
pub struct RealtimeVoiceSession {
    voice: String,
    state: std::sync::Mutex<VoiceSessionState>,
}

impl RealtimeVoiceSession {
    pub fn new(voice: impl Into<String>) -> Self {
        Self {
            voice: voice.into(),
            state: std::sync::Mutex::new(VoiceSessionState::Idle),
        }
    }

    fn set_state(&self, next: VoiceSessionState) {
        if let Ok(mut guard) = self.state.lock() {
            *guard = next;
        }
    }

    #[cfg(test)]
    fn state(&self) -> VoiceSessionState {
        self.state
            .lock()
            .map(|guard| *guard)
            .unwrap_or(VoiceSessionState::Closed)
    }
}

impl VoiceSession for RealtimeVoiceSession {
    fn open(&self, instructions: &str) -> Result<RealtimeCredentials> {
        self.set_state(VoiceSessionState::Connecting);
        match realtime::mint_realtime_session(&self.voice, instructions) {
            Ok(credentials) => {
                self.set_state(VoiceSessionState::Live);
                Ok(credentials)
            }
            Err(err) => {
                self.set_state(VoiceSessionState::Fallback);
                Err(err)
            }
        }
    }

    fn close(&self) {
        self.set_state(VoiceSessionState::Closed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, TaskStore, i64) {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        let task = store.create_task("voice").unwrap();
        (dir, store, task.id)
    }

    #[test]
    fn c4_instructions_carry_character_and_context_blocks() {
        let instructions = build_session_instructions(
            "You are Jeff.",
            Some("active document: draft.md"),
            Some("Relational context: prefers direct critique."),
            Some("Memory recall:\n- preference: strip em dashes"),
        );
        assert!(instructions.contains("You are Jeff."));
        assert!(instructions.contains("spoken conversation"));
        assert!(instructions.contains("active document"));
        assert!(instructions.contains("prefers direct critique"));
        assert!(instructions.contains("strip em dashes"));
    }

    #[test]
    fn c4_instructions_omit_empty_context() {
        let instructions = build_session_instructions("You are Jeff.", None, Some("   "), None);
        assert!(instructions.contains("You are Jeff."));
        assert!(!instructions.contains("Current situation"));
    }

    #[test]
    fn c4_tool_call_routes_spoken_request_as_text() {
        let action =
            route_voice_tool_call("route_request", &serde_json::json!({ "text": "fix it" }));
        assert_eq!(action, VoiceAction::RouteAsText("fix it".to_string()));
        // "fix it" spoken == "fix it" typed.
        assert_eq!(
            route_voice_tool_call("route_request", &serde_json::json!({ "text": "  fix it  " })),
            VoiceAction::RouteAsText("fix it".to_string())
        );
        assert_eq!(
            route_voice_tool_call("unknown", &serde_json::json!({ "text": "hi" })),
            VoiceAction::None
        );
        assert_eq!(
            route_voice_tool_call("route_request", &serde_json::json!({ "text": "  " })),
            VoiceAction::None
        );
    }

    #[test]
    fn c4_voice_turns_persist_as_normal_chat_messages() {
        let (_dir, store, task_id) = store();
        let user_id = persist_voice_turn(&store, task_id, "user", "where is my argument weakest?")
            .unwrap();
        let assistant_id =
            persist_voice_turn(&store, task_id, "assistant", "section two, it lacks evidence")
                .unwrap();
        assert!(user_id > 0 && assistant_id > 0);
        let messages = store.list_recent_chat_messages(task_id, 10).unwrap();
        assert_eq!(messages.len(), 2);
        assert!(messages.iter().all(|m| m.message_source == VOICE_SOURCE));
        assert!(messages.iter().any(|m| m.role == "user"));
        assert!(messages.iter().any(|m| m.role == "assistant"));
    }

    #[test]
    fn c4_empty_voice_turn_is_not_persisted() {
        let (_dir, store, task_id) = store();
        assert_eq!(persist_voice_turn(&store, task_id, "user", "   ").unwrap(), 0);
        assert!(store.list_recent_chat_messages(task_id, 10).unwrap().is_empty());
    }

    #[test]
    fn c4_fallback_when_disabled_or_no_key() {
        assert!(should_use_realtime(true, true));
        assert!(!should_use_realtime(false, true));
        assert!(!should_use_realtime(true, false));
    }

    #[test]
    fn c4_session_state_moves_to_fallback_without_key() {
        // no key configured in the test env -> mint fails -> fallback state.
        let session = RealtimeVoiceSession::new("verse");
        assert_eq!(session.state(), VoiceSessionState::Idle);
        let result = session.open("be a coworker");
        assert!(result.is_err());
        assert_eq!(session.state(), VoiceSessionState::Fallback);
    }

    #[test]
    fn c4_voice_config_defaults_to_disabled_with_default_voice() {
        let (_dir, store, _task) = store();
        let config = load_voice_config(&store);
        assert!(!config.enabled);
        assert_eq!(config.voice, realtime::REALTIME_DEFAULT_VOICE);
        assert_eq!(config.model, realtime::REALTIME_MODEL);
    }
}
