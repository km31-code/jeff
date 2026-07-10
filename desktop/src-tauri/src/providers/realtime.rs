// apex c4: OpenAI Realtime adapter. jeff mints a short-lived realtime session
// (a plain https POST) and hands the ephemeral client secret to the frontend,
// which opens the WebRTC audio connection directly to OpenAI. the model name
// lives here so no model string leaks outside providers/ (a1 grep gate).
//
// the mint call is env-gated (needs an OpenAI key); the request builder and
// response parser are pure and unit-tested.

use anyhow::{anyhow, Context, Result};
use std::time::Duration;

pub const REALTIME_MODEL: &str = "gpt-4o-realtime-preview";
pub const REALTIME_DEFAULT_VOICE: &str = "verse";
const REALTIME_SESSIONS_URL: &str = "https://api.openai.com/v1/realtime/sessions";
const REALTIME_BETA_HEADER: &str = "realtime=v1";
const MINT_TIMEOUT_MS: u64 = 6000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeCredentials {
    pub client_secret: String,
    pub expires_at: i64,
    pub model: String,
}

// build the realtime session config sent to OpenAI. instructions carry the
// character + situational context; a single route_request tool lets the model
// hand structured requests back to jeff's text command surface.
pub fn build_session_request(model: &str, voice: &str, instructions: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "voice": voice,
        "modalities": ["audio", "text"],
        "instructions": instructions,
        "input_audio_transcription": { "model": "whisper-1" },
        "turn_detection": { "type": "server_vad" },
        "tools": [
            {
                "type": "function",
                "name": "route_request",
                "description": "Route a spoken request to Jeff's text command surface, exactly as if the user had typed it (e.g. 'fix it', 'draft the intro').",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "The user's request in their words." }
                    },
                    "required": ["text"]
                }
            }
        ]
    })
}

pub fn parse_session_response(payload: &serde_json::Value) -> Result<RealtimeCredentials> {
    let client_secret = payload
        .get("client_secret")
        .and_then(|secret| secret.get("value"))
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow!("realtime session response missing client_secret.value"))?
        .to_string();
    let expires_at = payload
        .get("client_secret")
        .and_then(|secret| secret.get("expires_at"))
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    let model = payload
        .get("model")
        .and_then(|value| value.as_str())
        .unwrap_or(REALTIME_MODEL)
        .to_string();
    Ok(RealtimeCredentials {
        client_secret,
        expires_at,
        model,
    })
}

// mint a realtime session. env-gated: requires an OpenAI key. on any failure the
// caller falls back to the STT/TTS pipeline.
pub fn mint_realtime_session(voice: &str, instructions: &str) -> Result<RealtimeCredentials> {
    let api_key = crate::secrets::resolve_openai_api_key_required()?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(MINT_TIMEOUT_MS))
        .build()
        .context("failed to build realtime http client")?;
    let response = client
        .post(REALTIME_SESSIONS_URL)
        .bearer_auth(&api_key)
        .header("OpenAI-Beta", REALTIME_BETA_HEADER)
        .json(&build_session_request(REALTIME_MODEL, voice, instructions))
        .send()
        .context("failed to call realtime sessions API")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return Err(anyhow!("realtime session mint failed ({status}): {body}"));
    }
    let payload: serde_json::Value = response
        .json()
        .context("failed to parse realtime session response")?;
    parse_session_response(&payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c4_session_request_carries_instructions_tools_and_voice() {
        let request = build_session_request(REALTIME_MODEL, "verse", "be a good coworker");
        assert_eq!(request["model"], REALTIME_MODEL);
        assert_eq!(request["voice"], "verse");
        assert_eq!(request["instructions"], "be a good coworker");
        assert_eq!(request["tools"][0]["name"], "route_request");
        assert!(request["modalities"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("audio")));
    }

    #[test]
    fn c4_parse_session_response_extracts_ephemeral_secret() {
        let payload = serde_json::json!({
            "model": "gpt-4o-realtime-preview",
            "client_secret": { "value": "ek_abc123", "expires_at": 1_700_000_000i64 }
        });
        let creds = parse_session_response(&payload).unwrap();
        assert_eq!(creds.client_secret, "ek_abc123");
        assert_eq!(creds.expires_at, 1_700_000_000);
        assert_eq!(creds.model, "gpt-4o-realtime-preview");
    }

    #[test]
    fn c4_parse_session_response_rejects_missing_secret() {
        let payload = serde_json::json!({ "model": "x" });
        assert!(parse_session_response(&payload).is_err());
    }
}
