use anyhow::Result;

// re-export the canonical trait so all call sites use one definition.
// embedding::EmbeddingProvider resolves to providers::EmbeddingsProvider.
pub use crate::providers::EmbeddingsProvider as EmbeddingProvider;

#[derive(Clone)]
#[allow(dead_code)]
pub struct OpenAiEmbeddingProvider {
    inner: crate::providers::OpenAiEmbeddingsProvider,
}

#[allow(dead_code)]
impl OpenAiEmbeddingProvider {
    pub fn from_env() -> Self {
        Self {
            inner: crate::providers::OpenAiEmbeddingsProvider::from_env(),
        }
    }
}

impl crate::providers::EmbeddingsProvider for OpenAiEmbeddingProvider {
    fn embed_text(&self, input: &str) -> Result<Vec<f32>> {
        self.inner.embed_text(input)
    }

    fn model_id(&self) -> &'static str {
        self.inner.model_id()
    }
}
