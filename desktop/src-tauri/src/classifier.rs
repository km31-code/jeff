use anyhow::{anyhow, Context, Result};
use reqwest::blocking::Client;
use serde::Deserialize;
use std::time::Duration;

use crate::models::{IntentClassificationDto, IntentLabel, IntentSlotsDto};

const MODEL: &str = "gpt-4o-mini";
const REQUEST_TIMEOUT_MS: u64 = 300;

const SYSTEM_PROMPT: &str = "You are an intent classifier for a writing assistant called Jeff.\
\nGiven a user message, classify it into exactly one intent and extract structured slots.\
\n\
\nIntents:\
\n- answer: user wants information, explanation, or a question answered\
\n- revision: user wants to modify, edit, or improve an artifact (document, code, etc.)\
\n- subtask: user wants to delegate a bounded task for Jeff to execute independently\
\n- suggestion: user is asking for ideas, suggestions, or recommendations\
\n- unknown: intent is unclear or does not match any of the above\
\n\
\nSlots:\
\n- target_description: for revision, what artifact or section to revise\
\n- instruction: the core instruction text\
\n- draft_type: for subtask, the type of output (email, summary, plan, etc.)\
\n- topic: for answer/suggestion, the subject matter\
\n\
\nRespond with a JSON object only:\
\n{\
\n  \"intent\": \"answer\" | \"revision\" | \"subtask\" | \"suggestion\" | \"unknown\",\
\n  \"confidence\": <float 0.0-1.0>,\
\n  \"slots\": {\
\n    \"target_description\": <string or null>,\
\n    \"instruction\": <string or null>,\
\n    \"draft_type\": <string or null>,\
\n    \"topic\": <string or null>\
\n  }\
\n}";

#[derive(Debug, Deserialize)]
struct ApiResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ApiMessage,
}

#[derive(Debug, Deserialize)]
struct ApiMessage {
    content: String,
}

pub fn classify_intent(text: &str, api_key: &str) -> Result<IntentClassificationDto> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(IntentClassificationDto {
            intent: IntentLabel::Unknown,
            confidence: 0.0,
            slots: IntentSlotsDto::default(),
        });
    }

    let client = Client::builder()
        .timeout(Duration::from_millis(REQUEST_TIMEOUT_MS))
        .connect_timeout(Duration::from_millis(REQUEST_TIMEOUT_MS))
        .build()
        .context("failed to build HTTP client for intent classification")?;

    let response = client
        .post("https://api.openai.com/v1/chat/completions")
        .bearer_auth(api_key)
        .json(&serde_json::json!({
            "model": MODEL,
            "temperature": 0,
            "response_format": { "type": "json_object" },
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user", "content": trimmed }
            ]
        }))
        .send()
        .context("failed to call OpenAI chat completions for intent classification")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .unwrap_or_else(|_| "<unreadable>".to_string());
        return Err(anyhow!(
            "intent classifier request failed with status {}: {}",
            status,
            body
        ));
    }

    let payload: ApiResponse = response
        .json()
        .context("failed to parse intent classifier API response")?;

    let content = payload
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .ok_or_else(|| anyhow!("intent classifier response contained no choices"))?;

    parse_classification(&content)
}

fn parse_classification(content: &str) -> Result<IntentClassificationDto> {
    #[derive(Deserialize)]
    struct RawSlots {
        target_description: Option<String>,
        instruction: Option<String>,
        draft_type: Option<String>,
        topic: Option<String>,
    }

    #[derive(Deserialize)]
    struct Raw {
        intent: String,
        confidence: Option<f32>,
        slots: Option<RawSlots>,
    }

    let raw: Raw =
        serde_json::from_str(content).context("failed to parse intent classification JSON")?;

    let label = match raw.intent.to_lowercase().as_str() {
        "answer" => IntentLabel::Answer,
        "revision" => IntentLabel::Revision,
        "subtask" => IntentLabel::Subtask,
        "suggestion" => IntentLabel::Suggestion,
        _ => IntentLabel::Unknown,
    };

    let slots = raw
        .slots
        .map(|s| IntentSlotsDto {
            target_description: s.target_description,
            instruction: s.instruction,
            draft_type: s.draft_type,
            topic: s.topic,
        })
        .unwrap_or_default();

    Ok(IntentClassificationDto {
        intent: label,
        confidence: raw.confidence.unwrap_or(0.5).clamp(0.0, 1.0),
        slots,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_answer_classification() {
        let json = r#"{"intent":"answer","confidence":0.95,"slots":{"target_description":null,"instruction":"explain the codebase","draft_type":null,"topic":"architecture"}}"#;
        let result = parse_classification(json).unwrap();
        assert_eq!(result.intent, IntentLabel::Answer);
        assert!((result.confidence - 0.95).abs() < 0.01);
        assert_eq!(result.slots.topic.as_deref(), Some("architecture"));
        assert_eq!(
            result.slots.instruction.as_deref(),
            Some("explain the codebase")
        );
    }

    #[test]
    fn parse_valid_revision_classification() {
        let json = r#"{"intent":"revision","confidence":0.9,"slots":{"target_description":"the introduction","instruction":"make it shorter","draft_type":null,"topic":null}}"#;
        let result = parse_classification(json).unwrap();
        assert_eq!(result.intent, IntentLabel::Revision);
        assert_eq!(
            result.slots.target_description.as_deref(),
            Some("the introduction")
        );
        assert_eq!(result.slots.instruction.as_deref(), Some("make it shorter"));
    }

    #[test]
    fn parse_valid_subtask_classification() {
        let json = r#"{"intent":"subtask","confidence":0.88,"slots":{"target_description":null,"instruction":"draft a project summary","draft_type":"summary","topic":null}}"#;
        let result = parse_classification(json).unwrap();
        assert_eq!(result.intent, IntentLabel::Subtask);
        assert_eq!(result.slots.draft_type.as_deref(), Some("summary"));
    }

    #[test]
    fn parse_valid_suggestion_classification() {
        let json = r#"{"intent":"suggestion","confidence":0.75,"slots":{"target_description":null,"instruction":null,"draft_type":null,"topic":"project structure"}}"#;
        let result = parse_classification(json).unwrap();
        assert_eq!(result.intent, IntentLabel::Suggestion);
        assert_eq!(result.slots.topic.as_deref(), Some("project structure"));
    }

    #[test]
    fn parse_unknown_intent_label_maps_to_unknown() {
        let json = r#"{"intent":"something_new","confidence":0.3,"slots":{"target_description":null,"instruction":null,"draft_type":null,"topic":null}}"#;
        let result = parse_classification(json).unwrap();
        assert_eq!(result.intent, IntentLabel::Unknown);
    }

    #[test]
    fn parse_malformed_json_returns_error() {
        let result = parse_classification("not-valid-json");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("failed to parse intent classification JSON"));
    }

    #[test]
    fn parse_missing_slots_field_defaults_to_empty() {
        let json = r#"{"intent":"answer","confidence":0.8}"#;
        let result = parse_classification(json).unwrap();
        assert_eq!(result.intent, IntentLabel::Answer);
        assert!((result.confidence - 0.8).abs() < 0.01);
        assert!(result.slots.topic.is_none());
        assert!(result.slots.instruction.is_none());
    }

    #[test]
    fn parse_missing_confidence_defaults_to_half() {
        let json = r#"{"intent":"revision","slots":{"target_description":null,"instruction":null,"draft_type":null,"topic":null}}"#;
        let result = parse_classification(json).unwrap();
        assert!((result.confidence - 0.5).abs() < 0.01);
    }
}
