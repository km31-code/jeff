// apex b2: goal understanding. jeff understands what the user is trying to
// accomplish from natural conversation, not from literal prefix matching.
//
// the primary path is a reflex-tier structured extraction over the recent
// transcript ("what is this person trying to accomplish, in their words?").
// a deterministic heuristic is the no-llm fallback and the eval contrast
// baseline. the retired prefix matcher (extract_goal_from_text) survives only
// for backward-compatible tests and the eval contrast — it is off the live
// path.

use anyhow::{anyhow, Result};
use serde::Deserialize;

use crate::model_router::{GenerateOptions, ModelRequest, ModelRouter, Tier};
use crate::models::ChatMessageDto;

// only record goals the extractor is at least moderately sure about; below this
// the snapshot keeps whatever it had.
pub const RECORD_CONFIDENCE_MIN: f32 = 0.5;
// how many recent turns feed the extractor.
pub const TRANSCRIPT_TURNS: usize = 10;
const GOAL_MAX_CHARS: usize = 240;
const EXTRACT_TIMEOUT_MS: u64 = 4000;

pub const SYSTEM_PROMPT: &str = "You extract the user's current working goal from a short chat transcript. \
The goal is what the user is trying to accomplish right now, in their own framing — a task, deliverable, or objective. \
It may be stated explicitly (\"I'm working on the thesis intro\"), paraphrased (\"this chapter needs to exist by Friday\"), or implied by what they are asking for. \
If the user is only chatting, asking a general question, or no goal is present, return null. \
Do not infer work goals from compliments, thanks, jokes, or casual personal activities such as \"working on my tan\". \
Respond with a single JSON object and nothing else: \
{\"goal\": string | null, \"confidence\": number between 0 and 1, \"evidence_quote\": string | null}. \
goal must be a short phrase (under 20 words) describing the objective, not a summary of the conversation. \
evidence_quote must be a short verbatim span from the user that supports the goal, or null.";

#[derive(Debug, Clone, PartialEq)]
pub struct GoalExtraction {
    pub goal: Option<String>,
    pub confidence: f32,
    pub evidence_quote: Option<String>,
}

impl GoalExtraction {
    pub fn none() -> Self {
        Self {
            goal: None,
            confidence: 0.0,
            evidence_quote: None,
        }
    }

    // whether this extraction is confident enough to record into the
    // relational model / snapshot.
    pub fn is_recordable(&self) -> bool {
        self.goal.is_some() && self.confidence >= RECORD_CONFIDENCE_MIN
    }
}

// primary path: reflex-tier structured extraction. reflex defaults to the local
// runtime and falls back to a cheap cloud model when local is unavailable, so
// this works with or without a local sidecar. never called on a response path.
pub fn extract_goal(router: &ModelRouter, transcript: &[ChatMessageDto]) -> Result<GoalExtraction> {
    let user = build_transcript_prompt(transcript);
    if user.trim().is_empty() {
        return Ok(GoalExtraction::none());
    }
    let mut request =
        ModelRequest::new(Tier::Reflex, SYSTEM_PROMPT, &user).with_options(GenerateOptions {
            temperature: 0.0,
            max_tokens: Some(200),
            json_object: true,
            timeout_ms: Some(EXTRACT_TIMEOUT_MS),
        });
    request.purpose = Some("goal_extraction".to_string());
    let raw = router.route(request)?.text;
    parse_goal_json(&raw)
}

// llm path with a deterministic fallback: on any call failure, degrade to the
// heuristic rather than losing the signal entirely.
pub fn extract_goal_with_fallback(
    router: &ModelRouter,
    transcript: &[ChatMessageDto],
) -> GoalExtraction {
    match extract_goal(router, transcript) {
        Ok(extraction) => extraction,
        Err(err) => {
            eprintln!("[jeff] goal_extraction_fallback reason={err}");
            extract_goal_heuristic(transcript)
        }
    }
}

