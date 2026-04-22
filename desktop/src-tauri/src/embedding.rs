use anyhow::Result;

pub trait EmbeddingProvider: Send + Sync {
    fn embed_text(&self, input: &str) -> Result<Vec<f32>>;
}

#[derive(Clone)]
pub struct OpenAiEmbeddingProvider {
    inner: crate::providers::OpenAiEmbeddingsProvider,
}

impl OpenAiEmbeddingProvider {
    pub fn from_env() -> Self {
        Self {
            inner: crate::providers::OpenAiEmbeddingsProvider::from_env(),
        }
    }
}

impl EmbeddingProvider for OpenAiEmbeddingProvider {
    fn embed_text(&self, input: &str) -> Result<Vec<f32>> {
        use crate::providers::EmbeddingsProvider;
        self.inner.embed_text(input)
    }
}
