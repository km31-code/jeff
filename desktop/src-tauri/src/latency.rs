#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

pub const STARTUP_BUDGET_MS: u64 = 2000;
pub const FIRST_TOKEN_BUDGET_MS: u64 = 1000;
pub const FIRST_AUDIO_BUDGET_MS: u64 = 400;
pub const CLASSIFIER_BUDGET_MS: u64 = 150;

static LLM_INPUT_TOKENS: AtomicU64 = AtomicU64::new(0);
static LLM_OUTPUT_TOKENS: AtomicU64 = AtomicU64::new(0);
static LLM_CACHED_TOKENS: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct LlmCacheMetrics {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub cached_ratio: f64,
}

pub fn record_llm_usage(usage: crate::model_router::LlmUsage) -> LlmCacheMetrics {
    LLM_INPUT_TOKENS.fetch_add(usage.input_tokens, Ordering::Relaxed);
    LLM_OUTPUT_TOKENS.fetch_add(usage.output_tokens, Ordering::Relaxed);
    LLM_CACHED_TOKENS.fetch_add(usage.cached_tokens, Ordering::Relaxed);
    llm_cache_metrics()
}

pub fn llm_cache_metrics() -> LlmCacheMetrics {
    let input_tokens = LLM_INPUT_TOKENS.load(Ordering::Relaxed);
    let output_tokens = LLM_OUTPUT_TOKENS.load(Ordering::Relaxed);
    let cached_tokens = LLM_CACHED_TOKENS.load(Ordering::Relaxed);
    let cached_ratio = if input_tokens == 0 {
        0.0
    } else {
        cached_tokens as f64 / input_tokens as f64
    };

    LlmCacheMetrics {
        input_tokens,
        output_tokens,
        cached_tokens,
        cached_ratio,
    }
}

#[cfg(test)]
pub fn reset_llm_cache_metrics_for_test() {
    LLM_INPUT_TOKENS.store(0, Ordering::Relaxed);
    LLM_OUTPUT_TOKENS.store(0, Ordering::Relaxed);
    LLM_CACHED_TOKENS.store(0, Ordering::Relaxed);
}

// measures: provider struct construction (reqwest client init only).
// note: this does NOT cover actual startup latency (tray, hotkey, overlay,
// db open). db-open cost is covered by store::tests::store_cold_open_is_fast.
#[test]
fn provider_instantiation_is_fast() {
    use std::time::Instant;

    let started = Instant::now();
    // apex a1: reasoning and classification construct through the model
    // router; the router itself is config-only and instantiates instantly.
    let _router =
        crate::model_router::ModelRouter::new(crate::model_router::RouterConfig::default());
    let _stt = crate::providers::OpenAiSttProvider::from_env();
    let _tts = crate::providers::OpenAiTtsProvider::from_env();
    let _embeddings = crate::providers::OpenAiEmbeddingsProvider::from_env();
    let elapsed_ms = started.elapsed().as_millis() as u64;

    assert!(
        elapsed_ms < STARTUP_BUDGET_MS,
        "provider instantiation exceeded startup budget: {}ms >= {}ms",
        elapsed_ms,
        STARTUP_BUDGET_MS
    );
}

#[test]
fn a2_cached_ratio_accumulates_llm_usage() {
    reset_llm_cache_metrics_for_test();
    for turn in 0..20 {
        let cached_tokens = if turn == 0 { 0 } else { 900 };
        record_llm_usage(crate::model_router::LlmUsage {
            input_tokens: 1000,
            output_tokens: 50,
            cached_tokens,
        });
    }

    let metrics = llm_cache_metrics();
    assert_eq!(metrics.input_tokens, 20_000);
    assert_eq!(metrics.cached_tokens, 17_100);
    assert!(
        metrics.cached_ratio > 0.70,
        "expected cached ratio > 70%, got {:.1}%",
        metrics.cached_ratio * 100.0
    );
}
