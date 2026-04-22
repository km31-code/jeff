#![allow(dead_code)]

pub trait SpeechToTextProvider {
    fn transcribe(&self, _audio_bytes: &[u8]) -> Result<String, String>;
}

pub trait TextToSpeechProvider {
    fn synthesize(&self, _text: &str) -> Result<Vec<u8>, String>;
}

pub trait ReasoningModelProvider {
    fn generate_response(&self, _prompt: &str) -> Result<String, String>;
}

pub trait EmbeddingsProvider {
    fn embed_text(&self, _text: &str) -> Result<Vec<f32>, String>;
}
