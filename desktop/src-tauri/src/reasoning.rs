use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use reqwest::blocking::Client;
use serde::Deserialize;
use tokio::sync::mpsc;

pub trait ReasoningProvider: Send + Sync {
    fn generate_response(&self, system_prompt: &str, user_prompt: &str) -> Result<String>;
}

#[derive(Clone)]
pub struct OpenAiReasoningProvider {
    client: Client,
    api_key: Option<String>,
    model: String,
}

impl OpenAiReasoningProvider {
    pub fn from_env() -> Self {
        Self {
            client: Client::new(),
            api_key: std::env::var("OPENAI_API_KEY")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            model: "gpt-4o-mini".to_string(),
        }
    }
}

impl ReasoningProvider for OpenAiReasoningProvider {
    fn generate_response(&self, system_prompt: &str, user_prompt: &str) -> Result<String> {
        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| anyhow!("OPENAI_API_KEY is not configured"))?;

        let response = self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .bearer_auth(api_key)
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

// ---- streaming provider -----------------------------------------------------

// separate struct from the blocking provider so callers are explicit about
// which path they use. shares api_key and model config with the original.
#[derive(Clone)]
pub struct OpenAiStreamingReasoningProvider {
    api_key: Option<String>,
    model: String,
}

impl OpenAiStreamingReasoningProvider {
    pub fn from_env() -> Self {
        Self {
            api_key: std::env::var("OPENAI_API_KEY")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            model: "gpt-4o-mini".to_string(),
        }
    }

    // streams LLM tokens through an mpsc channel. spawns a tokio task that
    // reads SSE and sends delta strings. the caller reads from the returned
    // Receiver. when the stream ends or is cancelled, the channel closes.
    pub fn stream_response(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<mpsc::Receiver<Result<String>>> {
        let api_key = self
            .api_key
            .clone()
            .ok_or_else(|| anyhow!("OPENAI_API_KEY is not configured"))?;

        let body = serde_json::json!({
            "model": self.model,
            "temperature": 0,
            "stream": true,
            "messages": [
                { "role": "system", "content": system_prompt },
                { "role": "user",   "content": user_prompt   }
            ]
        });

        let (tx, rx) = mpsc::channel::<Result<String>>(256);

        tokio::spawn(async move {
            let client = reqwest::Client::new();
            let response = match client
                .post("https://api.openai.com/v1/chat/completions")
                .bearer_auth(&api_key)
                .json(&body)
                .send()
                .await
            {
                Ok(resp) => resp,
                Err(err) => {
                    let _ = tx
                        .send(Err(anyhow!("OpenAI streaming request failed: {err}")))
                        .await;
                    return;
                }
            };

            if !response.status().is_success() {
                let status = response.status();
                let body_text = response.text().await.unwrap_or_default();
                let _ = tx
                    .send(Err(anyhow!(
                        "OpenAI streaming status {status}: {body_text}"
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

                        // process every complete line in the buffer.
                        while let Some(newline_pos) = buf.find('\n') {
                            let line = buf[..newline_pos].trim().to_string();
                            buf = buf[newline_pos + 1..].to_string();

                            if line.is_empty() {
                                continue;
                            }

                            let Some(data) = line.strip_prefix("data: ") else {
                                continue;
                            };

                            if data == "[DONE]" {
                                return;
                            }

                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                                if let Some(delta) =
                                    json["choices"][0]["delta"]["content"].as_str()
                                {
                                    if !delta.is_empty() && tx.send(Ok(delta.to_string())).await.is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        Ok(rx)
    }
}

// ---- non-streaming response types ------------------------------------------

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