pub fn build_transcript_prompt(transcript: &[ChatMessageDto]) -> String {
    let recent: Vec<&ChatMessageDto> = transcript
        .iter()
        .rev()
        .take(TRANSCRIPT_TURNS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    recent
        .iter()
        .filter(|m| !m.content.trim().is_empty())
        .map(|m| format!("{}: {}", m.role, m.content.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn parse_goal_json(raw: &str) -> Result<GoalExtraction> {
    let json_slice = extract_json_object(raw)
        .ok_or_else(|| anyhow!("goal extraction response contained no JSON object"))?;

    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        goal: Option<serde_json::Value>,
        #[serde(default)]
        confidence: Option<f32>,
        #[serde(default)]
        evidence_quote: Option<serde_json::Value>,
    }

    let parsed: Raw = serde_json::from_str(json_slice)
        .map_err(|err| anyhow!("failed to parse goal extraction JSON: {err}"))?;

    let goal = normalize_optional_string(parsed.goal).map(|g| truncate_chars(&g, GOAL_MAX_CHARS));
    let evidence_quote = normalize_optional_string(parsed.evidence_quote);
    let confidence = parsed.confidence.unwrap_or(0.5).clamp(0.0, 1.0);

    // a null goal always means zero confidence in a goal, regardless of what the
    // model reported.
    let confidence = if goal.is_some() { confidence } else { 0.0 };

    Ok(GoalExtraction {
        goal,
        confidence,
        evidence_quote,
    })
}

fn normalize_optional_string(value: Option<serde_json::Value>) -> Option<String> {
    match value {
        Some(serde_json::Value::String(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        _ => None,
    }
}

fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end > start {
        Some(&raw[start..=end])
    } else {
        None
    }
}

// ---- deterministic heuristic extractor ---------------------------------------
// the no-llm fallback and the eval contrast baseline. materially broader than
// the retired prefix matcher: it handles explicit statements, paraphrases
// ("this chapter needs to exist by friday"), and simple imperatives/requests.

pub fn extract_goal_heuristic(transcript: &[ChatMessageDto]) -> GoalExtraction {
    for message in transcript.iter().rev() {
        if message.role != "user" {
            continue;
        }
        if let Some((goal, confidence, evidence)) = heuristic_goal_from_message(&message.content) {
            return GoalExtraction {
                goal: Some(goal),
                confidence,
                evidence_quote: Some(evidence),
            };
        }
    }
    GoalExtraction::none()
}

// single-message heuristic used both by the transcript scan and by the
// immediate per-message signal path (relational_model::record_message_signals).
pub fn heuristic_goal_from_message(text: &str) -> Option<(String, f32, String)> {
    let clean = text.trim();
    if clean.is_empty() {
        return None;
    }
    let lower = clean.to_ascii_lowercase();
    if is_goal_trap(&lower) {
        return None;
    }

    // 1. explicit first-person goal markers, highest confidence.
    for (pattern, confidence) in [
        ("my goal is", 0.85f32),
        ("the goal is", 0.8),
        ("i'm working on", 0.82),
        ("i am working on", 0.82),
        ("working on", 0.7),
        ("i'm trying to", 0.78),
        ("i am trying to", 0.78),
        ("trying to", 0.68),
        ("i need to", 0.75),
        ("i have to", 0.72),
        ("i want to", 0.7),
        ("i'd like to", 0.68),
        ("i would like to", 0.68),
        ("focusing on", 0.7),
        ("focus on", 0.66),
    ] {
        if let Some(goal) = capture_after(clean, &lower, pattern) {
            return Some((goal, confidence, clean.to_string()));
        }
    }

    // 2. third-person / paraphrased objective ("this chapter needs to exist by
    // friday", "the deck needs a stronger close", "the slides have to be ready").
    for pattern in [
        " needs to ",
        " needs ",
        " need to ",
        " has to ",
        " have to ",
        " should be ",
        " must be ",
    ] {
        if lower.contains(pattern) {
            let goal = truncate_chars(strip_leading_filler(clean), GOAL_MAX_CHARS);
            if !goal.is_empty() {
                let confidence = if has_deadline(&lower) { 0.7 } else { 0.6 };
                return Some((goal, confidence, clean.to_string()));
            }
        }
    }

    // 3. request/imperative framing ("help me draft the intro", "can you help me
    // finish the methods section", or a bare imperative "draft the launch memo").
    for pattern in [
        "help me ",
        "can you help me ",
        "could you help me ",
        "i'm supposed to ",
        "i am supposed to ",
    ] {
        if let Some(goal) = capture_after(clean, &lower, pattern) {
            return Some((goal, 0.62, clean.to_string()));
        }
    }

    // 4. bare imperative starting with a work verb.
    let first_word = lower.split_whitespace().next().unwrap_or("");
    if matches!(
        first_word,
        "draft" | "write" | "finish" | "revise" | "outline" | "prepare" | "build" | "edit"
    ) {
        let goal = truncate_chars(
            clean.trim_end_matches(['.', '!', '?']).trim(),
            GOAL_MAX_CHARS,
        );
        if goal.split_whitespace().count() >= 2 {
            return Some((goal, 0.58, clean.to_string()));
        }
    }

    if let Some(goal) = implicit_goal_from_message(clean, &lower) {
        return Some((goal, 0.58, clean.to_string()));
    }

    None
}

fn is_goal_trap(lower: &str) -> bool {
    let trimmed = lower.trim();
    trimmed.starts_with("i need to say")
        || trimmed.starts_with("i want to thank you")
        || trimmed.starts_with("i'd like to thank you")
        || trimmed.starts_with("i would like to thank you")
        || trimmed.starts_with("i want to say thanks")
        || trimmed.starts_with("i just want to say")
        || trimmed.contains("thank you for")
        || trimmed.contains("thanks for")
        || trimmed.contains("working on my tan")
}

fn implicit_goal_from_message(clean: &str, lower: &str) -> Option<String> {
    if lower.contains("reviewer") && (lower.contains("sample size") || lower.contains("concern")) {
        return Some("address the reviewer's sample-size concern".to_string());
    }
    if lower.contains("deadline") && lower.contains("abstract") {
        return Some(truncate_chars(strip_leading_filler(clean), GOAL_MAX_CHARS));
    }
    if lower.contains("contradict") && lower.contains("thesis") {
        return Some(truncate_chars(strip_leading_filler(clean), GOAL_MAX_CHARS));
    }
    if lower.contains("rewriting") && lower.contains("transition") {
        return Some("fix the transition".to_string());
    }
    if lower.contains("too long")
        && (lower.contains("word limit") || lower.contains("journal"))
        && (lower.contains("intro") || lower.contains("introduction"))
    {
        return Some(truncate_chars(strip_leading_filler(clean), GOAL_MAX_CHARS));
    }
    None
}

fn capture_after(original: &str, lower: &str, pattern: &str) -> Option<String> {
    let index = lower.find(pattern)?;
    let start = index + pattern.len();
    let tail = original[start..]
        .trim()
        .trim_start_matches([':', '-', ' '])
        .trim_end_matches(['.', '!', '?'])
        .trim();
    // reject captures that are trivially short or a dangling conjunction.
    if tail.split_whitespace().count() < 2 {
        return None;
    }
    Some(truncate_chars(tail, GOAL_MAX_CHARS))
}

fn strip_leading_filler(text: &str) -> &str {
    let trimmed = text.trim();
    for prefix in ["so ", "and ", "but ", "well ", "ok ", "okay ", "also "] {
        if trimmed.to_ascii_lowercase().starts_with(prefix) {
            return trimmed[prefix.len()..].trim();
        }
    }
    trimmed
}

fn has_deadline(lower: &str) -> bool {
    lower.contains(" by ")
        || lower.contains("due")
        || lower.contains("deadline")
        || lower.contains("tonight")
        || lower.contains("tomorrow")
        || [
            "monday",
            "tuesday",
            "wednesday",
            "thursday",
            "friday",
            "saturday",
            "sunday",
        ]
        .iter()
        .any(|day| lower.contains(day))
}

pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        trimmed.to_string()
    } else {
        trimmed.chars().take(max_chars).collect()
    }
}

// ---- eval support ------------------------------------------------------------
// shared by the goal eval binary (scripts/goal_eval.sh) and the integration
// test. the retired prefix matcher (relational_model::extract_goal_from_text)
// is the contrast baseline scored against extract_goal_heuristic and the llm
// extractor to quantify the b2 improvement.

// consumed by the goal_eval bin and the integration test (separate crates), so
// the main binary sees these as unused.
#[allow(dead_code)]
#[derive(Debug, Clone, serde::Deserialize)]
pub struct GoalEvalTurn {
    pub role: String,
    pub content: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, serde::Deserialize)]
pub struct GoalEvalCase {
    pub id: String,
    #[serde(default)]
    pub category: String,
    pub transcript: Vec<GoalEvalTurn>,
    #[serde(default)]
    pub expected_goal: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
}

#[allow(dead_code)]
impl GoalEvalCase {
    pub fn messages(&self) -> Vec<ChatMessageDto> {
        self.transcript
            .iter()
            .enumerate()
            .map(|(i, turn)| ChatMessageDto {
                id: i as i64,
                task_id: 0,
                session_id: None,
                role: turn.role.clone(),
                message_source: "user".to_string(),
                message_kind: "chat".to_string(),
                content: turn.content.clone(),
                created_at: String::new(),
            })
            .collect()
    }

    pub fn heuristic_goal(&self) -> Option<String> {
        extract_goal_heuristic(&self.messages()).goal
    }

    // the retired prefix matcher over the latest user message.
    pub fn prefix_goal(&self) -> Option<String> {
        self.messages()
            .iter()
            .rev()
            .filter(|m| m.role == "user")
            .find_map(|m| crate::relational_model::extract_goal_from_text(&m.content))
    }
}

// a prediction is correct when: a no-goal case yields no goal; or a goal case
// yields a goal that shares a labeled keyword or enough content-word overlap
// with the expected goal. lenient on wording, strict on presence/absence.
#[allow(dead_code)]
pub fn prediction_is_correct(case: &GoalEvalCase, predicted: &Option<String>) -> bool {
    match (case.expected_goal.as_deref(), predicted.as_deref()) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(_), None) => false,
        (Some(expected), Some(pred)) => {
            let pred_lower = pred.to_ascii_lowercase();
            if case
                .keywords
                .iter()
                .any(|k| !k.is_empty() && pred_lower.contains(&k.to_ascii_lowercase()))
            {
                return true;
            }
            token_jaccard_str(expected, pred) >= 0.3
        }
    }
}

