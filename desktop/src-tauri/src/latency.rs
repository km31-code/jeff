pub const STARTUP_BUDGET_MS: u64 = 2000;
pub const FIRST_TOKEN_BUDGET_MS: u64 = 1000;
pub const FIRST_AUDIO_BUDGET_MS: u64 = 400;
pub const CLASSIFIER_BUDGET_MS: u64 = 150;

#[test]
fn startup_budget_is_met() {
    use std::time::Instant;

    let started = Instant::now();
    let _reasoning = crate::providers::OpenAiReasoningProvider::from_env();
    let _stt = crate::providers::OpenAiSttProvider::from_env();
    let _tts = crate::providers::OpenAiTtsProvider::from_env();
    let _embeddings = crate::providers::OpenAiEmbeddingsProvider::from_env();
    let _classifier = crate::providers::OpenAiClassifierProvider::new();
    let elapsed_ms = started.elapsed().as_millis() as u64;

    assert!(
        elapsed_ms < STARTUP_BUDGET_MS,
        "startup exceeded budget: {}ms >= {}ms",
        elapsed_ms,
        STARTUP_BUDGET_MS
    );
}
