// apex a1: anthropic messages api adapter. blocking, async, and streaming
// entry points used exclusively by the model router — call sites never touch
// this module directly. key resolution goes through secrets.rs (keychain
// first, ANTHROPIC_API_KEY env fallback).

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde::Deserialize;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::model_router::{CacheHint, LlmUsage, SystemBlock};

const ANTHROPIC_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 4096;

pub struct AnthropicRequest<'a> {
    pub model: &'a str,
    pub system_blocks: &'a [SystemBlock],
    pub user: &'a str,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
    // when true, the system prompt gains a strict json-only instruction.
    // anthropic has no response_format parameter; this is the documented
    // equivalent for structured output at this scale.
    pub json_only: bool,
    pub timeout_ms: Option<u64>,
}

fn request_body(req: &AnthropicRequest, stream: bool) -> serde_json::Value {
    let system = system_content_blocks(req);
    serde_json::json!({
        "model": req.model,
        "max_tokens": req.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        "temperature": req.temperature,
        "system": system,
        "stream": stream,
        "messages": [
            { "role": "user", "content": req.user }
        ]
    })
}

fn system_content_blocks(req: &AnthropicRequest) -> Vec<serde_json::Value> {
    let mut blocks = req
        .system_blocks
        .iter()
        .filter_map(|block| {
            let text = block.text.trim();
            if text.is_empty() {
                return None;
            }
            let mut value = serde_json::json!({
                "type": "text",
                "text": text,
            });
            if matches!(block.cache_hint, CacheHint::Stable | CacheHint::Session) {
                value["cache_control"] = serde_json::json!({ "type": "ephemeral" });
            }
            Some(value)
        })
        .collect::<Vec<_>>();

    if req.json_only {
        blocks.push(serde_json::json!({
            "type": "text",
            "text": "Respond with a single valid JSON object and nothing else."
        }));
    }

    blocks
}

pub fn generate_blocking(req: &AnthropicRequest) -> Result<(String, LlmUsage)> {
    let api_key = crate::secrets::resolve_anthropic_api_key_required()?;

    let mut builder = reqwest::blocking::Client::builder();
    if let Some(timeout_ms) = req.timeout_ms {
        builder = builder
            .timeout(Duration::from_millis(timeout_ms))
            .connect_timeout(Duration::from_millis(timeout_ms));
    }
    let client = builder
        .build()
        .context("failed to build anthropic http client")?;

    let response = client
        .post(ANTHROPIC_MESSAGES_URL)
        .header("x-api-key", &api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .json(&request_body(req, false))
        .send()
        .context("failed to call Anthropic messages API")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .unwrap_or_else(|_| "<unreadable-body>".to_string());
        return Err(anyhow!(
            "Anthropic request failed with status {}: {}",
            status,
            body
        ));
    }

    let payload: MessagesResponse = response
        .json()
        .context("failed to parse Anthropic messages response")?;
    extract_text_and_usage(payload)
}

pub async fn generate_async(req: &AnthropicRequest<'_>) -> Result<(String, LlmUsage)> {
    let api_key = crate::secrets::resolve_anthropic_api_key_required()?;

    let mut builder = reqwest::Client::builder();
    if let Some(timeout_ms) = req.timeout_ms {
        builder = builder.timeout(Duration::from_millis(timeout_ms));
    }
    let client = builder
        .build()
        .context("failed to build anthropic http client")?;

    let response = client
        .post(ANTHROPIC_MESSAGES_URL)
        .header("x-api-key", &api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .json(&request_body(req, false))
        .send()
        .await
        .context("failed to call Anthropic messages API")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "Anthropic request failed with status {}: {}",
            status,
            body
        ));
    }

    let payload: MessagesResponse = response
        .json()
        .await
        .context("failed to parse Anthropic messages response")?;
    extract_text_and_usage(payload)
}

// streams text deltas through an mpsc channel with the same contract as the
// openai streaming path: caller reads Result<String> deltas until close.
pub fn stream(
    model: String,
    system_blocks: Vec<SystemBlock>,
    user: String,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<mpsc::Receiver<Result<String>>> {
    let api_key = crate::secrets::resolve_anthropic_api_key_required()?;

    let body = request_body(
        &AnthropicRequest {
            model: &model,
            system_blocks: &system_blocks,
            user: &user,
            temperature: 0.0,
            max_tokens: None,
            json_only: false,
            timeout_ms: None,
        },
        true,
    );

    let (tx, rx) = mpsc::channel::<Result<String>>(256);

    tokio::spawn(async move {
        let client = reqwest::Client::new();
        let response = match client
            .post(ANTHROPIC_MESSAGES_URL)
            .header("x-api-key", &api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(err) => {
                let _ = tx
                    .send(Err(anyhow!("Anthropic streaming request failed: {err}")))
                    .await;
                return;
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            let _ = tx
                .send(Err(anyhow!(
                    "Anthropic streaming status {status}: {body_text}"
                )))
                .await;
            return;
        }

        let mut stream = response.bytes_stream();
        let mut buf = String::new();

        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                chunk = stream.next() => {
                    let Some(chunk_result) = chunk else { break };
                    let bytes = match chunk_result {
                        Ok(b) => b,
                        Err(err) => {
                            let _ = tx.send(Err(anyhow!("stream read error: {err}"))).await;
                            break;
                        }
                    };
                    buf.push_str(&String::from_utf8_lossy(&bytes));

                    while let Some(newline_pos) = buf.find('\n') {
                        let line = buf[..newline_pos].trim().to_string();
                        buf = buf[newline_pos + 1..].to_string();

                        if line.is_empty() {
                            continue;
                        }
                        let Some(data) = line.strip_prefix("data: ") else {
                            continue;
                        };
                        let Ok(json) = serde_json::from_str::<serde_json::Value>(data) else {
                            continue;
                        };
                        match json["type"].as_str() {
                            Some("content_block_delta") => {
                                if let Some(delta) = json["delta"]["text"].as_str() {
                                    if !delta.is_empty()
                                        && tx.send(Ok(delta.to_string())).await.is_err()
                                    {
                                        return;
                                    }
                                }
                            }
                            Some("message_stop") => return,
                            Some("error") => {
                                let message = json["error"]["message"]
                                    .as_str()
                                    .unwrap_or("unknown stream error")
                                    .to_string();
                                let _ = tx
                                    .send(Err(anyhow!("Anthropic stream error: {message}")))
                                    .await;
                                return;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    });

    Ok(rx)
}

fn extract_text_and_usage(payload: MessagesResponse) -> Result<(String, LlmUsage)> {
    let text = payload
        .content
        .into_iter()
        .filter_map(|block| block.text)
        .collect::<Vec<_>>()
        .join("")
        .trim()
        .to_string();

    if text.is_empty() {
        return Err(anyhow!("Anthropic response contained no text"));
    }

    let usage = LlmUsage {
        input_tokens: payload.usage.input_tokens.unwrap_or(0),
        output_tokens: payload.usage.output_tokens.unwrap_or(0),
        cached_tokens: payload.usage.cache_read_input_tokens.unwrap_or(0),
    };
    Ok((text, usage))
}

#[derive(Debug, Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
    #[serde(default)]
    usage: UsagePayload,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct UsagePayload {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}
