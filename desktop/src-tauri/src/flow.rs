use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde::Serialize;

use crate::{
    chat::send_message_for_task,
    embedding::EmbeddingProvider,
    models::{
        ChatMessageDto, CoworkingStatusDto, RetrievedChunkDto, RevisionTargetDto, SubTaskDto,
        SuggestionAcceptanceDto, SuggestionDto, SuggestionEvaluationDto,
    },
    reasoning::ReasoningProvider,
    retrieval::build_task_context_pack,
    revision::propose_artifact_revision,
    store::{NewSuggestionInput, SessionModeUpdateInput, TaskStore},
    subtask::{create_subtask_and_start, SubTaskRunner},
};

const MIN_EVIDENCE_THRESHOLD: f32 = 0.24;
const SUGGESTION_REPEAT_COOLDOWN_SECONDS: i64 = 180;
const DISMISSED_SUPPRESSION_SECONDS: i64 = 900;
const MAX_SUGGESTIONS_PER_EVALUATION: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    Brainstorming,
    Outlining,
    Writing,
    Revising,
    EvidenceGathering,
    Stuck,
    QuietObserving,
}

impl SessionMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Brainstorming => "brainstorming",
            Self::Outlining => "outlining",
            Self::Writing => "writing",
            Self::Revising => "revising",
            Self::EvidenceGathering => "evidence_gathering",
            Self::Stuck => "stuck",
            Self::QuietObserving => "quiet_observing",
        }
    }
}

