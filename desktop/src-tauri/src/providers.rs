use anyhow::{anyhow, Context, Result};
use reqwest::blocking::{multipart, Client};
use serde::Deserialize;
use std::time::Duration;

use crate::model_router::{join_system_blocks, SystemBlock};
use crate::models::{IntentClassificationDto, SpeechSynthesisDto, TranscriptionResultDto};

// apex a1: anthropic messages api adapter, dispatched to by the model router.
pub mod anthropic;
pub mod local;

// tts model constant lives here so no call site outside providers/ names a
// model string (apex a1 grep gate). replaced by the voice session work in c4.
pub const OPENAI_TTS_MODEL: &str = "gpt-4o-mini-tts";
pub const OPENAI_EMBEDDING_MODEL: &str = "text-embedding-3-small";
pub const OPENAI_EMBEDDING_MODEL_ID: &str = "openai:text-embedding-3-small";

pub trait SpeechToTextProvider: Send + Sync {
    fn transcribe(&self, audio_bytes: &[u8], mime_type: &str) -> Result<String>;
}

pub trait TextToSpeechProvider: Send + Sync {
    fn synthesize(&self, text: &str, voice: &str) -> Result<Vec<u8>>;
}

pub trait ReasoningModelProvider: Send + Sync {
    fn generate_response(&self, system_prompt: &str, user_prompt: &str) -> Result<String>;

    fn generate_response_blocks(
        &self,
        system_blocks: &[SystemBlock],
        user_prompt: &str,
    ) -> Result<String> {
        self.generate_response(&join_system_blocks(system_blocks), user_prompt)
    }
}

pub trait EmbeddingsProvider: Send + Sync {
    fn embed_text(&self, text: &str) -> Result<Vec<f32>>;

    fn model_id(&self) -> &'static str {
        "unknown"
    }
}

#[allow(dead_code)]
pub trait ClassifierProvider: Send + Sync {
    fn classify(&self, text: &str) -> Result<IntentClassificationDto>;
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

// apex a1: call sites use the model router, but phase 17's provider seam
// remains available for tests and fallback wiring.

#[allow(dead_code)]
#[derive(Clone)]
pub struct OpenAiReasoningProvider {
    model: String,
}

#[allow(dead_code)]
impl OpenAiReasoningProvider {
    pub fn with_model(model: String) -> Self {
        Self { model }
    }
}

impl ReasoningModelProvider for OpenAiReasoningProvider {
    fn generate_response(&self, system_prompt: &str, user_prompt: &str) -> Result<String> {
        openai_generate_blocking(
            &self.model,
            system_prompt,
            user_prompt,
            0.0,
            None,
            false,
            None,
        )
        .map(|(text, _usage)| text)
    }
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct OpenAiClassifierProvider {
    model: String,
}

#[allow(dead_code)]
impl OpenAiClassifierProvider {
    pub fn with_model(model: String) -> Self {
        Self { model }
    }
}

impl ClassifierProvider for OpenAiClassifierProvider {
    fn classify(&self, text: &str) -> Result<IntentClassificationDto> {
        let raw = openai_generate_blocking(
            &self.model,
            crate::classifier::SYSTEM_PROMPT,
            text.trim(),
            0.0,
            Some(300),
            true,
            Some(300),
        )?
        .0;
        crate::classifier::parse_classification(&raw)
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
#[allow(dead_code)]
pub struct OpenAiEmbeddingsProvider {
    client: Client,
    model: String,
}

#[allow(dead_code)]
impl OpenAiEmbeddingsProvider {
    pub fn from_env() -> Self {
        Self {
            client: Client::new(),
            model: OPENAI_EMBEDDING_MODEL.to_string(),
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

    fn model_id(&self) -> &'static str {
        OPENAI_EMBEDDING_MODEL_ID
    }
}

// ---- apex a1: parameterized openai generation for the model router ----------
// free functions rather than provider structs: the router owns tier→model
// resolution, so these take the model explicitly and return usage for the
// cost governor (a4).

fn openai_chat_body(
    model: &str,
    system: &str,
    user: &str,
    temperature: f32,
    max_tokens: Option<u32>,
    json_object: bool,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": model,
        "temperature": temperature,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ]
    });
    if let Some(max) = max_tokens {
        body["max_tokens"] = serde_json::json!(max);
    }
    if json_object {
        body["response_format"] = serde_json::json!({ "type": "json_object" });
    }
    body
}

fn parse_openai_chat_response(
    payload: ChatCompletionResponse,
) -> Result<(String, crate::model_router::LlmUsage)> {
    let usage = payload
        .usage
        .as_ref()
        .map(|usage| crate::model_router::LlmUsage {
            input_tokens: usage.prompt_tokens.unwrap_or(0),
            output_tokens: usage.completion_tokens.unwrap_or(0),
            cached_tokens: usage
                .prompt_tokens_details
                .as_ref()
                .and_then(|details| details.cached_tokens)
                .unwrap_or(0),
        })
        .unwrap_or_default();

    let content = payload
        .choices
        .into_iter()
        .next()
        .map(|choice| choice.message.content)
        .unwrap_or_default()
        .trim()
        .to_string();

    if content.is_empty() {
        Err(anyhow!("OpenAI response was empty"))
    } else {
        Ok((content, usage))
    }
}

pub fn openai_generate_blocking(
    model: &str,
    system: &str,
    user: &str,
    temperature: f32,
    max_tokens: Option<u32>,
    json_object: bool,
    timeout_ms: Option<u64>,
) -> Result<(String, crate::model_router::LlmUsage)> {
    let api_key = crate::secrets::resolve_openai_api_key_required()?;

    let mut builder = Client::builder();
    if let Some(timeout) = timeout_ms {
        builder = builder
            .timeout(Duration::from_millis(timeout))
            .connect_timeout(Duration::from_millis(timeout));
    }
    let client = builder.build().context("failed to build openai client")?;

    let response = client
        .post("https://api.openai.com/v1/chat/completions")
        .bearer_auth(&api_key)
        .json(&openai_chat_body(
            model,
            system,
            user,
            temperature,
            max_tokens,
            json_object,
        ))
        .send()
        .context("failed to call OpenAI chat completions API")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .unwrap_or_else(|_| "<unreadable-body>".to_string());
        return Err(anyhow!(
            "OpenAI request failed with status {}: {}",
            status,
            body
        ));
    }

