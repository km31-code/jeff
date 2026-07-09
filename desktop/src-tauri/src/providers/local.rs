use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    sync::Arc,
};

use anyhow::{anyhow, Result};

use crate::{
    local_runtime::{
        LocalRuntime, LOCAL_EMBEDDING_MODEL_ID, LOCAL_SEMANTIC_EMBEDDING_MODEL_ID,
    },
    model_router::LlmUsage,
    models::{IntentClassificationDto, IntentLabel, IntentSlotsDto},
    providers::{EmbeddingsProvider, ReasoningModelProvider},
};

const HASH_EMBEDDING_DIMS: usize = 256;

#[derive(Clone)]
pub struct LocalReasoningProvider {
    runtime: Arc<LocalRuntime>,
    model: String,
}

impl LocalReasoningProvider {
    pub fn new(runtime: Arc<LocalRuntime>, model: impl Into<String>) -> Self {
        Self {
            runtime,
            model: model.into(),
        }
    }

    pub fn generate_with_usage(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        temperature: f32,
        max_tokens: Option<u32>,
        json_object: bool,
    ) -> Result<(String, LlmUsage)> {
        if is_classifier_prompt(system_prompt) {
            let status = self.runtime.status();
            if status.sidecar_configured && status.reasoning_model_present {
                let text = self.runtime.chat_completion(
                    &self.model,
                    system_prompt,
                    user_prompt,
                    temperature,
                    max_tokens,
                    json_object,
                )?;
                return Ok((
                    text,
                    LlmUsage {
                        input_tokens: estimate_tokens(system_prompt) + estimate_tokens(user_prompt),
                        output_tokens: 0,
                        cached_tokens: 0,
                    },
                ));
            }
            return Ok((
                classification_json(user_prompt)?,
                LlmUsage {
                    input_tokens: estimate_tokens(system_prompt) + estimate_tokens(user_prompt),
                    output_tokens: 80,
                    cached_tokens: 0,
                },
            ));
        }

        let text = self.runtime.chat_completion(
            &self.model,
            system_prompt,
            user_prompt,
            temperature,
            max_tokens,
            json_object,
        )?;
        Ok((
            text,
            LlmUsage {
                input_tokens: estimate_tokens(system_prompt) + estimate_tokens(user_prompt),
                output_tokens: 0,
                cached_tokens: 0,
            },
        ))
    }
}

impl ReasoningModelProvider for LocalReasoningProvider {
    fn generate_response(&self, system_prompt: &str, user_prompt: &str) -> Result<String> {
        Ok(self
            .generate_with_usage(system_prompt, user_prompt, 0.0, None, false)?
            .0)
    }
}

#[derive(Clone)]
pub struct LocalEmbeddingProvider {
    runtime: Arc<LocalRuntime>,
}

impl LocalEmbeddingProvider {
    pub fn new(runtime: Arc<LocalRuntime>) -> Self {
        Self { runtime }
    }
}

impl EmbeddingsProvider for LocalEmbeddingProvider {
    fn embed_text(&self, input: &str) -> Result<Vec<f32>> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("embedding input cannot be empty"));
        }

        // apex b1: prefer the real semantic model when installed and healthy.
        // model_id() gates on the same capability, so a vector and its stored
        // model tag always agree. if a specific sidecar embed fails we mark the
        // capability stale so subsequent calls (and model_id) fall back to
        // lexical hashing rather than tagging a hash vector as semantic.
        if self.runtime.semantic_embedding_available() {
            match self.runtime.embed_text_via_sidecar(trimmed) {
                Ok(embedding) if !embedding.is_empty() => return Ok(embedding),
                Ok(_) => self.runtime.mark_embedding_capability_stale(),
                Err(err) => {
                    eprintln!("[jeff] local_embedding_sidecar_failed: {err}");
                    self.runtime.mark_embedding_capability_stale();
                }
            }
        }

        Ok(hash_embedding(trimmed))
    }

    fn model_id(&self) -> &'static str {
        if self.runtime.semantic_embedding_available() {
            LOCAL_SEMANTIC_EMBEDDING_MODEL_ID
        } else {
            LOCAL_EMBEDDING_MODEL_ID
        }
    }
}

pub fn is_classifier_prompt(system_prompt: &str) -> bool {
    system_prompt.contains("intent classifier") && system_prompt.contains("Intents:")
}

pub fn classification_json(input: &str) -> Result<String> {
    serde_json::to_string(&classify_intent_locally(input))
        .map_err(|err| anyhow!("failed to serialize local classification: {err}"))
}

