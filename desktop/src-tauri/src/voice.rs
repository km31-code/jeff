use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use reqwest::blocking::{multipart, Client};
use serde::Deserialize;

use crate::models::{SpeechSynthesisDto, TranscriptionResultDto};

#[derive(Clone)]
pub struct OpenAiVoiceProvider {
    client: Client,
    api_key: Option<String>,
    stt_model: String,
    tts_model: String,
    tts_voice: String,
}

impl OpenAiVoiceProvider {
    pub fn from_env() -> Self {
        Self {
            client: Client::new(),
            api_key: std::env::var("OPENAI_API_KEY")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            stt_model: "whisper-1".to_string(),
            tts_model: "gpt-4o-mini-tts".to_string(),
            tts_voice: "alloy".to_string(),
        }
    }

    pub fn transcribe_audio_base64(
        &self,
        audio_base64: &str,
        mime_type: &str,
    ) -> Result<TranscriptionResultDto> {
        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| anyhow!("OPENAI_API_KEY is not configured"))?;

        let audio_bytes = BASE64
            .decode(audio_base64.trim())
            .context("failed to decode audio_base64 payload")?;

        if audio_bytes.is_empty() {
            return Err(anyhow!("audio payload is empty"));
        }

        let extension = extension_from_mime_type(mime_type);
        let file_name = format!("input.{extension}");

        let part = multipart::Part::bytes(audio_bytes)
            .file_name(file_name)
            .mime_str(mime_type)
            .context("failed to set multipart mime type for transcription")?;

        let form = multipart::Form::new()
            .text("model", self.stt_model.clone())
            .part("file", part);

        let response = self
            .client
            .post("https://api.openai.com/v1/audio/transcriptions")
            .bearer_auth(api_key)
            .multipart(form)
            .send()
            .context("failed to call OpenAI transcription API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .unwrap_or_else(|_| "<unreadable-body>".to_string());
            return Err(anyhow!(
                "OpenAI transcription request failed with status {}: {}",
                status,
                body
            ));
        }

        let payload: TranscriptionResponse = response
            .json()
            .context("failed to parse transcription response")?;

        let text = payload.text.trim().to_string();
        if text.is_empty() {
            return Err(anyhow!("transcription returned empty text"));
        }

        Ok(TranscriptionResultDto { text })
    }

    pub fn synthesize_speech(&self, text: &str) -> Result<SpeechSynthesisDto> {
        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| anyhow!("OPENAI_API_KEY is not configured"))?;

        let clean_text = text.trim();
        if clean_text.is_empty() {
            return Err(anyhow!("speech synthesis text cannot be empty"));
        }

        let response = self
            .client
            .post("https://api.openai.com/v1/audio/speech")
            .bearer_auth(api_key)
            .json(&serde_json::json!({
                "model": self.tts_model,
                "voice": self.tts_voice,
                "input": clean_text,
                "format": "mp3"
            }))
            .send()
            .context("failed to call OpenAI speech API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .unwrap_or_else(|_| "<unreadable-body>".to_string());
            return Err(anyhow!(
                "OpenAI speech request failed with status {}: {}",
                status,
                body
            ));
        }

        let audio_bytes = response
            .bytes()
            .context("failed to read speech response bytes")?
            .to_vec();

        if audio_bytes.is_empty() {
            return Err(anyhow!("speech response returned empty audio"));
        }

        Ok(SpeechSynthesisDto {
            audio_base64: BASE64.encode(audio_bytes),
            mime_type: "audio/mpeg".to_string(),
        })
    }
}

fn extension_from_mime_type(mime_type: &str) -> &'static str {
    match mime_type {
        "audio/webm" => "webm",
        "audio/mp4" => "m4a",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/mpeg" => "mp3",
        _ => "webm",
    }
}

#[derive(Debug, Deserialize)]
struct TranscriptionResponse {
    text: String,
}