    let payload: ChatCompletionResponse = response
        .json()
        .context("failed to parse chat completions response")?;
    parse_openai_chat_response(payload)
}

pub async fn openai_generate_async(
    model: &str,
    system: &str,
    user: &str,
    temperature: f32,
    max_tokens: Option<u32>,
    json_object: bool,
    timeout_ms: Option<u64>,
) -> Result<(String, crate::model_router::LlmUsage)> {
    let api_key = crate::secrets::resolve_openai_api_key_required()?;

    let mut builder = reqwest::Client::builder();
    if let Some(timeout) = timeout_ms {
        builder = builder.timeout(Duration::from_millis(timeout));
    }
    let client = builder.build().context("failed to build openai client")?;

    let response = client
        .post("https://api.openai.com/v1/chat/completions")
        .bearer_auth(&api_key)
        .json(&openai_chat_body(
            model,
            system,
            user,
            temperature,
            max_tokens,
            json_object,
        ))
        .send()
        .await
        .context("failed to call OpenAI chat completions API")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "OpenAI request failed with status {}: {}",
            status,
            body
        ));
    }

    let payload: ChatCompletionResponse = response
        .json()
        .await
        .context("failed to parse chat completions response")?;
    parse_openai_chat_response(payload)
}

fn extension_from_mime_type(mime_type: &str) -> &'static str {
    let base = mime_type.split(';').next().unwrap_or(mime_type).trim();
    match base {
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
    #[serde(default)]
    usage: Option<OpenAiUsagePayload>,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsagePayload {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    #[serde(default)]
    prompt_tokens_details: Option<OpenAiPromptTokensDetails>,
}

#[derive(Debug, Deserialize)]
struct OpenAiPromptTokensDetails {
    cached_tokens: Option<u64>,
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
#[allow(dead_code)]
struct OpenAiEmbeddingsResponse {
    data: Vec<OpenAiEmbeddingData>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OpenAiEmbeddingData {
    embedding: Vec<f32>,
}
