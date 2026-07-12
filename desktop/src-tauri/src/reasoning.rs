use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use tokio::sync::mpsc;

// re-export the canonical trait so all call sites use one definition.
// reasoning::ReasoningProvider resolves to providers::ReasoningModelProvider.
pub use crate::providers::ReasoningModelProvider as ReasoningProvider;

// ---- streaming provider -----------------------------------------------------

// apex a1: the model is injected by the model router — no model names live in
// this module. blocking generation also moved into the router; the legacy
// blocking wrapper that lived here is gone.
#[derive(Clone)]
pub struct OpenAiStreamingReasoningProvider {
    model: String,
    endpoint: String,
    bearer_token: Option<String>,
}

impl OpenAiStreamingReasoningProvider {
    pub fn with_model(model: String) -> Self {
        Self {
            model,
            endpoint: "https://api.openai.com/v1/chat/completions".to_string(),
            bearer_token: None,
        }
    }

    pub fn with_endpoint_and_token(model: String, endpoint: String, bearer_token: String) -> Self {
        Self {
            model,
            endpoint,
            bearer_token: Some(bearer_token),
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
        let api_key = match &self.bearer_token {
            Some(token) => token.clone(),
            None => crate::secrets::resolve_openai_api_key_required()?,
        };
        let endpoint = self.endpoint.clone();

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
                .post(endpoint)
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