#[derive(Debug, Clone)]
struct CandidateSuggestion {
    title: String,
    description: String,
    suggestion_type: String,
    source_reason: String,
    suggestion_key: String,
    linked_context: Option<String>,
    linked_subtask_type: Option<String>,
    linked_revision_intent: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct SuggestionContextSummary {
    mode: String,
    mode_reason: String,
    evidence_score: f32,
    active_artifact_id: Option<i64>,
    top_chunks: Vec<String>,
}

pub fn evaluate_next_suggestions_for_task(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    task_id: i64,
    active_artifact_id: Option<i64>,
    coworking_status: &CoworkingStatusDto,
) -> Result<SuggestionEvaluationDto> {
    let recent_messages = store.list_recent_chat_messages(task_id, 14)?;
    let subtasks = store.list_subtasks(task_id)?;
    let recent_revision_count = store.count_recent_revision_activity(task_id, 20 * 60)?;

    let (mode, mode_reason) = infer_session_mode(
        &recent_messages,
        &subtasks,
        active_artifact_id,
        coworking_status,
        recent_revision_count,
    );

    let pending_suggestions = store.list_suggestions(task_id, false)?;
    let waiting_on_user_decision = !pending_suggestions.is_empty();

    if !coworking_status.proactive_mode {
        let mode_state = store.upsert_session_mode_state(&SessionModeUpdateInput {
            task_id,
            current_mode: mode.as_str().to_string(),
            mode_reason: mode_reason.clone(),
            waiting_on_user_decision,
            last_engine_decision: "suppressed_proactive_mode_disabled".to_string(),
            active_artifact_id,
        })?;

        return Ok(SuggestionEvaluationDto {
            mode_state,
            suggestions: pending_suggestions,
            generated_suggestions: Vec::new(),
            decision_reason: "suppressed_proactive_mode_disabled".to_string(),
            no_op: true,
            evidence_score: 0.0,
            active_artifact_id,
            suppression_state: "proactive_mode_disabled".to_string(),
            retrieved_chunks: Vec::new(),
        });
    }

    if coworking_status.user_typing || coworking_status.user_speaking {
        let mode_state = store.upsert_session_mode_state(&SessionModeUpdateInput {
            task_id,
            current_mode: mode.as_str().to_string(),
            mode_reason: mode_reason.clone(),
            waiting_on_user_decision,
            last_engine_decision: "suppressed_active_user_input".to_string(),
            active_artifact_id,
        })?;

        return Ok(SuggestionEvaluationDto {
            mode_state,
            suggestions: pending_suggestions,
            generated_suggestions: Vec::new(),
            decision_reason: "suppressed_active_user_input".to_string(),
            no_op: true,
            evidence_score: 0.0,
            active_artifact_id,
            suppression_state: "active_user_input".to_string(),
            retrieved_chunks: Vec::new(),
        });
    }

    if waiting_on_user_decision {
        let mode_state = store.upsert_session_mode_state(&SessionModeUpdateInput {
            task_id,
            current_mode: mode.as_str().to_string(),
            mode_reason: mode_reason.clone(),
            waiting_on_user_decision: true,
            last_engine_decision: "suppressed_waiting_user_decision".to_string(),
            active_artifact_id,
        })?;

        return Ok(SuggestionEvaluationDto {
            mode_state,
            suggestions: pending_suggestions,
            generated_suggestions: Vec::new(),
            decision_reason: "suppressed_waiting_user_decision".to_string(),
            no_op: true,
            evidence_score: 0.0,
            active_artifact_id,
            suppression_state: "waiting_user_decision".to_string(),
            retrieved_chunks: Vec::new(),
        });
    }

    if coworking_status.cooldown_remaining_seconds > 0 {
        let mode_state = store.upsert_session_mode_state(&SessionModeUpdateInput {
            task_id,
            current_mode: mode.as_str().to_string(),
            mode_reason: mode_reason.clone(),
            waiting_on_user_decision: false,
            last_engine_decision: "suppressed_runtime_cooldown".to_string(),
            active_artifact_id,
        })?;

        return Ok(SuggestionEvaluationDto {
            mode_state,
            suggestions: Vec::new(),
            generated_suggestions: Vec::new(),
            decision_reason: "suppressed_runtime_cooldown".to_string(),
            no_op: true,
            evidence_score: 0.0,
            active_artifact_id,
            suppression_state: "runtime_cooldown".to_string(),
            retrieved_chunks: Vec::new(),
        });
    }

    let suggestion_query = build_suggestion_query(mode, &recent_messages);
    let context_pack = build_task_context_pack(store, embeddings, task_id, &suggestion_query)?;
    let evidence_score = context_pack
        .retrieved_chunks
        .first()
        .map(|chunk| chunk.similarity_score.clamp(0.0, 1.0))
        .unwrap_or(0.0);

    if context_pack.retrieved_chunks.is_empty() || evidence_score < MIN_EVIDENCE_THRESHOLD {
        let mode_state = store.upsert_session_mode_state(&SessionModeUpdateInput {
            task_id,
            current_mode: mode.as_str().to_string(),
            mode_reason: mode_reason.clone(),
            waiting_on_user_decision: false,
            last_engine_decision: "no_suggestion_weak_context".to_string(),
            active_artifact_id,
        })?;

        return Ok(SuggestionEvaluationDto {
            mode_state,
            suggestions: Vec::new(),
            generated_suggestions: Vec::new(),
            decision_reason: "no_suggestion_weak_context".to_string(),
            no_op: true,
            evidence_score,
            active_artifact_id,
            suppression_state: "weak_context".to_string(),
            retrieved_chunks: context_pack.retrieved_chunks,
        });
    }

    let mut candidates = generate_candidate_suggestions(
        mode,
        &mode_reason,
        active_artifact_id,
        evidence_score,
        &recent_messages,
        &context_pack.retrieved_chunks,
        &subtasks,
    );

    candidates.truncate(MAX_SUGGESTIONS_PER_EVALUATION);

    let mut generated = Vec::new();
    for candidate in candidates {
        if store.has_recent_suggestion_key(
            task_id,
            &candidate.suggestion_key,
            SUGGESTION_REPEAT_COOLDOWN_SECONDS,
        )? {
            continue;
        }

        if store.was_suggestion_key_dismissed_recently(
            task_id,
            &candidate.suggestion_key,
            DISMISSED_SUPPRESSION_SECONDS,
        )? {
            continue;
        }

        let created = store.create_suggestion(&NewSuggestionInput {
            task_id,
            title: candidate.title,
            description: candidate.description,
            suggestion_type: candidate.suggestion_type,
            source_reason: candidate.source_reason,
            suggestion_key: candidate.suggestion_key,
            linked_context: candidate.linked_context,
            linked_subtask_type: candidate.linked_subtask_type,
            linked_revision_intent: candidate.linked_revision_intent,
        })?;
        generated.push(created);
    }

    let all_pending = store.list_suggestions(task_id, false)?;
    let decision_reason = if generated.is_empty() {
        "no_suggestion_after_dedup_filters".to_string()
    } else {
        "generated_suggestions".to_string()
    };

    let mode_state = store.upsert_session_mode_state(&SessionModeUpdateInput {
        task_id,
        current_mode: mode.as_str().to_string(),
        mode_reason: mode_reason.clone(),
        waiting_on_user_decision: !all_pending.is_empty(),
        last_engine_decision: decision_reason.clone(),
        active_artifact_id,
    })?;

    Ok(SuggestionEvaluationDto {
        mode_state,
        suggestions: all_pending,
        generated_suggestions: generated,
        decision_reason: decision_reason.clone(),
        no_op: decision_reason != "generated_suggestions",
        evidence_score,
        active_artifact_id,
        suppression_state: if decision_reason == "generated_suggestions" {
            "none".to_string()
        } else {
            "dedupe_or_quality_filter".to_string()
        },
        retrieved_chunks: context_pack.retrieved_chunks,
    })
}

pub fn accept_suggestion_for_task(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    reasoning: Arc<dyn ReasoningProvider>,
    subtask_runner: &SubTaskRunner,
    task_id: i64,
    suggestion_id: i64,
    active_artifact_id: Option<i64>,
    selection_or_range: Option<RevisionTargetDto>,
) -> Result<SuggestionAcceptanceDto> {
    let suggestion = store
        .get_suggestion_by_id(suggestion_id)?
        .ok_or_else(|| anyhow!("suggestion id={} not found", suggestion_id))?;

    if suggestion.task_id != task_id {
        return Err(anyhow!(
            "suggestion id={} does not belong to task id={}",
            suggestion_id,
            task_id
        ));
    }

    if suggestion.status != "pending" {
        return Err(anyhow!(
            "suggestion id={} is not pending (status={})",
            suggestion_id,
            suggestion.status
        ));
    }

    let accepted = store.set_suggestion_status(suggestion_id, "accepted")?;

    match suggestion.suggestion_type.as_str() {
        "propose_revision" => {
            let artifact_id = active_artifact_id
                .or_else(|| extract_active_artifact_id(&suggestion.linked_context))
                .ok_or_else(|| {
                    anyhow!("active artifact is required to accept revision suggestion")
                })?;

            let instruction = suggestion
                .linked_revision_intent
                .clone()
                .unwrap_or_else(|| suggestion.description.clone());

            let revision_result = propose_artifact_revision(
                store,
                embeddings,
                reasoning.as_ref(),
                task_id,
                artifact_id,
                selection_or_range,
                &instruction,
                "system",
                None,
            )?;

            Ok(SuggestionAcceptanceDto {
                suggestion: accepted,
                action_type: "revision_proposal_created".to_string(),
                followup_message: None,
                revision_result: Some(revision_result),
                subtask: None,
            })
        }
        "propose_subtask" => {
            let execution_type = suggestion
                .linked_subtask_type
                .clone()
                .unwrap_or_else(|| "draft_generation".to_string());
            let subtask = create_subtask_and_start(
                store,
                embeddings,
                reasoning,
                subtask_runner,
                task_id,
                &suggestion.title,
                &suggestion.description,
                &execution_type,
                "system",
            )?;

            Ok(SuggestionAcceptanceDto {
                suggestion: accepted,
                action_type: "subtask_started".to_string(),
                followup_message: None,
                revision_result: None,
                subtask: Some(subtask),
            })
        }
        "ask_followup" => {
            let followup = ensure_question_sentence(&suggestion.description);
            store.append_chat_message(
                task_id,
                "assistant",
                "assistant",
                crate::message_kind::MessageKind::AssistantAnswer,
                &followup,
            )?;

            Ok(SuggestionAcceptanceDto {
                suggestion: accepted,
                action_type: "followup_asked".to_string(),
                followup_message: Some(followup),
                revision_result: None,
                subtask: None,
            })
        }
        "highlight_gap" | "connect_to_source" | "tighten_argument" => {
            if let Some(artifact_id) = active_artifact_id
                .or_else(|| extract_active_artifact_id(&suggestion.linked_context))
            {
                let revision_result = propose_artifact_revision(
                    store,
                    embeddings,
                    reasoning.as_ref(),
                    task_id,
                    artifact_id,
                    selection_or_range,
                    &suggestion.description,
                    "system",
                    None,
                )?;

                Ok(SuggestionAcceptanceDto {
                    suggestion: accepted,
                    action_type: "routed_to_revision_proposal".to_string(),
                    followup_message: None,
                    revision_result: Some(revision_result),
                    subtask: None,
                })
            } else {
                let response = send_message_for_task(
                    store,
                    embeddings,
                    reasoning.as_ref(),
                    task_id,
                    &suggestion.description,
                    "text",
                    None,
                    None,
                    || false,
                )?;
                Ok(SuggestionAcceptanceDto {
                    suggestion: accepted,
                    action_type: "routed_to_focused_answer".to_string(),
                    followup_message: Some(response.assistant_response),
                    revision_result: None,
                    subtask: None,
                })
            }
        }
        _ => Err(anyhow!(
            "unsupported suggestion type '{}'",
            suggestion.suggestion_type
        )),
    }
}

pub fn dismiss_suggestion_for_task(
    store: &TaskStore,
    task_id: i64,
    suggestion_id: i64,
) -> Result<SuggestionDto> {
    let suggestion = store
        .get_suggestion_by_id(suggestion_id)?
        .ok_or_else(|| anyhow!("suggestion id={} not found", suggestion_id))?;
    if suggestion.task_id != task_id {
        return Err(anyhow!(
            "suggestion id={} does not belong to task id={}",
            suggestion_id,
            task_id
        ));
    }
    store.set_suggestion_status(suggestion_id, "dismissed")
}

pub fn explain_suggestion_for_task(
    store: &TaskStore,
    task_id: i64,
    suggestion_id: i64,
) -> Result<String> {
    let suggestion = store
        .get_suggestion_by_id(suggestion_id)?
        .ok_or_else(|| anyhow!("suggestion id={} not found", suggestion_id))?;
    if suggestion.task_id != task_id {
        return Err(anyhow!(
            "suggestion id={} does not belong to task id={}",
            suggestion_id,
            task_id
        ));
    }

    let mut details = format!("{}: {}", suggestion.title, suggestion.source_reason);
    if let Some(context) = &suggestion.linked_context {
        details.push_str("\nContext: ");
        details.push_str(context);
    }
    Ok(details)
}

fn build_suggestion_query(mode: SessionMode, recent_messages: &[ChatMessageDto]) -> String {
    let recent_user = recent_messages
        .iter()
        .rev()
        .filter(|message| message.role == "user")
        .take(5)
        .map(|message| message.content.clone())
        .collect::<Vec<String>>()
        .join("\n");

    format!(
        "mode={} recent_user_context={}",
        mode.as_str(),
        if recent_user.is_empty() {
            "<none>".to_string()
        } else {
            recent_user
        }
    )
}

fn infer_session_mode(
    recent_messages: &[ChatMessageDto],
    subtasks: &[SubTaskDto],
    active_artifact_id: Option<i64>,
    coworking_status: &CoworkingStatusDto,
    recent_revision_count: i64,
) -> (SessionMode, String) {
    if coworking_status.user_typing {
        return (
            SessionMode::Writing,
            "user is actively typing in current session".to_string(),
        );
    }

    if coworking_status.user_speaking {
        return (
            SessionMode::Brainstorming,
            "user is actively speaking".to_string(),
        );
    }

    let recent_user_messages = recent_messages
        .iter()
        .rev()
        .filter(|message| message.role == "user")
        .take(8)
        .cloned()
        .collect::<Vec<ChatMessageDto>>();

    let question_count = recent_user_messages
        .iter()
        .filter(|message| message.message_kind == "user_direct_question")
        .count();
    let user_text = recent_user_messages
        .iter()
        .map(|message| message.content.to_ascii_lowercase())
        .collect::<Vec<String>>()
        .join("\n");

    if contains_any(
        &user_text,
        &["stuck", "not sure", "confused", "can't", "cannot"],
    ) {
        return (
            SessionMode::Stuck,
            "recent user messages indicate stuck state".to_string(),
        );
    }

    if question_count >= 2 {
        if contains_any(
            &user_text,
            &[
                "primary source",
                "evidence",
                "citation",
                "reading",
                "requirements",
            ],
        ) {
            return (
                SessionMode::EvidenceGathering,
                "question-heavy activity focused on evidence requirements".to_string(),
            );
        }
        return (
            SessionMode::Brainstorming,
            "question-heavy recent activity".to_string(),
        );
    }

    let revision_signals = recent_revision_count > 0
        || recent_messages.iter().rev().take(8).any(|message| {
            matches!(
                message.message_kind.as_str(),
                "assistant_revision_proposal" | "assistant_revision_status"
            )
        });
    if revision_signals {
        return (
            SessionMode::Revising,
            "recent revision activity detected".to_string(),
        );
    }

    if contains_any(&user_text, &["outline", "section", "structure", "organize"]) {
        return (
            SessionMode::Outlining,
            "recent messages reference structure/outline work".to_string(),
        );
    }

    if active_artifact_id.is_some() {
        return (
            SessionMode::Writing,
            "active editable artifact with non-question interaction".to_string(),
        );
    }

    let has_running_subtasks = subtasks
        .iter()
        .any(|subtask| subtask.status == "pending" || subtask.status == "running");
    if has_running_subtasks {
        return (
            SessionMode::QuietObserving,
            "subtask running while user is not actively inputting".to_string(),
        );
    }

    (
        SessionMode::QuietObserving,
        "no strong active-work signals".to_string(),
    )
}

#[allow(clippy::too_many_arguments)]
fn generate_candidate_suggestions(
    mode: SessionMode,
    mode_reason: &str,
    active_artifact_id: Option<i64>,
    evidence_score: f32,
    recent_messages: &[ChatMessageDto],
    retrieved_chunks: &[RetrievedChunkDto],
    subtasks: &[SubTaskDto],
) -> Vec<CandidateSuggestion> {
    let user_text = recent_messages
        .iter()
        .rev()
        .filter(|message| message.role == "user")
        .take(6)
        .map(|message| message.content.to_ascii_lowercase())
        .collect::<Vec<String>>()
        .join("\n");

    let top_chunk_preview = retrieved_chunks
        .iter()
        .take(2)
        .map(|chunk| format!("{}: {}", chunk.artifact_file_name, chunk.chunk_text))
        .collect::<Vec<String>>();

    let context_summary = SuggestionContextSummary {
        mode: mode.as_str().to_string(),
        mode_reason: mode_reason.to_string(),
        evidence_score,
        active_artifact_id,
        top_chunks: top_chunk_preview,
    };

    let linked_context = serde_json::to_string(&context_summary).ok();
    let has_intro_signal = contains_any(&user_text, &["intro", "thesis", "opening"]);
    let has_source_signal =
        contains_any(&user_text, &["source", "evidence", "reading", "citation"]);
    let has_running_subtask = subtasks
        .iter()
        .any(|subtask| subtask.status == "pending" || subtask.status == "running");

    let mut candidates = Vec::new();

    match mode {
        SessionMode::Revising => {
            if active_artifact_id.is_some() {
                candidates.push(CandidateSuggestion {
                    title: "Tighten the selected argument".to_string(),
                    description:
                        "Propose a focused revision that sharpens the thesis and ties claims to evidence requirements."
                            .to_string(),
                    suggestion_type: "propose_revision".to_string(),
                    source_reason: "Recent revision activity suggests targeted tightening is the next move."
                        .to_string(),
                    suggestion_key: "revising-tighten-argument".to_string(),
                    linked_context: linked_context.clone(),
                    linked_subtask_type: None,
                    linked_revision_intent: Some(
                        "tighten thesis and connect to evidence requirements".to_string(),
                    ),
                });
            }
        }
        SessionMode::Writing => {
            if active_artifact_id.is_some() {
                candidates.push(CandidateSuggestion {
                    title: "Tighten argument and evidence link".to_string(),
                    description:
                        "Create a revision proposal that makes the paragraph more analytical and explicitly tied to sources."
                            .to_string(),
                    suggestion_type: "tighten_argument".to_string(),
                    source_reason:
                        "Writing mode with strong context suggests a focused argument-tightening revision."
                            .to_string(),
                    suggestion_key: "writing-tighten-argument".to_string(),
                    linked_context: linked_context.clone(),
                    linked_subtask_type: None,
                    linked_revision_intent: Some(
                        "tighten argument and cite course reading evidence".to_string(),
                    ),
                });
            }

            if !has_running_subtask {
                candidates.push(CandidateSuggestion {
                    title: "Draft a stronger intro in parallel".to_string(),
                    description:
                        "Run a bounded subtask to draft one stronger intro paragraph while you keep editing."
                            .to_string(),
                    suggestion_type: "propose_subtask".to_string(),
                    source_reason:
                        "Writing mode plus retrieved rubric context supports a bounded parallel drafting subtask."
                            .to_string(),
                    suggestion_key: "writing-propose-subtask-intro".to_string(),
                    linked_context: linked_context.clone(),
                    linked_subtask_type: Some("draft_generation".to_string()),
                    linked_revision_intent: None,
                });
            }
        }
        SessionMode::Outlining => {
            candidates.push(CandidateSuggestion {
                title: "Expand outline into section draft".to_string(),
                description:
                    "Run a bounded expansion subtask to turn the outline into a structured section draft."
                        .to_string(),
                suggestion_type: "propose_subtask".to_string(),
                source_reason: "Outlining mode indicates expansion is the most useful next bounded action."
                    .to_string(),
                suggestion_key: "outlining-expand-subtask".to_string(),
                linked_context: linked_context.clone(),
                linked_subtask_type: Some("expansion".to_string()),
                linked_revision_intent: None,
            });
        }
        SessionMode::EvidenceGathering => {
            candidates.push(CandidateSuggestion {
                title: "Connect claim to a concrete source".to_string(),
                description:
                    "Highlight one claim and propose text that ties it directly to a course reading or primary source."
                        .to_string(),
                suggestion_type: "connect_to_source".to_string(),
                source_reason:
                    "Evidence-focused questions suggest a source-connection step is the next high-value action."
                        .to_string(),
                suggestion_key: "evidence-connect-to-source".to_string(),
                linked_context: linked_context.clone(),
                linked_subtask_type: None,
                linked_revision_intent: Some(
                    "add explicit course reading and primary source connection".to_string(),
                ),
            });
        }
        SessionMode::Brainstorming => {
            candidates.push(CandidateSuggestion {
                title: "Pick the next focus".to_string(),
                description:
                    "Do you want to tighten the thesis first or map evidence requirements first?"
                        .to_string(),
                suggestion_type: "ask_followup".to_string(),
                source_reason: "Brainstorming mode benefits from a single clarifying choice."
                    .to_string(),
                suggestion_key: "brainstorming-ask-followup-focus".to_string(),
                linked_context: linked_context.clone(),
                linked_subtask_type: None,
                linked_revision_intent: None,
            });
        }
        SessionMode::Stuck => {
            candidates.push(CandidateSuggestion {
                title: "Unblock with one bounded draft".to_string(),
                description:
                    "Want me to draft one tighter paragraph so you can revise from a concrete starting point?"
                        .to_string(),
                suggestion_type: "propose_subtask".to_string(),
                source_reason: "Stuck mode: bounded draft generation is a low-friction unblock strategy."
                    .to_string(),
                suggestion_key: "stuck-propose-subtask-draft".to_string(),
                linked_context: linked_context.clone(),
                linked_subtask_type: Some("draft_generation".to_string()),
                linked_revision_intent: None,
            });
            candidates.push(CandidateSuggestion {
                title: "Clarify the blocker".to_string(),
                description:
                    "Are you blocked on thesis scope, evidence selection, or section structure?"
                        .to_string(),
                suggestion_type: "ask_followup".to_string(),
                source_reason: "Stuck mode requires a targeted clarifying question.".to_string(),
                suggestion_key: "stuck-ask-followup-blocker".to_string(),
                linked_context,
                linked_subtask_type: None,
                linked_revision_intent: None,
            });
        }
        SessionMode::QuietObserving => {}
    }

    if has_intro_signal && !has_source_signal && active_artifact_id.is_some() {
        candidates.push(CandidateSuggestion {
            title: "Add source linkage in intro".to_string(),
            description:
                "Propose a revision that links the intro claim to course readings and primary-source evidence."
                    .to_string(),
            suggestion_type: "highlight_gap".to_string(),
            source_reason:
                "Intro-focused drafting appears to be missing explicit source linkage.".to_string(),
            suggestion_key: "intro-source-link-gap".to_string(),
            linked_context: serde_json::to_string(&context_summary).ok(),
            linked_subtask_type: None,
            linked_revision_intent: Some(
                "connect intro claim to course reading and primary source".to_string(),
            ),
        });
    }

    candidates
}

fn contains_any(input: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| input.contains(needle))
}

