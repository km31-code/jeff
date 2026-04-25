use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

use crate::models::{SpeechSynthesisDto, TranscriptionResultDto};

#[derive(Clone)]
pub struct OpenAiVoiceProvider {
    stt: crate::providers::OpenAiSttProvider,
    tts: crate::providers::OpenAiTtsProvider,
}

impl OpenAiVoiceProvider {
    pub fn from_env() -> Self {
        Self {
            stt: crate::providers::OpenAiSttProvider::from_env(),
            tts: crate::providers::OpenAiTtsProvider::from_env(),
        }
    }

    pub fn transcribe_audio_base64(
        &self,
        audio_base64: &str,
        mime_type: &str,
    ) -> Result<TranscriptionResultDto> {
        let audio_bytes = BASE64
            .decode(audio_base64.trim())
            .context("failed to decode audio_base64 payload")?;

        if audio_bytes.is_empty() {
            return Err(anyhow!("audio payload is empty"));
        }

        use crate::providers::SpeechToTextProvider;
        let text = self.stt.transcribe(&audio_bytes, mime_type)?;
        let cleaned = text.trim().to_string();
        if cleaned.is_empty() {
            return Err(anyhow!("transcription returned empty text"));
        }

        Ok(TranscriptionResultDto { text: cleaned })
    }

    pub fn synthesize_speech(&self, text: &str, voice: &str) -> Result<SpeechSynthesisDto> {
        let clean_text = text.trim();
        if clean_text.is_empty() {
            return Err(anyhow!("speech synthesis text cannot be empty"));
        }

        use crate::providers::TextToSpeechProvider;
        let audio_bytes = self.tts.synthesize(clean_text, voice)?;

        Ok(SpeechSynthesisDto {
            audio_base64: BASE64.encode(audio_bytes),
            mime_type: "audio/mpeg".to_string(),
        })
    }
}

impl crate::providers::VoiceProvider for OpenAiVoiceProvider {
    fn transcribe_audio_base64(
        &self,
        audio_base64: &str,
        mime_type: &str,
    ) -> Result<TranscriptionResultDto> {
        self.transcribe_audio_base64(audio_base64, mime_type)
    }

    fn synthesize_speech(&self, text: &str, voice: &str) -> Result<SpeechSynthesisDto> {
        self.synthesize_speech(text, voice)
    }
}
