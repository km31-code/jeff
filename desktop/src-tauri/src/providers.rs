use anyhow::{anyhow, Context, Result};
use reqwest::blocking::{multipart, Client};
use serde::Deserialize;
use std::time::Duration;

use crate::models::{IntentClassificationDto, SpeechSynthesisDto, TranscriptionResultDto};

pub trait SpeechToTextProvider: Send + Sync {
    fn transcribe(&self, audio_bytes: &[u8], mime_type: &str) -> Result<String>;
}

pub trait TextToSpeechProvider: Send + Sync {
    fn synthesize(&self, text: &str, voice: &str) -> Result<Vec<u8>>;
}

pub trait ReasoningModelProvider: Send + Sync {
    fn generate_response(&self, system_prompt: &str, user_prompt: &str) -> Result<String>;
}

pub trait EmbeddingsProvider: Send + Sync {
    fn embed_text(&self, text: &str) -> Result<Vec<f32>>;
}

pub trait ClassifierProvider: Send + Sync {
    fn classify(
        &self,
        text: &str,
        api_key: &str,
    ) -> std::result::Result<IntentClassificationDto, String>;
}

// composite voice seam: transcription + synthesis behind a single injectable interface.
// concrete implementation is OpenAiVoiceProvider in voice.rs; state.rs holds
// Arc<dyn VoiceProvider> so the call path is provider-agnostic.
pub trait VoiceProvider: Send + Sync {
    fn transcribe_audio_base64(
        &self,
        audio_base64: &str,
        mime_type: &str,
    ) -> Result<TranscriptionResultDto>;
    fn synthesize_speech(&self, text: &str, voice: &str) -> Result<SpeechSynthesisDto>;
}

#[derive(Clone)]
pub struct OpenAiReasoningProvider {
    client: Client,
    model: String,
}

impl OpenAiReasoningProvider {
    pub fn from_env() -> Self {
        Self {
            client: Client::new(),
            model: "gpt-4o-mini".to_string(),
        }
    }
}

impl ReasoningModelProvider for OpenAiReasoningProvider {
    fn generate_response(&self, system_prompt: &str, user_prompt: &str) -> Result<String> {
        let api_key = crate::secrets::resolve_openai_api_key_required()?;

        let response = self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .bearer_auth(&api_key)
            .json(&serde_json::json!({
                "model": self.model,
                "temperature": 0,
                "messages": [
                    { "role": "system", "content": system_prompt },
                    { "role": "user", "content": user_prompt }
                ]
            }))
            .send()
            .context("failed to call OpenAI chat completions API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .unwrap_or_else(|_| "<unreadable-body>".to_string());
            return Err(anyhow!(
                "OpenAI reasoning request failed with status {}: {}",
                status,
                body
            ));
        }

        let payload: ChatCompletionResponse = response
            .json()
            .context("failed to parse chat completions response")?;

        let content = payload
            .choices
            .into_iter()
            .next()
            .map(|choice| choice.message.content)
            .unwrap_or_default()
            .trim()
            .to_string();

        if content.is_empty() {
            Err(anyhow!("OpenAI reasoning response was empty"))
        } else {
            Ok(content)
        }
    }
}

#[derive(Clone)]
pub struct OpenAiSttProvider {
    client: Client,
    model: String,
}

impl OpenAiSttProvider {
    pub fn from_env() -> Self {
        Self {
            client: Client::new(),
            model: "whisper-1".to_string(),
        }
    }
}

impl SpeechToTextProvider for OpenAiSttProvider {
    fn transcribe(&self, audio_bytes: &[u8], mime_type: &str) -> Result<String> {
        let api_key = crate::secrets::resolve_openai_api_key_required()?;

        if audio_bytes.is_empty() {
            return Err(anyhow!("audio payload is empty"));
        }

        let extension = extension_from_mime_type(mime_type);
        let file_name = format!("input.{extension}");

        let part = multipart::Part::bytes(audio_bytes.to_vec())
            .file_name(file_name)
            .mime_str(mime_type)
            .context("failed to set multipart mime type for transcription")?;

        let form = multipart::Form::new()
            .text("model", self.model.clone())
            .part("file", part);

        let response = self
            .client
            .post("https://api.openai.com/v1/audio/transcriptions")
            .bearer_auth(&api_key)
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

        Ok(text)
    }
}

#[derive(Clone)]
pub struct OpenAiTtsProvider {
    client: Client,
    model: String,
}

impl OpenAiTtsProvider {
    pub fn from_env() -> Self {
        Self {
            client: Client::new(),
            model: "gpt-4o-mini-tts".to_string(),
        }
    }
}