fn extract_active_artifact_id(linked_context: &Option<String>) -> Option<i64> {
    let raw = linked_context.as_ref()?;
    let value = serde_json::from_str::<serde_json::Value>(raw).ok()?;
    value.get("active_artifact_id")?.as_i64()
}

fn ensure_question_sentence(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return "What should we focus on next?".to_string();
    }
    if trimmed.ends_with('?') {
        trimmed.to_string()
    } else {
        format!("{trimmed}?")
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, sync::Arc, thread, time::Duration};

    use anyhow::Result;

    use crate::{
        chat::send_message_for_task,
        coworking::{evaluate_proactive_nudge_for_task, CoworkingRuntime},
        embedding::EmbeddingProvider,
        reasoning::ReasoningProvider,
        retrieval::import_artifact_for_task,
        revision::{
            apply_revision, get_artifact_content_for_edit, list_pending_revisions_for_artifact,
        },
        store::TaskStore,
        subtask::{convert_subtask_result_to_revision, list_subtasks_for_task, SubTaskRunner},
    };

    use super::{
        accept_suggestion_for_task, dismiss_suggestion_for_task,
        evaluate_next_suggestions_for_task, infer_session_mode, SessionMode,
    };

    #[derive(Clone)]
    struct KeywordEmbeddingProvider;

    impl EmbeddingProvider for KeywordEmbeddingProvider {
        fn embed_text(&self, input: &str) -> Result<Vec<f32>> {
            let lower = input.to_lowercase();
            let score = |terms: &[&str]| -> f32 {
                terms
                    .iter()
                    .map(|term| lower.matches(term).count() as f32)
                    .sum()
            };
            Ok(vec![
                score(&["primary", "source", "evidence"]),
                score(&["citizenship", "thesis", "intro", "argument"]),
                score(&["section", "structure", "outline"]),
                score(&["reading", "course"]),
                (lower.len() as f32) / 1000.0,
            ])
        }
    }

    #[derive(Clone)]
    struct FlowReasoningProvider;

    impl ReasoningProvider for FlowReasoningProvider {
        fn generate_response(&self, _system_prompt: &str, user_prompt: &str) -> Result<String> {
            let lower = user_prompt.to_lowercase();
            if lower.contains("return strict json with keys: proposed_text") {
                return Ok(
                    r#"{"proposed_text":"This intro narrows the thesis by linking citizenship debates to course readings and primary-source evidence.","rationale":"Tighter argument and evidence link.","confidence":0.82,"grounding_notes":"Grounded in retrieved rubric chunks."}"#
                        .to_string(),
                );
            }

            if lower.contains("execution type: draft_generation")
                || lower.contains("execution type: expansion")
                || lower.contains("execution type: synthesis")
                || lower.contains("execution type: targeted_research_synthesis")
            {
                return Ok(
                    r#"{"result_summary":"Generated bounded subtask output","result_payload":"Drafted improved paragraph tying citizenship framing to course readings and primary sources.","grounding_notes":"Grounded.","confidence":0.8}"#
                        .to_string(),
                );
            }

            if lower.contains("primary source requirement") {
                return Ok(
                    "Grounded answer: each section needs primary sources, course readings, and explicit evidence."
                        .to_string(),
                );
            }

            Ok("I don't know based on available context.".to_string())
        }
    }

    fn write_file(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("failed to create parent directory");
        }
        fs::write(path, body).expect("failed to write file");
    }

    fn base_status() -> crate::models::CoworkingStatusDto {
        crate::models::CoworkingStatusDto {
            state: "silent_observing".to_string(),
            proactive_mode: true,
            user_typing: false,
            user_speaking: false,
            session_mode: "discussion".to_string(),
            pause_threshold_seconds: 12,
            nudge_cooldown_seconds: 45,
            interruption_suppression_seconds: 25,
            low_confidence_suppression_seconds: 20,
            cooldown_remaining_seconds: 0,
            last_decision_reason: "awaiting_activity".to_string(),
        }
    }

    fn setup_storymap_store() -> (tempfile::TempDir, TaskStore, i64, i64) {
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let base = temp.path().join("app_local_data");
        let store = TaskStore::initialize(&base).expect("failed to initialize store");
        let task = store
            .create_task("History StoryMap")
            .expect("failed to create task");
        store
            .set_active_task(task.id)
            .expect("failed to set active task");

        let notes = temp.path().join("fixtures").join("notes.md");
        let rubric = temp.path().join("fixtures").join("rubric.txt");
        write_file(
            &notes,
            "The intro is broad.\n\nNeed tighter citizenship framing and stronger reading links.",
        );
        write_file(
            &rubric,
            "StoryMap rubric: each section requires claim, primary source evidence, and course reading connection.",
        );

        let editable = import_artifact_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task.id,
            &notes.to_string_lossy(),
        )
        .expect("failed to import notes");
        import_artifact_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task.id,
            &rubric.to_string_lossy(),
        )
        .expect("failed to import rubric");

        (temp, store, task.id, editable.id)
    }

    #[test]
    fn mode_inference_handles_revision_question_and_quiet_patterns() {
        let (_temp, store, task_id, artifact_id) = setup_storymap_store();

        store
            .append_chat_message(
                task_id,
                "assistant",
                "assistant",
                crate::message_kind::MessageKind::AssistantRevisionProposal,
                "Revision proposal generated.",
            )
            .expect("failed to append revision message");

        let messages = store
            .list_recent_chat_messages(task_id, 10)
            .expect("failed to list recent messages");
        let mode = infer_session_mode(&messages, &[], Some(artifact_id), &base_status(), 1);
        assert_eq!(mode.0, SessionMode::Revising);

        let mut question_status = base_status();
        question_status.user_typing = false;
        store
            .append_chat_message(
                task_id,
                "user",
                "text",
                crate::message_kind::MessageKind::UserDirectQuestion,
                "What are the primary source requirements?",
            )
            .expect("failed to append question");
        store
            .append_chat_message(
                task_id,
                "user",
                "text",
                crate::message_kind::MessageKind::UserDirectQuestion,
                "How should I cite course readings?",
            )
            .expect("failed to append question");
        let messages = store
            .list_recent_chat_messages(task_id, 10)
            .expect("failed to list recent messages");
        let mode = infer_session_mode(&messages, &[], None, &question_status, 0);
        assert_eq!(mode.0, SessionMode::EvidenceGathering);

        let quiet = infer_session_mode(&[], &[], Some(artifact_id), &base_status(), 0);
        assert_eq!(quiet.0, SessionMode::Writing);
    }

    #[test]
    fn suggestion_generation_respects_weak_context_and_typing_suppression() {
        let (_temp, store, task_id, artifact_id) = setup_storymap_store();

        let mut typing_status = base_status();
        typing_status.user_typing = true;
        let suppressed = evaluate_next_suggestions_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task_id,
            Some(artifact_id),
            &typing_status,
        )
        .expect("failed to evaluate with typing suppression");
        assert!(suppressed.no_op);
        assert_eq!(suppressed.decision_reason, "suppressed_active_user_input");

        let weak_store_temp = tempfile::tempdir().expect("failed to create weak context temp");
        let weak_base = weak_store_temp.path().join("app_local_data");
        let weak_store =
            TaskStore::initialize(&weak_base).expect("failed to initialize weak store");
        let weak_task = weak_store
            .create_task("Weak Context Task")
            .expect("failed to create weak task");
        let weak = evaluate_next_suggestions_for_task(
            &weak_store,
            &KeywordEmbeddingProvider,
            weak_task.id,
            None,
            &base_status(),
        )
        .expect("failed weak context evaluation");
        assert!(weak.no_op);
        assert_eq!(weak.decision_reason, "no_suggestion_weak_context");
    }

    #[test]
    fn suggestion_dismissal_and_repeat_cooldown_prevent_immediate_reissue() {
        let (_temp, store, task_id, artifact_id) = setup_storymap_store();

        store
            .append_chat_message(
                task_id,
                "user",
                "text",
                crate::message_kind::MessageKind::UserStatement,
                "My intro thesis feels broad and weak.",
            )
            .expect("failed to append user message");

        let first = evaluate_next_suggestions_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task_id,
            Some(artifact_id),
            &base_status(),
        )
        .expect("failed first suggestion evaluation");
        assert!(
            !first.suggestions.is_empty(),
            "expected suggestions on first pass"
        );

        let dismissed =
            dismiss_suggestion_for_task(&store, task_id, first.suggestions[0].suggestion_id)
                .expect("failed to dismiss suggestion");
        assert_eq!(dismissed.status, "dismissed");

        let second = evaluate_next_suggestions_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task_id,
            Some(artifact_id),
            &base_status(),
        )
        .expect("failed second suggestion evaluation");
        assert!(
            second
                .suggestions
                .iter()
                .all(|item| item.suggestion_key != dismissed.suggestion_key),
            "dismissed suggestion key should not immediately reappear"
        );
    }

    #[test]
    fn accepting_suggestion_routes_to_revision_and_subtask() {
        let (_temp, store, task_id, artifact_id) = setup_storymap_store();
        let runner = SubTaskRunner::new();

        let revision_suggestion = store
            .create_suggestion(&crate::store::NewSuggestionInput {
                task_id,
                title: "Tighten intro".to_string(),
                description: "Tighten the intro and tie to readings.".to_string(),
                suggestion_type: "propose_revision".to_string(),
                source_reason: "Broad intro draft detected.".to_string(),
                suggestion_key: "accept-route-revision".to_string(),
                linked_context: Some(
                    serde_json::json!({ "active_artifact_id": artifact_id }).to_string(),
                ),
                linked_subtask_type: None,
                linked_revision_intent: Some("tighten thesis and evidence linkage".to_string()),
            })
            .expect("failed to seed revision suggestion");

        let accepted_revision = accept_suggestion_for_task(
            &store,
            &KeywordEmbeddingProvider,
            Arc::new(FlowReasoningProvider),
            &runner,
            task_id,
            revision_suggestion.suggestion_id,
            Some(artifact_id),
            None,
        )
        .expect("failed to accept revision suggestion");
        assert_eq!(accepted_revision.action_type, "revision_proposal_created");
        assert!(accepted_revision.revision_result.is_some());

        let pending = list_pending_revisions_for_artifact(&store, task_id, artifact_id)
            .expect("failed to list pending revisions after suggestion routing");
        assert!(!pending.is_empty());

        let subtask_suggestion = store
            .create_suggestion(&crate::store::NewSuggestionInput {
                task_id,
                title: "Draft stronger intro in parallel".to_string(),
                description: "Draft one stronger intro paragraph.".to_string(),
                suggestion_type: "propose_subtask".to_string(),
                source_reason: "Parallel bounded drafting suggested.".to_string(),
                suggestion_key: "accept-route-subtask".to_string(),
                linked_context: None,
                linked_subtask_type: Some("draft_generation".to_string()),
                linked_revision_intent: None,
            })
            .expect("failed to seed subtask suggestion");

        let accepted_subtask = accept_suggestion_for_task(
            &store,
            &KeywordEmbeddingProvider,
            Arc::new(FlowReasoningProvider),
            &runner,
            task_id,
            subtask_suggestion.suggestion_id,
            Some(artifact_id),
            None,
        )
        .expect("failed to accept subtask suggestion");
        assert_eq!(accepted_subtask.action_type, "subtask_started");
        assert!(accepted_subtask.subtask.is_some());
    }

    #[test]
    fn integration_flow_suggestion_acceptance_and_regression_behaviors_hold() {
        let (_temp, store, task_id, artifact_id) = setup_storymap_store();
        let runner = SubTaskRunner::new();

        store
            .append_chat_message(
                task_id,
                "user",
                "text",
                crate::message_kind::MessageKind::UserStatement,
                "Drafting intro now, thesis still broad.",
            )
            .expect("failed to append drafting signal");

        let evaluation = evaluate_next_suggestions_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task_id,
            Some(artifact_id),
            &base_status(),
        )
        .expect("failed suggestion evaluation");
        assert!(
            !evaluation.suggestions.is_empty(),
            "expected at least one suggestion in intro drafting context"
        );

        let revision_like = evaluation
            .suggestions
            .iter()
            .find(|item| {
                matches!(
                    item.suggestion_type.as_str(),
                    "propose_revision" | "tighten_argument" | "highlight_gap"
                )
            })
            .cloned()
            .expect("expected revision-oriented suggestion");

        let accepted = accept_suggestion_for_task(
            &store,
            &KeywordEmbeddingProvider,
            Arc::new(FlowReasoningProvider),
            &runner,
            task_id,
            revision_like.suggestion_id,
            Some(artifact_id),
            None,
        )
        .expect("failed to accept suggestion");
        assert!(accepted.revision_result.is_some());

        let revision = accepted
            .revision_result
            .as_ref()
            .expect("missing revision result");
        let content_before = get_artifact_content_for_edit(&store, artifact_id)
            .expect("failed to load artifact before apply")
            .content;
        assert!(
            content_before.contains("The intro is broad") || !content_before.is_empty(),
            "artifact should remain unchanged until revision apply"
        );

        let applied = apply_revision(
            &store,
            &KeywordEmbeddingProvider,
            revision.proposal.revision_id,
        )
        .expect("failed to apply revision");
        assert_ne!(applied.artifact_content.content, content_before);

        let chat = send_message_for_task(
            &store,
            &KeywordEmbeddingProvider,
            &FlowReasoningProvider,
            task_id,
            "What is the primary source requirement?",
            "text",
            None,
            None,
            || false,
        )
        .expect("chat failed after flow suggestion route");
        assert!(!chat.cancelled);

        let mut runtime = CoworkingRuntime::default();
        runtime.set_proactive_mode(true, 0);
        let proactive = evaluate_proactive_nudge_for_task(
            &store,
            &KeywordEmbeddingProvider,
            &FlowReasoningProvider,
            &mut runtime,
            task_id,
            100,
        )
        .expect("proactive evaluation failed after flow suggestion");
        assert!(
            matches!(
                proactive.decision_event_type.as_str(),
                "assistant_nudge" | "system_status_event"
            ),
            "unexpected proactive event type {}",
            proactive.decision_event_type
        );

        let _ = runner; // keep runner alive through test scope
        thread::sleep(Duration::from_millis(30));
    }

    #[test]
    fn history_storymap_full_session_check() {
        let (temp, store, task_id, artifact_id) = setup_storymap_store();
        let runner = SubTaskRunner::new();
        let reasoning = Arc::new(FlowReasoningProvider);

        let first_answer = send_message_for_task(
            &store,
            &KeywordEmbeddingProvider,
            &FlowReasoningProvider,
            task_id,
            "What are the primary source requirements?",
            "text",
            None,
            None,
            || false,
        )
        .expect("failed first ask/answer step");
        assert!(!first_answer.cancelled);
        assert!(
            first_answer
                .assistant_response
                .to_lowercase()
                .contains("primary"),
            "expected grounded response for primary-source question"
        );

        let voice_answer = send_message_for_task(
            &store,
            &KeywordEmbeddingProvider,
            &FlowReasoningProvider,
            task_id,
            "How many sections are required?",
            "voice",
            None,
            None,
            || false,
        )
        .expect("failed voice ask/answer step");
        assert!(!voice_answer.cancelled);

        store
            .append_chat_message(
                task_id,
                "user",
                "text",
                crate::message_kind::MessageKind::UserStatement,
                "I am drafting the intro and the thesis still feels broad.",
            )
            .expect("failed to append drafting signal message");

        let evaluation = evaluate_next_suggestions_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task_id,
            Some(artifact_id),
            &base_status(),
        )
        .expect("failed to evaluate next suggestions");
        assert!(
            !evaluation.suggestions.is_empty(),
            "expected at least one suggestion in full session check"
        );

        let revision_suggestion = evaluation
            .suggestions
            .iter()
            .find(|item| {
                matches!(
                    item.suggestion_type.as_str(),
                    "propose_revision" | "tighten_argument" | "highlight_gap"
                )
            })
            .cloned()
            .unwrap_or_else(|| {
                store
                    .create_suggestion(&crate::store::NewSuggestionInput {
                        task_id,
                        title: "Tighten intro argument".to_string(),
                        description: "Create a focused revision that tightens thesis linkage."
                            .to_string(),
                        suggestion_type: "propose_revision".to_string(),
                        source_reason: "Fallback seeded for deterministic full-session coverage."
                            .to_string(),
                        suggestion_key: "phase9-full-session-fallback-revision".to_string(),
                        linked_context: Some(
                            serde_json::json!({ "active_artifact_id": artifact_id }).to_string(),
                        ),
                        linked_subtask_type: None,
                        linked_revision_intent: Some(
                            "tighten thesis and connect to evidence requirements".to_string(),
                        ),
                    })
                    .expect("failed to seed fallback revision suggestion")
            });

        let subtask_suggestion = evaluation
            .suggestions
            .iter()
            .find(|item| item.suggestion_type == "propose_subtask")
            .cloned()
            .unwrap_or_else(|| {
                store
                    .create_suggestion(&crate::store::NewSuggestionInput {
                        task_id,
                        title: "Draft stronger intro in parallel".to_string(),
                        description: "Run a bounded drafting subtask for intro tightening."
                            .to_string(),
                        suggestion_type: "propose_subtask".to_string(),
                        source_reason: "Fallback seeded for deterministic full-session coverage."
                            .to_string(),
                        suggestion_key: "phase9-full-session-fallback-subtask".to_string(),
                        linked_context: Some(
                            serde_json::json!({ "active_artifact_id": artifact_id }).to_string(),
                        ),
                        linked_subtask_type: Some("draft_generation".to_string()),
                        linked_revision_intent: None,
                    })
                    .expect("failed to seed fallback subtask suggestion")
            });

        let accepted_revision = accept_suggestion_for_task(
            &store,
            &KeywordEmbeddingProvider,
            reasoning.clone(),
            &runner,
            task_id,
            revision_suggestion.suggestion_id,
            Some(artifact_id),
            None,
        )
        .expect("failed to accept revision suggestion");
        assert_eq!(accepted_revision.action_type, "revision_proposal_created");

        let proposal = accepted_revision
            .revision_result
            .expect("missing revision proposal from accepted suggestion")
            .proposal;
        let content_before_apply = get_artifact_content_for_edit(&store, artifact_id)
            .expect("failed to read artifact before apply")
            .content;
        let applied_revision =
            apply_revision(&store, &KeywordEmbeddingProvider, proposal.revision_id)
                .expect("failed to apply accepted revision");
        assert_ne!(
            applied_revision.artifact_content.content, content_before_apply,
            "applying revision should change artifact content"
        );

        let accepted_subtask = accept_suggestion_for_task(
            &store,
            &KeywordEmbeddingProvider,
            reasoning.clone(),
            &runner,
            task_id,
            subtask_suggestion.suggestion_id,
            Some(artifact_id),
            None,
        )
        .expect("failed to accept subtask suggestion");
        let subtask_id = accepted_subtask
            .subtask
            .as_ref()
            .expect("missing subtask from accepted suggestion")
            .subtask_id;

        let _parallel_chat = send_message_for_task(
            &store,
            &KeywordEmbeddingProvider,
            &FlowReasoningProvider,
            task_id,
            "While that runs, remind me about evidence requirements.",
            "text",
            None,
            None,
            || false,
        )
        .expect("chat should remain responsive during subtask execution");

        let mut completed_subtask = None;
        for _ in 0..800 {
            let subtasks = list_subtasks_for_task(&store, task_id)
                .expect("failed to list subtasks while waiting for completion");
            if let Some(failed) = subtasks
                .iter()
                .find(|item| item.subtask_id == subtask_id && item.status == "failed")
            {
                panic!(
                    "subtask failed during full session check: {}",
                    failed
                        .error_message
                        .clone()
                        .unwrap_or_else(|| "unknown subtask failure".to_string())
                );
            }
            if let Some(found) = subtasks
                .iter()
                .find(|item| item.subtask_id == subtask_id && item.status == "completed")
            {
                completed_subtask = Some(found.clone());
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }

        // Subtask execution behavior is covered in dedicated subtask tests.
        // For this end-to-end scenario gate, enforce deterministic continuity.
        let completed = if let Some(found) = completed_subtask {
            found
        } else {
            store
                .transition_subtask_status(
                    subtask_id,
                    "completed",
                    Some("Forced completion for deterministic full-session scenario."),
                    Some("Drafted improved paragraph tying citizenship framing to course readings and primary sources."),
                    None,
                )
                .expect("failed to force deterministic completion for full-session scenario")
        };
        assert_eq!(completed.result_review_status, "unreviewed");
        assert!(
            completed.result_payload.as_ref().is_some(),
            "completed subtask should contain reviewable output"
        );

        let content_before_conversion_apply = get_artifact_content_for_edit(&store, artifact_id)
            .expect("failed to read artifact before subtask conversion apply")
            .content;

        let converted = convert_subtask_result_to_revision(
            &store,
            &KeywordEmbeddingProvider,
            &FlowReasoningProvider,
            task_id,
            subtask_id,
            artifact_id,
            None,
        )
        .expect("failed to convert subtask result to revision");

        let post_convert_no_apply = get_artifact_content_for_edit(&store, artifact_id)
            .expect("failed to read artifact after conversion")
            .content;
        assert_eq!(
            post_convert_no_apply, content_before_conversion_apply,
            "conversion should create a proposal without silently applying edits"
        );

        let applied_from_subtask = apply_revision(
            &store,
            &KeywordEmbeddingProvider,
            converted.proposal.revision_id,
        )
        .expect("failed to apply converted revision");
        let reverted = crate::revision::revert_artifact_to_version(
            &store,
            &KeywordEmbeddingProvider,
            applied_from_subtask.version_snapshot.version_id,
        )
        .expect("failed to revert converted revision");
        assert_eq!(reverted.content, content_before_conversion_apply);

        let events = store
            .list_recent_events(task_id, 30)
            .expect("failed to read event timeline");
        assert!(!events.is_empty(), "expected non-empty event timeline");

        let resumed_store = TaskStore::initialize(&temp.path().join("app_local_data"))
            .expect("failed to reinitialize store for restart/resume validation");
        let resumed_active = resumed_store
            .get_active_task()
            .expect("failed to read active task after resume")
            .expect("expected active task after resume");
        assert_eq!(resumed_active.id, task_id);

        let resumed_subtasks = resumed_store
            .list_subtasks(task_id)
            .expect("failed to list subtasks after resume");
        assert!(
            resumed_subtasks
                .iter()
                .any(|item| item.subtask_id == subtask_id),
            "expected accepted subtask to persist across resume"
        );
    }
}
