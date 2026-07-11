// apex c4: OpenAI Realtime adapter. Jeff mints a short-lived client secret
// (a plain HTTPS POST) and hands it to the frontend, which opens the WebRTC
// call directly to OpenAI. The model name lives here so no model string leaks
// outside providers/ (A1 grep gate).
//
// the mint call is env-gated (needs an OpenAI key); the request builder and
// response parser are pure and unit-tested.

use anyhow::{anyhow, Context, Result};
use std::time::Duration;

pub const REALTIME_MODEL: &str = "gpt-realtime-2.1";
pub const REALTIME_DEFAULT_VOICE: &str = "verse";
const REALTIME_CLIENT_SECRETS_URL: &str =
    "https://api.openai.com/v1/realtime/client_secrets";
const MINT_TIMEOUT_MS: u64 = 6000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeCredentials {
    pub client_secret: String,
    pub expires_at: i64,
    pub model: String,
}

// Build the current client-secrets request. The session is deliberately nested
// under `session`; this differs from the retired beta `/realtime/sessions`
// request shape. Audio is the sole output modality (audio includes a transcript
// event), and route_request is the only mutation-capable tool surface.
pub fn build_session_request(model: &str, voice: &str, instructions: &str) -> serde_json::Value {
    serde_json::json!({
        "expires_after": {
            "anchor": "created_at",
            "seconds": 60
        },
        "session": {
            "type": "realtime",
            "model": model,
            "output_modalities": ["audio"],
            "instructions": instructions,
            "audio": {
                "input": {
                    "transcription": { "model": "whisper-1" },
                    "turn_detection": { "type": "server_vad" }
                },
                "output": { "voice": voice }
            },
            "tool_choice": "auto",
            "tools": [
                {
                    "type": "function",
                    "name": "route_request",
                    "description": "Route a spoken request to Jeff's text command surface, exactly as if the user had typed it (for example, 'fix it' or 'draft the intro').",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "text": { "type": "string", "description": "The user's request in their words." }
                        },
                        "required": ["text"],
                        "additionalProperties": false
                    }
                }
            ]
        }
    })
}

pub fn parse_session_response(payload: &serde_json::Value) -> Result<RealtimeCredentials> {
    // Current `/realtime/client_secrets` responses put the secret at the top
    // level. Accept the old nested shape only to make an in-flight backend
    // rollout non-breaking; new requests always use the current endpoint.
    let client_secret = payload
        .get("value")
        .and_then(|value| value.as_str())
        .or_else(|| {
            payload
                .get("client_secret")
                .and_then(|secret| secret.get("value"))
                .and_then(|value| value.as_str())
        })
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("realtime client-secret response missing value"))?
        .to_string();
    let expires_at = payload
        .get("expires_at")
        .and_then(|value| value.as_i64())
        .or_else(|| {
            payload
                .get("client_secret")
                .and_then(|secret| secret.get("expires_at"))
                .and_then(|value| value.as_i64())
        })
        .unwrap_or(0);
    let model = payload
        .get("session")
        .and_then(|session| session.get("model"))
        .and_then(|value| value.as_str())
        .or_else(|| payload.get("model").and_then(|value| value.as_str()))
        .unwrap_or(REALTIME_MODEL)
        .to_string();
    Ok(RealtimeCredentials {
        client_secret,
        expires_at,
        model,
    })
}

// Mint a Realtime client secret. Env-gated: requires an OpenAI key. On any
// failure the caller falls back to the STT/TTS pipeline.
pub fn mint_realtime_session(voice: &str, instructions: &str) -> Result<RealtimeCredentials> {
    let api_key = crate::secrets::resolve_openai_api_key_required()?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(MINT_TIMEOUT_MS))
        .build()
        .context("failed to build realtime http client")?;
    let response = client
        .post(REALTIME_CLIENT_SECRETS_URL)
        .bearer_auth(&api_key)
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
        assert_eq!(request["session"]["type"], "realtime");
        assert_eq!(request["session"]["model"], REALTIME_MODEL);
        assert_eq!(request["session"]["audio"]["output"]["voice"], "verse");
        assert_eq!(request["session"]["instructions"], "be a good coworker");
        assert_eq!(request["session"]["tools"][0]["name"], "route_request");
        assert!(request["session"]["output_modalities"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("audio")));
    }

    #[test]
    fn c4_parse_session_response_extracts_ephemeral_secret() {
        let payload = serde_json::json!({
            "value": "ek_abc123",
            "expires_at": 1_700_000_000i64,
            "session": { "type": "realtime", "model": REALTIME_MODEL }
        });
        let creds = parse_session_response(&payload).unwrap();
        assert_eq!(creds.client_secret, "ek_abc123");
        assert_eq!(creds.expires_at, 1_700_000_000);
        assert_eq!(creds.model, REALTIME_MODEL);
    }

    #[test]
    fn c4_parse_session_response_keeps_legacy_shape_compatible() {
        let payload = serde_json::json!({
            "model": "legacy-model",
            "client_secret": { "value": "ek_legacy", "expires_at": 42 }
        });
        let creds = parse_session_response(&payload).unwrap();
        assert_eq!(creds.client_secret, "ek_legacy");
        assert_eq!(creds.expires_at, 42);
        assert_eq!(creds.model, "legacy-model");
    }

    #[test]
    fn c4_parse_session_response_rejects_missing_secret() {
        let payload = serde_json::json!({ "model": "x" });
        assert!(parse_session_response(&payload).is_err());
    }
}
