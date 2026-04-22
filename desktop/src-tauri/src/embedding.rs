use anyhow::{anyhow, Context, Result};
use reqwest::blocking::Client;
use serde::Deserialize;

pub trait EmbeddingProvider: Send + Sync {
    fn embed_text(&self, input: &str) -> Result<Vec<f32>>;
}

#[derive(Clone)]
pub struct OpenAiEmbeddingProvider {
    client: Client,
    api_key: Option<String>,
    model: String,
}

impl OpenAiEmbeddingProvider {
    pub fn from_env() -> Self {
        Self {
            client: Client::new(),
            api_key: std::env::var("OPENAI_API_KEY")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            model: "text-embedding-3-small".to_string(),
        }
    }
}

impl EmbeddingProvider for OpenAiEmbeddingProvider {
    fn embed_text(&self, input: &str) -> Result<Vec<f32>> {
        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| anyhow!("OPENAI_API_KEY is not configured"))?;

        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("embedding input cannot be empty"));
        }

        let response = self
            .client
            .post("https://api.openai.com/v1/embeddings")
            .bearer_auth(api_key)
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

#[derive(Debug, Deserialize)]
struct OpenAiEmbeddingsResponse {
    data: Vec<OpenAiEmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct OpenAiEmbeddingData {
    embedding: Vec<f32>,
}