impl TextToSpeechProvider for OpenAiTtsProvider {
    fn synthesize(&self, text: &str, voice: &str) -> Result<Vec<u8>> {
        let api_key = crate::secrets::resolve_openai_api_key_required()?;

        let clean_text = text.trim();
        if clean_text.is_empty() {
            return Err(anyhow!("speech synthesis text cannot be empty"));
        }

        let voice = crate::voice_naturalness::normalize_tts_voice(voice);
        let response = self
            .client
            .post("https://api.openai.com/v1/audio/speech")
            .bearer_auth(&api_key)
            .json(&serde_json::json!({
                "model": self.model,
                "voice": voice,
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

        Ok(audio_bytes)
    }
}

#[derive(Clone)]
pub struct OpenAiEmbeddingsProvider {
    client: Client,
    model: String,
}

impl OpenAiEmbeddingsProvider {
    pub fn from_env() -> Self {
        Self {
            client: Client::new(),
            model: "text-embedding-3-small".to_string(),
        }
    }
}

impl EmbeddingsProvider for OpenAiEmbeddingsProvider {
    fn embed_text(&self, input: &str) -> Result<Vec<f32>> {
        let api_key = crate::secrets::resolve_openai_api_key_required()?;

        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("embedding input cannot be empty"));
        }

        let response = self
            .client
            .post("https://api.openai.com/v1/embeddings")
            .bearer_auth(&api_key)
            .json(&serde_json::json!({
                "model": self.model,
                "input": trimmed
            }))
            .send()
            .context("failed to call OpenAI embeddings API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .unwrap_or_else(|_| "<unreadable-body>".to_string());
            return Err(anyhow!(
                "OpenAI embeddings request failed with status {}: {}",
                status,
                body
            ));
        }

        let payload: OpenAiEmbeddingsResponse = response
            .json()
            .context("failed to parse embeddings API response")?;

        let first = payload
            .data
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("OpenAI embeddings response returned no vectors"))?;

        if first.embedding.is_empty() {
            return Err(anyhow!(
                "OpenAI embeddings response returned an empty vector"
            ));
        }

        Ok(first.embedding)
    }
}

#[derive(Clone)]
pub struct OpenAiClassifierProvider {
    model: String,
}

impl OpenAiClassifierProvider {
    pub fn new() -> Self {
        Self {
            model: crate::classifier::MODEL.to_string(),
        }
    }
}

impl ClassifierProvider for OpenAiClassifierProvider {
    fn classify(
        &self,
        text: &str,
        api_key: &str,
    ) -> std::result::Result<IntentClassificationDto, String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(IntentClassificationDto {
                intent: crate::models::IntentLabel::Unknown,
                confidence: 0.0,
                slots: crate::models::IntentSlotsDto::default(),
            });
        }

        let client = Client::builder()
            .timeout(Duration::from_millis(crate::classifier::REQUEST_TIMEOUT_MS))
            .connect_timeout(Duration::from_millis(crate::classifier::REQUEST_TIMEOUT_MS))
            .build()
            .map_err(|err| {
                format!("failed to build HTTP client for intent classification: {err}")
            })?;

        let response = client
            .post("https://api.openai.com/v1/chat/completions")
            .bearer_auth(api_key)
            .json(&serde_json::json!({
                "model": self.model,
                "temperature": 0,
                "response_format": { "type": "json_object" },
                "messages": [
                    { "role": "system", "content": crate::classifier::SYSTEM_PROMPT },
                    { "role": "user", "content": trimmed }
                ]
            }))
            .send()
            .map_err(|err| {
                format!("failed to call OpenAI chat completions for intent classification: {err}")
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .unwrap_or_else(|_| "<unreadable>".to_string());
            return Err(format!(
                "intent classifier request failed with status {}: {}",
                status, body
            ));
        }

        let payload: ClassifierApiResponse = response
            .json()
            .map_err(|err| format!("failed to parse intent classifier API response: {err}"))?;

        let content = payload
            .choices
            .into_iter()
            .next()
            .map(|choice| choice.message.content)
            .ok_or_else(|| "intent classifier response contained no choices".to_string())?;

        crate::classifier::parse_classification(&content).map_err(|err| err.to_string())
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
struct ChatCompletionResponse {
    choices: Vec<ChatCompletionChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChoice {
    message: ChatCompletionMessage,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionMessage {
    content: String,
}

#[derive(Debug, Deserialize)]
struct TranscriptionResponse {
    text: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiEmbeddingsResponse {
    data: Vec<OpenAiEmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct OpenAiEmbeddingData {
    embedding: Vec<f32>,
}

#[derive(Debug, Deserialize)]
struct ClassifierApiResponse {
    choices: Vec<ClassifierChoice>,
}

#[derive(Debug, Deserialize)]
struct ClassifierChoice {
    message: ClassifierMessage,
}

#[derive(Debug, Deserialize)]
struct ClassifierMessage {
    content: String,
}