pub fn classify_intent_locally(input: &str) -> IntentClassificationDto {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return IntentClassificationDto {
            intent: IntentLabel::Unknown,
            confidence: 0.0,
            slots: IntentSlotsDto::default(),
        };
    }

    let lower = trimmed.to_ascii_lowercase();
    let mut slots = IntentSlotsDto::default();
    slots.instruction = Some(trimmed.to_string());

    let (intent, confidence) = if contains_any(
        &lower,
        &[
            "revise",
            "revision",
            "rewrite",
            "edit",
            "tighten",
            "shorten",
            "improve",
            "polish",
            "make this",
            "change this",
            "fix this",
            "update this",
        ],
    ) {
        slots.target_description = extract_target_description(trimmed);
        (IntentLabel::Revision, 0.86)
    } else if contains_any(
        &lower,
        &[
            "draft",
            "write",
            "create",
            "generate",
            "build",
            "make a plan",
            "turn this into",
            "put together",
            "summarize this for me",
        ],
    ) {
        slots.draft_type = infer_draft_type(&lower);
        (IntentLabel::Subtask, 0.78)
    } else if contains_any(
        &lower,
        &[
            "suggest",
            "recommend",
            "ideas",
            "what should",
            "which option",
            "brainstorm",
        ],
    ) {
        slots.topic = Some(trimmed.to_string());
        (IntentLabel::Suggestion, 0.74)
    } else if contains_any(
        &lower,
        &[
            "what",
            "why",
            "how",
            "explain",
            "tell me",
            "walk me through",
            "difference",
            "compare",
        ],
    ) || lower.ends_with('?')
    {
        slots.topic = Some(trimmed.trim_end_matches('?').to_string());
        (IntentLabel::Answer, 0.8)
    } else {
        slots.instruction = None;
        (IntentLabel::Unknown, 0.35)
    };

    IntentClassificationDto {
        intent,
        confidence,
        slots,
    }
}

pub fn hash_embedding(input: &str) -> Vec<f32> {
    let mut vector = vec![0.0_f32; HASH_EMBEDDING_DIMS];
    let tokens = tokenize(input);
    for token in tokens {
        let mut hasher = DefaultHasher::new();
        token.hash(&mut hasher);
        let index = (hasher.finish() as usize) % HASH_EMBEDDING_DIMS;
        vector[index] += 1.0;
    }
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn extract_target_description(input: &str) -> Option<String> {
    for marker in ["this", "the ", "my "] {
        if let Some(index) = input.to_ascii_lowercase().find(marker) {
            return Some(input[index..].chars().take(80).collect::<String>());
        }
    }
    None
}

fn infer_draft_type(lower: &str) -> Option<String> {
    for kind in [
        "email",
        "summary",
        "plan",
        "outline",
        "intro",
        "paragraph",
        "memo",
    ] {
        if lower.contains(kind) {
            return Some(kind.to_string());
        }
    }
    None
}

fn tokenize(input: &str) -> Vec<String> {
    input
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .map(|token| token.trim().to_ascii_lowercase())
        .filter(|token| token.len() > 1)
        .collect()
}

fn estimate_tokens(input: &str) -> u64 {
    (input.chars().count() as u64 / 4).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        classifier,
        local_runtime::LOCAL_REASONING_MODEL_ID,
    };

    #[test]
    fn a3_local_reflex_classifier_handles_revision_without_api_key() {
        let classification = classify_intent_locally("rewrite this intro so it is tighter");
        assert_eq!(classification.intent, IntentLabel::Revision);
        assert!(classification.confidence > 0.8);
        assert!(classification.slots.instruction.is_some());
    }

    #[test]
    fn a3_classification_json_parses_through_existing_parser() {
        let raw =
            classification_json("what is the difference between a mutex and a semaphore?").unwrap();
        let parsed = classifier::parse_classification(&raw).unwrap();
        assert_eq!(parsed.intent, IntentLabel::Answer);
    }

    #[test]
    fn a3_hash_embedding_is_deterministic_normalized_and_nonempty() {
        let left = hash_embedding("alpha beta beta");
        let right = hash_embedding("alpha beta beta");
        assert_eq!(left, right);
        assert_eq!(left.len(), HASH_EMBEDDING_DIMS);
        let norm = left.iter().map(|value| value * value).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.001);
    }

    #[test]
    fn a3_local_embedding_provider_reports_local_model_id() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Arc::new(LocalRuntime::new(dir.path()));
        let provider = LocalEmbeddingProvider::new(runtime);
        assert_eq!(provider.model_id(), LOCAL_EMBEDDING_MODEL_ID);
        assert!(!provider.embed_text("alpha beta").unwrap().is_empty());
    }

    #[test]
    fn a3_local_reasoning_provider_short_circuits_classifier_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Arc::new(LocalRuntime::new(dir.path()));
        let provider = LocalReasoningProvider::new(runtime, LOCAL_REASONING_MODEL_ID);
        let raw = provider
            .generate_response(classifier::SYSTEM_PROMPT, "suggest options for the intro")
            .unwrap();
        let parsed = classifier::parse_classification(&raw).unwrap();
        assert_eq!(parsed.intent, IntentLabel::Suggestion);
    }
}