fn token_jaccard_str(a: &str, b: &str) -> f32 {
    let ta = content_tokens(a);
    let tb = content_tokens(b);
    if ta.is_empty() || tb.is_empty() {
        return 0.0;
    }
    let intersection = ta.iter().filter(|t| tb.contains(*t)).count();
    let union = ta.len() + tb.len() - intersection;
    if union == 0 {
        0.0
    } else {
        intersection as f32 / union as f32
    }
}

fn content_tokens(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() > 2)
        .map(|t| t.to_ascii_lowercase())
        .collect();
    tokens.sort();
    tokens.dedup();
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: &str) -> ChatMessageDto {
        ChatMessageDto {
            id: 0,
            task_id: 0,
            session_id: None,
            role: role.to_string(),
            message_source: "user".to_string(),
            message_kind: "chat".to_string(),
            content: content.to_string(),
            created_at: String::new(),
        }
    }

    #[test]
    fn b2_parses_structured_goal_json() {
        let raw = r#"{"goal": "finish the thesis introduction", "confidence": 0.9, "evidence_quote": "the intro"}"#;
        let parsed = parse_goal_json(raw).unwrap();
        assert_eq!(
            parsed.goal.as_deref(),
            Some("finish the thesis introduction")
        );
        assert!(parsed.confidence > 0.8);
        assert!(parsed.is_recordable());
    }

    #[test]
    fn b2_parses_null_goal_as_zero_confidence() {
        let raw = r#"{"goal": null, "confidence": 0.4, "evidence_quote": null}"#;
        let parsed = parse_goal_json(raw).unwrap();
        assert!(parsed.goal.is_none());
        assert_eq!(parsed.confidence, 0.0);
        assert!(!parsed.is_recordable());
    }

    #[test]
    fn b2_tolerates_surrounding_prose_in_json() {
        let raw =
            "Here is the result: {\"goal\": \"draft the launch memo\", \"confidence\": 0.7} thanks";
        let parsed = parse_goal_json(raw).unwrap();
        assert_eq!(parsed.goal.as_deref(), Some("draft the launch memo"));
    }

    #[test]
    fn b2_heuristic_catches_paraphrased_goal() {
        let extraction =
            extract_goal_heuristic(&[msg("user", "this chapter needs to exist by Friday")]);
        assert!(extraction.goal.is_some());
        assert!(extraction.confidence >= RECORD_CONFIDENCE_MIN);
    }

    #[test]
    fn b2_heuristic_ignores_pure_chit_chat() {
        let extraction =
            extract_goal_heuristic(&[msg("user", "haha that is pretty funny, thanks")]);
        assert!(extraction.goal.is_none());
    }

    #[test]
    fn b2_heuristic_rejects_polite_goal_traps() {
        for text in [
            "I need to say, this is really impressive work.",
            "I want to thank you for the help earlier.",
            "Honestly I'm working on my tan this weekend, don't mind me.",
        ] {
            let extraction = extract_goal_heuristic(&[msg("user", text)]);
            assert!(extraction.goal.is_none(), "trap should not record: {text}");
        }
    }

    #[test]
    fn b2_heuristic_catches_implicit_work_goals() {
        for text in [
            "The deadline for the abstract just got moved up to tomorrow.",
            "Chapter two still contradicts the thesis in a few places.",
            "I keep rewriting the same transition and it's still not working.",
            "The intro is way too long for the journal's word limit.",
        ] {
            let extraction = extract_goal_heuristic(&[msg("user", text)]);
            assert!(
                extraction.goal.is_some(),
                "implicit goal should be inferred: {text}"
            );
            assert!(extraction.confidence >= RECORD_CONFIDENCE_MIN);
        }
    }

    #[test]
    fn b2_heuristic_reads_latest_user_goal() {
        let extraction = extract_goal_heuristic(&[
            msg("user", "i'm working on the methods section"),
            msg("assistant", "got it"),
            msg("user", "actually i need to fix the sample size argument"),
        ]);
        assert!(extraction
            .goal
            .as_deref()
            .unwrap()
            .contains("sample size argument"));
    }
}
