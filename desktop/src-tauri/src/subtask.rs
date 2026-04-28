use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    character::{self, SubtaskContext},
    embedding::EmbeddingProvider,
    message_kind::MessageKind,
    models::{
        FileWriteProposalDto, RetrievedChunkDto, RevisionProposalResultDto, RevisionTargetDto,
        SubTaskDto, SubTaskStepDto, SubTaskSuggestionDto,
    },
    reasoning::ReasoningProvider,
    retrieval::{build_task_context_pack, retrieve_relevant_chunks},
    revision::propose_artifact_revision,
    store::{NewSubTaskInput, TaskStore},
};

// e1: companion events emitted by background subtask chain threads via sync channel.
// main.rs wires a relay thread that forwards them to the frontend via AppHandle::emit.
pub enum CompanionEvent {
    Started {
        subtask_id: i64,
        task_id: i64,
        title: String,
    },
    Complete {
        subtask_id: i64,
        task_id: i64,
        final_status: String,
    },
    WriteProposal(FileWriteProposalDto),
}

// phase 16: multi-step chain constants and prompts

pub const MAX_SUBTASK_STEPS: usize = 5;
const MAX_CHAIN_STEP_DESCRIPTION_CHARS: usize = 600;
const MAX_CHAIN_CONTEXT_CHARS: usize = 10_000;
const MAX_CHAIN_STEP_OUTPUT_CHARS: usize = 8_000;
const MAX_CHAIN_RETRIEVAL_CHUNKS: usize = 8;
const MAX_CHAIN_FILE_PROPOSAL_CONTENT_CHARS: usize = 24_000;
const MAX_CHAIN_PATH_CHARS: usize = 240;

#[derive(Debug, Deserialize)]
struct ChainPlanStep {
    step_type: String,
    description: String,
    proposed_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChainPlan {
    steps: Vec<ChainPlanStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubTaskContextSnapshot {
    pub task_summary: String,
    pub instruction: String,
    pub execution_type: String,
    pub recent_messages: Vec<String>,
    pub retrieved_chunks: Vec<RetrievedChunkDto>,
}

#[derive(Debug)]
pub struct SubTaskRunner {
    cancellation_flags: Arc<Mutex<HashMap<i64, Arc<AtomicBool>>>>,
    companion_tx: Arc<Mutex<Option<mpsc::SyncSender<CompanionEvent>>>>,
}

impl Default for SubTaskRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl SubTaskRunner {
    pub fn new() -> Self {
        Self {
            cancellation_flags: Arc::new(Mutex::new(HashMap::new())),
            companion_tx: Arc::new(Mutex::new(None)),
        }
    }

    // e1: called once from main.rs setup to wire the companion event relay.
    pub fn set_companion_notify(&self, tx: mpsc::SyncSender<CompanionEvent>) {
        if let Ok(mut guard) = self.companion_tx.lock() {
            *guard = Some(tx);
        }
    }

    fn clone_companion_tx(&self) -> Option<mpsc::SyncSender<CompanionEvent>> {
        self.companion_tx.lock().ok().and_then(|g| g.clone())
    }

    pub fn start_subtask(
        &self,
        store: TaskStore,
        reasoning: Arc<dyn ReasoningProvider>,
        subtask_id: i64,
    ) -> Result<()> {
        let cancel_token = Arc::new(AtomicBool::new(false));

        {
            let mut flags = self
                .cancellation_flags
                .lock()
                .map_err(|_| anyhow!("failed to lock subtask cancellation map"))?;
            if flags.contains_key(&subtask_id) {
                return Err(anyhow!("subtask id={} is already running", subtask_id));
            }
            flags.insert(subtask_id, cancel_token.clone());
        }

        let flags_ref = Arc::clone(&self.cancellation_flags);
        thread::spawn(move || {
            let run_result =
                run_subtask_execution(&store, reasoning.as_ref(), subtask_id, &cancel_token);
            if let Err(error) = run_result {
                let _ = mark_subtask_failed(&store, subtask_id, &error.to_string());
            }

            if let Ok(mut flags) = flags_ref.lock() {
                flags.remove(&subtask_id);
            }
        });

        Ok(())
    }

    pub fn request_cancel(&self, subtask_id: i64) -> bool {
        if let Ok(flags) = self.cancellation_flags.lock() {
            if let Some(flag) = flags.get(&subtask_id) {
                flag.store(true, Ordering::SeqCst);
                return true;
            }
        }
        false
    }

    pub fn start_subtask_chain(
        &self,
        store: TaskStore,
        reasoning: Arc<dyn ReasoningProvider>,
        embeddings: Arc<dyn EmbeddingProvider>,
        subtask_id: i64,
    ) -> Result<()> {
        let cancel_token = Arc::new(AtomicBool::new(false));

        {
            let mut flags = self
                .cancellation_flags
                .lock()
                .map_err(|_| anyhow!("failed to lock subtask cancellation map for chain"))?;
            if flags.contains_key(&subtask_id) {
                return Err(anyhow!("subtask id={} is already running", subtask_id));
            }
            flags.insert(subtask_id, cancel_token.clone());
        }

        let flags_ref = Arc::clone(&self.cancellation_flags);
        // e1: clone sender before move; None if relay not yet wired (e.g. in tests).
        let companion_sender = self.clone_companion_tx();
        thread::spawn(move || {
            let run_result = run_subtask_chain(
                &store,
                reasoning.as_ref(),
                embeddings.as_ref(),
                subtask_id,
                &cancel_token,
                companion_sender.as_ref(),
            );
            if let Err(error) = run_result {
                let _ = mark_subtask_failed(&store, subtask_id, &error.to_string());
                // e1: send failed event when chain errors out unexpectedly.
                if let Some(tx) = &companion_sender {
                    if let Ok(Some(s)) = store.get_subtask_by_id(subtask_id) {
                        let _ = tx.try_send(CompanionEvent::Complete {
                            subtask_id,
                            task_id: s.task_id,
                            final_status: "failed".to_string(),
                        });
                    }
                }
            }
            if let Ok(mut flags) = flags_ref.lock() {
                flags.remove(&subtask_id);
            }
        });

        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct SubTaskOutputJson {
    result_summary: String,
    result_payload: String,
    grounding_notes: Option<String>,
    confidence: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct SubTaskSuggestionJson {
    title: String,
    description: String,
    execution_type: String,
    reason: Option<String>,
}

#[derive(Debug, Clone)]
struct GeneratedSubTaskOutput {
    result_summary: String,
    result_payload: String,
}

pub fn create_subtask_and_start(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    reasoning: Arc<dyn ReasoningProvider>,
    runner: &SubTaskRunner,
    task_id: i64,
    title: &str,
    description: &str,
    execution_type: &str,
    instruction_source: &str,
) -> Result<SubTaskDto> {
    let clean_title = title.trim();
    let clean_description = description.trim();
    let normalized_type = normalize_execution_type(execution_type)?;
    let normalized_source = normalize_instruction_source(instruction_source);

    if clean_title.is_empty() {
        return Err(anyhow!("subtask title cannot be empty"));
    }
    if clean_description.is_empty() {
        return Err(anyhow!("subtask description cannot be empty"));
    }

    let instruction = format!("{clean_title}\n{clean_description}");
    let (snapshot_json, _) =
        build_subtask_snapshot(store, embeddings, task_id, &instruction, normalized_type)?;

    let created = store.create_subtask(&NewSubTaskInput {
        task_id,
        title: clean_title.to_string(),
        description: clean_description.to_string(),
        execution_type: normalized_type.to_string(),
        instruction_source: normalized_source.to_string(),
        parent_context_snapshot: snapshot_json,
    })?;

    let started = runner.start_subtask(store.clone(), reasoning, created.subtask_id);
    if let Err(error) = started {
        let _ = store.transition_subtask_status(
            created.subtask_id,
            "failed",
            Some("Subtask failed to start"),
            None,
            Some(&error.to_string()),
        );
        return Err(error);
    }

    store.append_chat_message(
        task_id,
        "assistant",
        "assistant",
        MessageKind::SystemStatusEvent,
        &format!(
            "Subtask #{} started: {}.",
            created.subtask_id, created.title
        ),
    )?;

    store
        .get_subtask_by_id(created.subtask_id)?
        .ok_or_else(|| anyhow!("subtask was created but could not be reloaded"))
}

pub fn list_subtasks_for_task(store: &TaskStore, task_id: i64) -> Result<Vec<SubTaskDto>> {
    store.list_subtasks(task_id)
}

pub fn cancel_subtask(
    store: &TaskStore,
    runner: &SubTaskRunner,
    subtask_id: i64,
) -> Result<SubTaskDto> {
    let _ = runner.request_cancel(subtask_id);

    let current = store
        .get_subtask_by_id(subtask_id)?
        .ok_or_else(|| anyhow!("subtask id={} not found", subtask_id))?;

    if matches!(current.status.as_str(), "pending" | "running") {
        let cancelled = store.transition_subtask_status(
            subtask_id,
            "cancelled",
            None,
            None,
            Some("cancelled_by_user"),
        )?;

        store.append_chat_message(
            cancelled.task_id,
            "assistant",
            "assistant",
            MessageKind::SystemStatusEvent,
            &format!("Subtask #{} cancelled.", cancelled.subtask_id),
        )?;

        Ok(cancelled)
    } else {
        Ok(current)
    }
}

pub fn accept_subtask_result(store: &TaskStore, subtask_id: i64) -> Result<SubTaskDto> {
    store.set_subtask_result_review_status(subtask_id, "accepted")
}

pub fn reject_subtask_result(store: &TaskStore, subtask_id: i64) -> Result<SubTaskDto> {
    store.set_subtask_result_review_status(subtask_id, "rejected")
}

pub fn suggest_subtask_for_task(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    reasoning: &dyn ReasoningProvider,
    task_id: i64,
) -> Result<Option<SubTaskSuggestionDto>> {
    let summary = store.get_task_summary(task_id)?;
    let recent_messages = store.list_recent_chat_messages(task_id, 8)?;

    let recent_context = recent_messages
        .iter()
        .map(|message| {
            format!(
                "{} [{}]: {}",
                message.role, message.message_source, message.content
            )
        })
        .collect::<Vec<String>>()
        .join("\n");

    let suggestion_query = if recent_context.trim().is_empty() {
        summary.summary_text.clone()
    } else {
        format!("{}\n{}", summary.summary_text, recent_context)
    };

    let context_pack = build_task_context_pack(store, embeddings, task_id, &suggestion_query)?;
    if context_pack.retrieved_chunks.is_empty() {
        return Ok(None);
    }

    let snapshot = SubTaskContextSnapshot {
        task_summary: context_pack.task_summary.clone(),
        instruction: "suggest bounded subtask".to_string(),
        execution_type: "draft_generation".to_string(),
        recent_messages: recent_messages
            .iter()
            .map(|message| {
                format!(
                    "{} [{}]: {}",
                    message.role, message.message_kind, message.content
                )
            })
            .collect(),
        retrieved_chunks: context_pack.retrieved_chunks.clone(),
    };
    let snapshot_json = serde_json::to_string(&snapshot)
        .context("failed to serialize subtask suggestion context snapshot")?;

    let prompt = format!(
        "Task summary:\n{}\n\nRecent session messages:\n{}\n\nRetrieved chunks:\n{}\n\nSuggest one bounded subtask.",
        context_pack.task_summary,
        if recent_context.is_empty() {
            "<none>".to_string()
        } else {
            recent_context
        },
        context_pack
            .retrieved_chunks
            .iter()
            .enumerate()
            .map(|(index, chunk)| {
                format!(
                    "Chunk {} | {} | score {:.3}\n{}",
                    index + 1,
                    chunk.artifact_file_name,
                    chunk.similarity_score,
                    chunk.chunk_text
                )
            })
            .collect::<Vec<String>>()
            .join("\n\n")
    );

    let system_prompt = build_subtask_system_prompt(
        store,
        &context_pack.task_summary,
        "suggest bounded subtask",
        "subtask_suggestion",
    );
    let raw = reasoning.generate_response(&system_prompt, &prompt)?;
    let parsed = serde_json::from_str::<SubTaskSuggestionJson>(raw.trim())
        .unwrap_or(SubTaskSuggestionJson {
        title: "Draft a stronger intro paragraph".to_string(),
        description:
            "Draft one tighter intro paragraph grounded in citizenship framing and course readings."
                .to_string(),
        execution_type: "draft_generation".to_string(),
        reason: Some("Useful next bounded drafting step from current materials.".to_string()),
    });

    let normalized_type = normalize_execution_type(&parsed.execution_type)?;

    Ok(Some(SubTaskSuggestionDto {
        task_id,
        title: parsed.title.trim().to_string(),
        description: parsed.description.trim().to_string(),
        execution_type: normalized_type.to_string(),
        instruction_source: "system".to_string(),
        reason: parsed
            .reason
            .unwrap_or_else(|| "Bounded drafting suggestion based on task context.".to_string()),
        parent_context_snapshot: snapshot_json,
        retrieved_chunks: context_pack.retrieved_chunks,
    }))
}

pub fn refine_subtask_and_start(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    reasoning: Arc<dyn ReasoningProvider>,
    runner: &SubTaskRunner,
    subtask_id: i64,
    refinement_instruction: &str,
    instruction_source: &str,
) -> Result<SubTaskDto> {
    let source = store
        .get_subtask_by_id(subtask_id)?
        .ok_or_else(|| anyhow!("subtask id={} not found", subtask_id))?;

    if source.status != "completed" {
        return Err(anyhow!(
            "subtask id={} is not completed (status={})",
            source.subtask_id,
            source.status
        ));
    }

    let clean_refinement = refinement_instruction.trim();
    if clean_refinement.is_empty() {
        return Err(anyhow!("refinement instruction cannot be empty"));
    }

    let prior_output = source
        .result_payload
        .clone()
        .or(source.result_summary.clone())
        .unwrap_or_default();

    let title = format!("Refine: {}", source.title);
    let description = format!(
        "Refine the prior subtask output.\nRefinement request: {}\nPrior output:\n{}",
        clean_refinement, prior_output
    );

    create_subtask_and_start(
        store,
        embeddings,
        reasoning,
        runner,
        source.task_id,
        &title,
        &description,
        &source.execution_type,
        instruction_source,
    )
}

pub fn convert_subtask_result_to_revision(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    reasoning: &dyn ReasoningProvider,
    task_id: i64,
    subtask_id: i64,
    artifact_id: i64,
    selection_or_range: Option<RevisionTargetDto>,
) -> Result<RevisionProposalResultDto> {
    let subtask = store
        .get_subtask_by_id(subtask_id)?
        .ok_or_else(|| anyhow!("subtask id={} not found", subtask_id))?;

    if subtask.task_id != task_id {
        return Err(anyhow!(
            "subtask id={} does not belong to task id={}",
            subtask_id,
            task_id
        ));
    }

    if subtask.status != "completed" {
        return Err(anyhow!(
            "subtask id={} is not completed (status={})",
            subtask_id,
            subtask.status
        ));
    }

    let subtask_output = subtask
        .result_payload
        .clone()
        .or(subtask.result_summary.clone())
        .ok_or_else(|| anyhow!("subtask id={} has no output payload to convert", subtask_id))?;

    let instruction = format!(
        "Use this completed bounded subtask result as guidance for a focused revision.\n\
         Subtask title: {}\n\
         Subtask description: {}\n\
         Subtask execution type: {}\n\
         Subtask result:\n{}\n\
         Preserve grounding and avoid unsupported claims.",
        subtask.title, subtask.description, subtask.execution_type, subtask_output
    );

    let proposal = propose_artifact_revision(
        store,
        embeddings,
        reasoning,
        task_id,
        artifact_id,
        selection_or_range,
        &instruction,
        "system",
    )?;

    let _ = store.set_subtask_result_review_status(subtask_id, "converted");

    Ok(proposal)
}

pub fn create_chain_subtask_and_start(
    store: &TaskStore,
    embeddings: Arc<dyn EmbeddingProvider>,
    reasoning: Arc<dyn ReasoningProvider>,
    runner: &SubTaskRunner,
    task_id: i64,
    title: &str,
    description: &str,
    execution_type: &str,
    instruction_source: &str,
) -> Result<SubTaskDto> {
    let clean_title = title.trim();
    let clean_description = description.trim();
    let normalized_type = normalize_execution_type(execution_type)?;
    let normalized_source = normalize_instruction_source(instruction_source);

    if clean_title.is_empty() {
        return Err(anyhow!("subtask title cannot be empty"));
    }
    if clean_description.is_empty() {
        return Err(anyhow!("subtask description cannot be empty"));
    }

    let instruction = format!("{clean_title}\n{clean_description}");
    let (snapshot_json, _) = build_subtask_snapshot(
        store,
        embeddings.as_ref(),
        task_id,
        &instruction,
        normalized_type,
    )?;

    let created = store.create_subtask(&NewSubTaskInput {
        task_id,
        title: clean_title.to_string(),
        description: clean_description.to_string(),
        execution_type: normalized_type.to_string(),
        instruction_source: normalized_source.to_string(),
        parent_context_snapshot: snapshot_json,
    })?;

    let started =
        runner.start_subtask_chain(store.clone(), reasoning, embeddings, created.subtask_id);
    if let Err(error) = started {
        let _ = store.transition_subtask_status(
            created.subtask_id,
            "failed",
            Some("chain subtask failed to start"),
            None,
            Some(&error.to_string()),
        );
        return Err(error);
    }

    store.append_chat_message(
        task_id,
        "assistant",
        "assistant",
        MessageKind::SystemStatusEvent,
        &format!(
            "Chain subtask #{} started: {}.",
            created.subtask_id, created.title
        ),
    )?;

    store
        .get_subtask_by_id(created.subtask_id)?
        .ok_or_else(|| anyhow!("chain subtask was created but could not be reloaded"))
}

fn run_subtask_execution(
    store: &TaskStore,
    reasoning: &dyn ReasoningProvider,
    subtask_id: i64,
    cancel_token: &AtomicBool,
) -> Result<()> {
    let existing = store
        .get_subtask_by_id(subtask_id)?
        .ok_or_else(|| anyhow!("subtask id={} not found", subtask_id))?;

    if existing.status == "cancelled" {
        return Ok(());
    }

    if existing.status == "pending" {
        store.transition_subtask_status(subtask_id, "running", None, None, None)?;
    }

    if cancel_token.load(Ordering::SeqCst) {
        let _ = store.transition_subtask_status(
            subtask_id,
            "cancelled",
            None,
            None,
            Some("cancelled_by_user"),
        );
        return Ok(());
    }

    let running = store
        .get_subtask_by_id(subtask_id)?
        .ok_or_else(|| anyhow!("subtask id={} disappeared during execution", subtask_id))?;

    if running.status != "running" {
        return Ok(());
    }

    let snapshot: SubTaskContextSnapshot = serde_json::from_str(&running.parent_context_snapshot)
        .context("failed to parse subtask context snapshot")?;

    let result = execute_subtask_with_reasoning(store, reasoning, &running, &snapshot)?;

    if cancel_token.load(Ordering::SeqCst) {
        let _ = store.transition_subtask_status(
            subtask_id,
            "cancelled",
            None,
            None,
            Some("cancelled_by_user"),
        );
        return Ok(());
    }

    let latest = store
        .get_subtask_by_id(subtask_id)?
        .ok_or_else(|| anyhow!("subtask id={} not found before completion", subtask_id))?;
    if latest.status == "cancelled" {
        return Ok(());
    }

    let completed = store.transition_subtask_status(
        subtask_id,
        "completed",
        Some(&result.result_summary),
        Some(&result.result_payload),
        None,
    )?;

    store.append_chat_message(
        completed.task_id,
        "assistant",
        "assistant",
        MessageKind::SystemStatusEvent,
        &format!(
            "Subtask #{} completed: {}",
            completed.subtask_id, completed.title
        ),
    )?;

    Ok(())
}

fn mark_subtask_failed(store: &TaskStore, subtask_id: i64, error_message: &str) -> Result<()> {
    let subtask = store
        .get_subtask_by_id(subtask_id)?
        .ok_or_else(|| anyhow!("subtask id={} not found for failure handling", subtask_id))?;

    if subtask.status == "cancelled" {
        return Ok(());
    }

    let failed = store.transition_subtask_status(
        subtask_id,
        "failed",
        Some("Subtask execution failed"),
        None,
        Some(error_message),
    )?;

    store.append_chat_message(
        failed.task_id,
        "assistant",
        "assistant",
        MessageKind::SystemStatusEvent,
        &format!(
            "Subtask #{} failed: {}",
            failed.subtask_id,
            error_message.trim()
        ),
    )?;

    Ok(())
}

// phase 16: multi-step chain executor

fn companion_send(tx: Option<&mpsc::SyncSender<CompanionEvent>>, event: CompanionEvent) {
    if let Some(t) = tx {
        let _ = t.try_send(event);
    }
}

fn run_subtask_chain(
    store: &TaskStore,
    reasoning: &dyn ReasoningProvider,
    embeddings: &dyn EmbeddingProvider,
    subtask_id: i64,
    cancel_token: &AtomicBool,
    companion_tx: Option<&mpsc::SyncSender<CompanionEvent>>,
) -> Result<()> {
    let subtask = store
        .get_subtask_by_id(subtask_id)?
        .ok_or_else(|| anyhow!("chain subtask id={} not found", subtask_id))?;

    if subtask.status == "cancelled" {
        return Ok(());
    }

    if subtask.status == "pending" {
        store.transition_subtask_status(subtask_id, "running", None, None, None)?;
    }

    // e1: emit started after transitioning to running so the companion indicator appears.
    companion_send(
        companion_tx,
        CompanionEvent::Started {
            subtask_id,
            task_id: subtask.task_id,
            title: subtask.title.clone(),
        },
    );

    if cancel_token.load(Ordering::SeqCst) {
        let _ = store.transition_subtask_status(
            subtask_id,
            "cancelled",
            None,
            None,
            Some("cancelled_by_user"),
        );
        // e1: emit complete so the companion indicator clears.
        companion_send(
            companion_tx,
            CompanionEvent::Complete {
                subtask_id,
                task_id: subtask.task_id,
                final_status: "cancelled".to_string(),
            },
        );
        return Ok(());
    }

    let snapshot: SubTaskContextSnapshot =
        serde_json::from_str(&subtask.parent_context_snapshot)
            .context("failed to parse chain subtask context snapshot")?;

    // chain planning phase: ask LLM to produce a step list
    let planning_prompt = build_chain_planning_prompt(&subtask, &snapshot);
    let planning_system_prompt = build_subtask_system_prompt(
        store,
        &snapshot.task_summary,
        &subtask.title,
        "chain_planning",
    );
    let raw_plan = reasoning
        .generate_response(&planning_system_prompt, &planning_prompt)
        .context("chain planning LLM call failed")?;

    let mut plan = serde_json::from_str::<ChainPlan>(raw_plan.trim()).unwrap_or(ChainPlan {
        steps: vec![ChainPlanStep {
            step_type: "llm_call".to_string(),
            description: format!("{}\n{}", subtask.title, subtask.description),
            proposed_path: None,
        }],
    });

    // enforce step cap
    if plan.steps.len() > MAX_SUBTASK_STEPS {
        eprintln!(
            "[jeff] chain planner proposed {} steps for subtask id={}; truncating to {}",
            plan.steps.len(),
            subtask_id,
            MAX_SUBTASK_STEPS
        );
        plan.steps.truncate(MAX_SUBTASK_STEPS);
    }

    // filter unknown step types
    plan.steps.retain(|s| {
        let valid = matches!(
            s.step_type.as_str(),
            "llm_call" | "retrieval" | "file_write_proposal"
        );
        if !valid {
            eprintln!(
                "[jeff] unknown chain step type '{}' — skipping",
                s.step_type
            );
        }
        valid
    });

    if plan.steps.is_empty() {
        return Err(anyhow!(
            "chain plan produced no valid steps for subtask id={}",
            subtask_id
        ));
    }

    // store all steps as pending
    let mut stored_steps: Vec<SubTaskStepDto> = Vec::new();
    for (i, plan_step) in plan.steps.iter().enumerate() {
        let mut step_description = plan_step.description.trim().to_string();
        if step_description.is_empty() {
            step_description = "(no step description provided)".to_string();
        }
        step_description = truncate_chars(&step_description, MAX_CHAIN_STEP_DESCRIPTION_CHARS);

        let description = match plan_step.step_type.as_str() {
            "file_write_proposal" => {
                // embed sanitized proposed_path into description with a parseable prefix.
                let path = sanitize_relative_proposed_path(
                    plan_step.proposed_path.as_deref().unwrap_or("output.md"),
                );
                format!("path:{}|intent:{}", path, step_description)
            }
            _ => step_description,
        };
        let step =
            store.create_subtask_step(subtask_id, i as i64, &plan_step.step_type, &description)?;
        stored_steps.push(step);
    }

    // step execution loop
    let mut prior_payloads: Vec<String> = Vec::new();
    let mut final_result_payload: Option<String> = None;
    let mut final_result_summary: Option<String> = None;

    for step in &stored_steps {
        if cancel_token.load(Ordering::SeqCst) {
            let _ = store.update_subtask_step_status(
                step.id,
                "failed",
                None,
                None,
                Some("cancelled_mid_step"),
            );
            mark_remaining_steps_skipped(store, subtask_id, step.step_index + 1, &stored_steps);
            auto_reject_pending_proposals(store, subtask_id);
            let _ = store.transition_subtask_status(
                subtask_id,
                "cancelled",
                None,
                None,
                Some("cancelled_by_user"),
            );
            // e1: companion indicator must clear on mid-step cancellation.
            companion_send(
                companion_tx,
                CompanionEvent::Complete {
                    subtask_id,
                    task_id: subtask.task_id,
                    final_status: "cancelled".to_string(),
                },
            );
            return Ok(());
        }

        let _ = store.update_subtask_step_status(step.id, "running", None, None, None);
        let _ = store.update_subtask_current_step(subtask_id, step.step_index);

        let step_result = execute_chain_step(
            store,
            reasoning,
            embeddings,
            &subtask,
            step,
            &prior_payloads,
            cancel_token,
            companion_tx,
        );

        match step_result {
            Ok(payload) => {
                let summary = format!("step {} ({}) completed", step.step_index, step.step_type);
                let _ = store.update_subtask_step_status(
                    step.id,
                    "completed",
                    Some(&summary),
                    Some(&payload),
                    None,
                );
                if step.step_type != "file_write_proposal" {
                    final_result_summary = Some(summary);
                    final_result_payload = Some(payload.clone());
                }
                prior_payloads.push(payload);
            }
            Err(error) => {
                let _ = store.update_subtask_step_status(
                    step.id,
                    "failed",
                    None,
                    None,
                    Some(&error.to_string()),
                );
                mark_remaining_steps_skipped(store, subtask_id, step.step_index + 1, &stored_steps);
                auto_reject_pending_proposals(store, subtask_id);
                return Err(error);
            }
        }
    }

    let completed = store.transition_subtask_status(
        subtask_id,
        "completed",
        final_result_summary
            .as_deref()
            .or(Some("chain subtask completed")),
        final_result_payload.as_deref(),
        None,
    )?;

    store.append_chat_message(
        completed.task_id,
        "assistant",
        "assistant",
        MessageKind::SystemStatusEvent,
        &format!(
            "Chain subtask #{} completed: {}",
            completed.subtask_id, completed.title
        ),
    )?;

    // e1: emit complete so the companion indicator clears.
    companion_send(
        companion_tx,
        CompanionEvent::Complete {
            subtask_id: completed.subtask_id,
            task_id: completed.task_id,
            final_status: "completed".to_string(),
        },
    );

    Ok(())
}

fn execute_chain_step(
    store: &TaskStore,
    reasoning: &dyn ReasoningProvider,
    embeddings: &dyn EmbeddingProvider,
    subtask: &SubTaskDto,
    step: &SubTaskStepDto,
    prior_payloads: &[String],
    cancel_token: &AtomicBool,
    companion_tx: Option<&mpsc::SyncSender<CompanionEvent>>,
) -> Result<String> {
    if cancel_token.load(Ordering::SeqCst) {
        return Err(anyhow!("cancelled before step execution"));
    }

    match step.step_type.as_str() {
        "retrieval" => {
            let mut chunks =
                retrieve_relevant_chunks(store, embeddings, subtask.task_id, &step.description)
                    .unwrap_or_default();
            chunks.truncate(MAX_CHAIN_RETRIEVAL_CHUNKS);
            for chunk in &mut chunks {
                chunk.chunk_text = truncate_chars(&chunk.chunk_text, 1200);
            }
            let serialized =
                serde_json::to_string(&chunks).context("failed to serialize retrieval results")?;
            Ok(truncate_chars(&serialized, MAX_CHAIN_STEP_OUTPUT_CHARS))
        }

        "llm_call" => {
            let context = build_chain_step_context(subtask, prior_payloads, &step.description);
            let system_prompt = build_subtask_system_prompt(store, "", &subtask.title, "step_llm");
            let output = reasoning
                .generate_response(&system_prompt, &context)
                .context("chain llm_call step failed")?;
            Ok(truncate_chars(
                &character::strip_filler_phrases(&output),
                MAX_CHAIN_STEP_OUTPUT_CHARS,
            ))
        }

        "file_write_proposal" => {
            let (proposed_path, intent) = parse_file_write_step_description(&step.description);
            let context = build_chain_step_context(subtask, prior_payloads, &intent);
            let system_prompt =
                build_subtask_system_prompt(store, "", &subtask.title, "file_write_proposal");
            let content = reasoning
                .generate_response(&system_prompt, &context)
                .context("chain file_write_proposal content generation failed")?;
            if content.chars().count() > MAX_CHAIN_FILE_PROPOSAL_CONTENT_CHARS {
                return Err(anyhow!(
                    "generated file proposal content exceeds limit ({} chars > {})",
                    content.chars().count(),
                    MAX_CHAIN_FILE_PROPOSAL_CONTENT_CHARS
                ));
            }
            let proposal = store.create_file_write_proposal(
                subtask.subtask_id,
                Some(step.id),
                subtask.task_id,
                &proposed_path,
                &content,
            )?;
            // e1: notify companion so the approval card appears without requiring
            // the user to open the workspace.
            companion_send(
                companion_tx,
                CompanionEvent::WriteProposal(proposal.clone()),
            );
            Ok(format!("proposal_id:{}", proposal.id))
        }

        other => Err(anyhow!("unknown chain step type: {}", other)),
    }
}

fn build_chain_planning_prompt(subtask: &SubTaskDto, snapshot: &SubTaskContextSnapshot) -> String {
    let chunks_text = if snapshot.retrieved_chunks.is_empty() {
        "<no context chunks>".to_string()
    } else {
        snapshot
            .retrieved_chunks
            .iter()
            .enumerate()
            .map(|(i, c)| {
                format!(
                    "Chunk {} | {}\n{}",
                    i + 1,
                    c.artifact_file_name,
                    c.chunk_text
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    format!(
        "Task summary: {}\nSubtask title: {}\nSubtask description: {}\nExecution type: {}\n\nContext chunks:\n{}\n\nPlan the steps.",
        snapshot.task_summary,
        subtask.title,
        subtask.description,
        subtask.execution_type,
        chunks_text
    )
}

fn build_chain_step_context(
    subtask: &SubTaskDto,
    prior_payloads: &[String],
    step_instruction: &str,
) -> String {
    let prior_text = if prior_payloads.is_empty() {
        "<no prior steps>".to_string()
    } else {
        prior_payloads
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let capped = if p.len() > 2000 {
                    &p[..2000]
                } else {
                    p.as_str()
                };
                format!("Step {} output:\n{}", i + 1, capped)
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let context = format!(
        "Subtask: {} — {}\n\nPrior step outputs:\n{}\n\nCurrent step instruction:\n{}",
        truncate_chars(&subtask.title, 200),
        truncate_chars(&subtask.description, 600),
        prior_text,
        truncate_chars(step_instruction, 800)
    );
    truncate_chars(&context, MAX_CHAIN_CONTEXT_CHARS)
}

// parse "path:relative/file.md|intent:description text" into (path, intent)
fn parse_file_write_step_description(description: &str) -> (String, String) {
    if let Some(rest) = description.strip_prefix("path:") {
        if let Some(pipe_pos) = rest.find("|intent:") {
            let path = sanitize_relative_proposed_path(&rest[..pipe_pos]);
            let intent = truncate_chars(
                rest[pipe_pos + 8..].trim(),
                MAX_CHAIN_STEP_DESCRIPTION_CHARS,
            );
            return (path, intent);
        }
        return (
            sanitize_relative_proposed_path(rest.trim()),
            "(no intent provided)".to_string(),
        );
    }
    // fallback: use description as intent, default path
    (
        "output.md".to_string(),
        truncate_chars(description.trim(), MAX_CHAIN_STEP_DESCRIPTION_CHARS),
    )
}

fn sanitize_relative_proposed_path(raw: &str) -> String {
    let normalized = raw.trim().replace('\\', "/");
    let mut segments = Vec::new();
    for segment in normalized.split('/') {
        let cleaned = segment
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .collect::<String>();
        if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
            continue;
        }
        segments.push(cleaned);
    }
    let joined = segments.join("/");
    if joined.is_empty() {
        "output.md".to_string()
    } else {
        truncate_chars(&joined, MAX_CHAIN_PATH_CHARS)
    }
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    input.chars().take(max_chars).collect::<String>()
}

fn mark_remaining_steps_skipped(
    store: &TaskStore,
    _subtask_id: i64,
    from_index: i64,
    all_steps: &[SubTaskStepDto],
) {
    for step in all_steps {
        if step.step_index >= from_index && step.status == "pending" {
            let _ = store.update_subtask_step_status(step.id, "skipped", None, None, None);
        }
    }
}

fn auto_reject_pending_proposals(store: &TaskStore, subtask_id: i64) {
    if let Ok(proposals) = store.list_file_write_proposals_for_subtask(subtask_id) {
        for proposal in proposals {
            if proposal.status == "pending_approval" {
                let _ = store.resolve_file_write_proposal(proposal.id, "rejected");
                let _ = store.append_write_audit_entry(
                    proposal.task_id,
                    subtask_id,
                    proposal.id,
                    "rejected",
                    &proposal.proposed_path,
                );
            }
        }
    }
}

fn build_subtask_system_prompt(
    store: &TaskStore,
    task_summary: &str,
    subtask_title: &str,
    execution_type: &str,
) -> String {
    let profile_injection = if store
        .get_privacy_user_profile_memory_enabled()
        .unwrap_or(false)
    {
        crate::user_model::build_profile_injection(store)
    } else {
        None
    };

    character::build_subtask_system_prompt(&SubtaskContext {
        task_summary: task_summary.to_string(),
        subtask_title: subtask_title.to_string(),
        execution_type: execution_type.to_string(),
        profile_injection,
    })
}

fn execute_subtask_with_reasoning(
    store: &TaskStore,
    reasoning: &dyn ReasoningProvider,
    subtask: &SubTaskDto,
    snapshot: &SubTaskContextSnapshot,
) -> Result<GeneratedSubTaskOutput> {
    let execution_brief = match subtask.execution_type.as_str() {
        "draft_generation" => {
            "Draft a concise section or paragraph that can be reviewed before use."
        }
        "expansion" => "Expand outline bullets into coherent prose while staying grounded.",
        "synthesis" => "Synthesize notes into structured output with clear sections.",
        "targeted_research_synthesis" => {
            "Synthesize only from ingested artifacts; do not invent external facts."
        }
        _ => "Produce a bounded grounded drafting output.",
    };

    let prompt = format!(
        "Execution type: {}\n\
         Subtask title: {}\n\
         Subtask description: {}\n\
         Execution brief: {}\n\n\
         Task summary:\n{}\n\n\
         User instruction:\n{}\n\n\
         Recent messages:\n{}\n\n\
         Retrieved grounding chunks:\n{}\n\n\
         Return strict JSON only.",
        subtask.execution_type,
        subtask.title,
        subtask.description,
        execution_brief,
        snapshot.task_summary,
        snapshot.instruction,
        if snapshot.recent_messages.is_empty() {
            "<none>".to_string()
        } else {
            snapshot.recent_messages.join("\n")
        },
        if snapshot.retrieved_chunks.is_empty() {
            "<none>".to_string()
        } else {
            snapshot
                .retrieved_chunks
                .iter()
                .enumerate()
                .map(|(index, chunk)| {
                    format!(
                        "Chunk {} | {} | score {:.3}\n{}",
                        index + 1,
                        chunk.artifact_file_name,
                        chunk.similarity_score,
                        chunk.chunk_text
                    )
                })
                .collect::<Vec<String>>()
                .join("\n\n")
        }
    );

    let system_prompt = build_subtask_system_prompt(
        store,
        &snapshot.task_summary,
        &subtask.title,
        &subtask.execution_type,
    );
    let raw = reasoning.generate_response(&system_prompt, &prompt)?;
    Ok(parse_subtask_output(&raw, &subtask.execution_type))
}

fn build_subtask_snapshot(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    task_id: i64,
    instruction: &str,
    execution_type: &str,
) -> Result<(String, Vec<RetrievedChunkDto>)> {
    let context_pack = build_task_context_pack(store, embeddings, task_id, instruction)?;
    let recent_messages = store.list_recent_chat_messages(task_id, 8)?;
    let snapshot = SubTaskContextSnapshot {
        task_summary: context_pack.task_summary,
        instruction: instruction.to_string(),
        execution_type: execution_type.to_string(),
        recent_messages: recent_messages
            .iter()
            .map(|message| {
                format!(
                    "{} ({}) [{}]: {}",
                    message.role, message.message_source, message.message_kind, message.content
                )
            })
            .collect(),
        retrieved_chunks: context_pack.retrieved_chunks.clone(),
    };

    let snapshot_json =
        serde_json::to_string(&snapshot).context("failed to serialize subtask context snapshot")?;
    Ok((snapshot_json, context_pack.retrieved_chunks))
}

fn normalize_execution_type(value: &str) -> Result<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "draft_generation" => Ok("draft_generation"),
        "expansion" => Ok("expansion"),
        "synthesis" => Ok("synthesis"),
        "targeted_research_synthesis" => Ok("targeted_research_synthesis"),
        other => Err(anyhow!(
            "invalid execution_type '{}' (expected draft_generation, expansion, synthesis, or targeted_research_synthesis)",
            other
        )),
    }
}

fn normalize_instruction_source(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "voice" => "voice",
        "system" => "system",
        _ => "text",
    }
}

fn parse_subtask_output(raw: &str, execution_type: &str) -> GeneratedSubTaskOutput {
    if let Ok(parsed) = serde_json::from_str::<SubTaskOutputJson>(raw.trim()) {
        let _grounding_notes = parsed
            .grounding_notes
            .unwrap_or_else(|| "Grounded in task summary and retrieved chunks.".to_string());
        let _confidence = parsed.confidence.unwrap_or(0.5).clamp(0.0, 1.0);

        return GeneratedSubTaskOutput {
            result_summary: if parsed.result_summary.trim().is_empty() {
                format!("Completed {} subtask", execution_type)
            } else {
                character::strip_filler_phrases(parsed.result_summary.trim())
            },
            result_payload: if parsed.result_payload.trim().is_empty() {
                "No payload returned.".to_string()
            } else {
                character::strip_filler_phrases(parsed.result_payload.trim())
            },
        };
    }

    GeneratedSubTaskOutput {
        result_summary: format!("Completed {} subtask", execution_type),
        result_payload: character::strip_filler_phrases(raw.trim()),
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
        retrieval::{import_artifact_for_task, retrieve_relevant_chunks},
        revision::{
            apply_revision, list_artifact_versions_for_artifact, revert_artifact_to_version,
        },
        store::TaskStore,
    };

    use super::{
        accept_subtask_result, cancel_subtask, convert_subtask_result_to_revision,
        create_chain_subtask_and_start, create_subtask_and_start, list_subtasks_for_task,
        refine_subtask_and_start, reject_subtask_result, suggest_subtask_for_task, SubTaskRunner,
        MAX_CHAIN_FILE_PROPOSAL_CONTENT_CHARS, MAX_SUBTASK_STEPS,
    };

    // phase 16 chain test provider — routes responses by system prompt keyword
    #[derive(Clone)]
    struct ChainReasoningProvider {
        plan_json: String,
    }

    impl ReasoningProvider for ChainReasoningProvider {
        fn generate_response(&self, system_prompt: &str, _user_prompt: &str) -> Result<String> {
            if system_prompt.contains("chain planner") {
                return Ok(self.plan_json.clone());
            }
            if system_prompt.contains("file writer") {
                return Ok("# Output\nThis is the generated file content.".to_string());
            }
            // step executor
            Ok("Step output: grounded result from context.".to_string())
        }
    }

    #[derive(Clone)]
    struct OversizedFileReasoningProvider {
        plan_json: String,
    }

    impl ReasoningProvider for OversizedFileReasoningProvider {
        fn generate_response(&self, system_prompt: &str, _user_prompt: &str) -> Result<String> {
            if system_prompt.contains("chain planner") {
                return Ok(self.plan_json.clone());
            }
            if system_prompt.contains("file writer") {
                return Ok("x".repeat(MAX_CHAIN_FILE_PROPOSAL_CONTENT_CHARS + 1));
            }
            Ok("Step output: grounded result from context.".to_string())
        }
    }

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
                score(&["citizenship", "history", "analytical", "thesis"]),
                score(&["sections", "structure", "outline"]),
                score(&["reading", "readings", "course"]),
                score(&["intro", "introduction"]),
                (lower.len() as f32) / 1000.0,
            ])
        }
    }

    #[derive(Clone)]
    struct Phase7ReasoningProvider;

    impl ReasoningProvider for Phase7ReasoningProvider {
        fn generate_response(&self, _system_prompt: &str, user_prompt: &str) -> Result<String> {
            let lower = user_prompt.to_lowercase();

            if lower.contains("suggest one bounded subtask") {
                return Ok(
                    r#"{"title":"Draft a stronger intro","description":"Draft one tighter intro paragraph linking citizenship framing to course readings and evidence.","execution_type":"draft_generation","reason":"High-impact bounded drafting step."}"#
                        .to_string(),
                );
            }

            if lower.contains("return strict json with keys: proposed_text") {
                return Ok(
                    r#"{"proposed_text":"Citizenship debates frame this project as a focused argument grounded in course readings and primary-source evidence.","rationale":"Applied bounded subtask output.","confidence":0.82,"grounding_notes":"Aligned with storymap rubric context."}"#
                        .to_string(),
                );
            }

            if lower.contains("execution type: draft_generation")
                || lower.contains("draft a stronger intro")
            {
                return Ok(
                    r#"{"result_summary":"Drafted a stronger intro paragraph","result_payload":"This intro argues that contested definitions of citizenship shaped participation, using course readings and primary-source evidence as required by the rubric.","grounding_notes":"Grounded in rubric and notes chunks.","confidence":0.84}"#
                        .to_string(),
                );
            }

            if lower.contains("execution type: expansion") {
                return Ok(
                    r#"{"result_summary":"Expanded outline into prose","result_payload":"Expanded section text with clearer transitions and evidence anchors.","grounding_notes":"Grounded in retrieved notes.","confidence":0.78}"#
                        .to_string(),
                );
            }

            if lower.contains("execution type: synthesis")
                || lower.contains("execution type: targeted_research_synthesis")
            {
                return Ok(
                    r#"{"result_summary":"Synthesized materials","result_payload":"Synthesis: six sections, each with claim, evidence, and citation to course readings and primary sources.","grounding_notes":"Grounded in ingested artifacts only.","confidence":0.8}"#
                        .to_string(),
                );
            }

            if lower.contains("primary source requirement") {
                return Ok("Grounded answer: include primary sources, course readings, and evidence requirements.".to_string());
            }

            Ok("I don't know based on available context.".to_string())
        }
    }

    #[derive(Clone)]
    struct SlowReasoningProvider;

    impl ReasoningProvider for SlowReasoningProvider {
        fn generate_response(&self, _system_prompt: &str, _user_prompt: &str) -> Result<String> {
            thread::sleep(Duration::from_millis(400));
            Ok(
                r#"{"result_summary":"Completed slow subtask","result_payload":"Slow output grounded in notes and rubric.","grounding_notes":"Grounded.","confidence":0.7}"#
                    .to_string(),
            )
        }
    }

    fn write_file(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("failed to create parent directory");
        }
        fs::write(path, body).expect("failed to write file");
    }

    fn wait_for_terminal_subtask(store: &TaskStore, subtask_id: i64) -> crate::models::SubTaskDto {
        for _ in 0..600 {
            let subtask = store
                .get_subtask_by_id(subtask_id)
                .expect("failed to load subtask")
                .expect("subtask missing");
            if matches!(
                subtask.status.as_str(),
                "completed" | "failed" | "cancelled"
            ) {
                return subtask;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("subtask did not reach terminal state");
    }

    fn setup_storymap_store() -> (tempfile::TempDir, TaskStore, i64, i64) {
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let base_path = temp.path().join("app_local_data");
        let store = TaskStore::initialize(&base_path).expect("failed to initialize store");
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
            "Intro notes: thesis should frame citizenship debates.\n\nEach section needs evidence with course readings and primary sources.",
        );
        write_file(
            &rubric,
            "StoryMap rubric: 6 sections required. Each section should include claim, evidence requirement, and primary source support tied to course readings.",
        );

        import_artifact_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task.id,
            &notes.to_string_lossy(),
        )
        .expect("failed to import notes");
        let artifact = import_artifact_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task.id,
            &rubric.to_string_lossy(),
        )
        .expect("failed to import rubric");

        (temp, store, task.id, artifact.id)
    }

    #[test]
    fn create_and_run_subtask_lifecycle_completes_with_result() {
        let (_temp, store, task_id, _artifact_id) = setup_storymap_store();
        let runner = SubTaskRunner::new();

        let created = create_subtask_and_start(
            &store,
            &KeywordEmbeddingProvider,
            Arc::new(Phase7ReasoningProvider),
            &runner,
            task_id,
            "Draft a stronger intro",
            "Draft a stronger intro grounded in citizenship and readings.",
            "draft_generation",
            "text",
        )
        .expect("failed to create/start subtask");

        assert!(matches!(
            created.status.as_str(),
            "pending" | "running" | "completed"
        ));

        let completed = wait_for_terminal_subtask(&store, created.subtask_id);
        assert_eq!(completed.status, "completed");
        assert!(completed
            .result_payload
            .as_deref()
            .unwrap_or_default()
            .to_lowercase()
            .contains("citizenship"));
    }

    #[test]
    fn context_snapshot_is_immutable_across_execution() {
        let (_temp, store, task_id, _artifact_id) = setup_storymap_store();
        let runner = SubTaskRunner::new();

        let created = create_subtask_and_start(
            &store,
            &KeywordEmbeddingProvider,
            Arc::new(Phase7ReasoningProvider),
            &runner,
            task_id,
            "Synthesize notes",
            "Synthesize sections and evidence requirements.",
            "synthesis",
            "text",
        )
        .expect("failed to create/start subtask");

        let initial_snapshot = created.parent_context_snapshot.clone();
        let terminal = wait_for_terminal_subtask(&store, created.subtask_id);
        assert_eq!(terminal.parent_context_snapshot, initial_snapshot);

        let parsed: super::SubTaskContextSnapshot =
            serde_json::from_str(&terminal.parent_context_snapshot)
                .expect("failed to parse snapshot");
        assert!(parsed.instruction.to_lowercase().contains("synthesize"));
        assert!(!parsed.retrieved_chunks.is_empty());
    }

    #[test]
    fn execution_routing_and_review_status_controls_work() {
        let (_temp, store, task_id, _artifact_id) = setup_storymap_store();
        let runner = SubTaskRunner::new();

        let created = create_subtask_and_start(
            &store,
            &KeywordEmbeddingProvider,
            Arc::new(Phase7ReasoningProvider),
            &runner,
            task_id,
            "Expand outline",
            "Expand the outline into fuller text.",
            "expansion",
            "text",
        )
        .expect("failed to create subtask");

        let completed = wait_for_terminal_subtask(&store, created.subtask_id);
        assert_eq!(completed.status, "completed");
        assert!(completed
            .result_summary
            .as_deref()
            .unwrap_or_default()
            .to_lowercase()
            .contains("expanded"));

        let accepted = accept_subtask_result(&store, completed.subtask_id)
            .expect("failed to accept subtask result");
        assert_eq!(accepted.result_review_status, "accepted");
        let rejected = reject_subtask_result(&store, completed.subtask_id)
            .expect("failed to reject subtask result");
        assert_eq!(rejected.result_review_status, "rejected");
    }

    #[test]
    fn parallel_behavior_keeps_chat_responsive_while_subtask_runs() {
        let (_temp, store, task_id, _artifact_id) = setup_storymap_store();
        let runner = SubTaskRunner::new();

        let created = create_subtask_and_start(
            &store,
            &KeywordEmbeddingProvider,
            Arc::new(SlowReasoningProvider),
            &runner,
            task_id,
            "Draft while user continues",
            "Generate intro draft in background.",
            "draft_generation",
            "text",
        )
        .expect("failed to create background subtask");

        thread::sleep(Duration::from_millis(40));
        let in_flight = store
            .get_subtask_by_id(created.subtask_id)
            .expect("failed to read in-flight subtask")
            .expect("missing in-flight subtask");
        assert!(matches!(in_flight.status.as_str(), "pending" | "running"));

        let chat_response = send_message_for_task(
            &store,
            &KeywordEmbeddingProvider,
            &Phase7ReasoningProvider,
            task_id,
            "What is the primary source requirement?",
            "text",
            None,
            None,
            || false,
        )
        .expect("send_message failed while subtask was running");
        assert!(!chat_response.cancelled);

        let terminal = wait_for_terminal_subtask(&store, created.subtask_id);
        assert_eq!(terminal.status, "completed");
    }

    #[test]
    fn cancel_behavior_prevents_completion_and_keeps_no_result_payload() {
        let (_temp, store, task_id, _artifact_id) = setup_storymap_store();
        let runner = SubTaskRunner::new();

        let created = create_subtask_and_start(
            &store,
            &KeywordEmbeddingProvider,
            Arc::new(SlowReasoningProvider),
            &runner,
            task_id,
            "Cancelable intro draft",
            "Draft intro text in background.",
            "draft_generation",
            "voice",
        )
        .expect("failed to create/start subtask");

        thread::sleep(Duration::from_millis(30));
        let cancelled =
            cancel_subtask(&store, &runner, created.subtask_id).expect("failed to cancel subtask");
        assert_eq!(cancelled.status, "cancelled");

        thread::sleep(Duration::from_millis(450));
        let final_state = store
            .get_subtask_by_id(created.subtask_id)
            .expect("failed to read cancelled subtask")
            .expect("missing cancelled subtask");
        assert_eq!(final_state.status, "cancelled");
        assert!(final_state.result_payload.is_none());
    }

    #[test]
    fn result_to_revision_path_requires_explicit_accept_before_file_change_and_revert_works() {
        let (_temp, store, task_id, _artifact_id) = setup_storymap_store();
        let runner = SubTaskRunner::new();

        let editable = store
            .list_artifacts(task_id)
            .expect("failed to list artifacts")
            .into_iter()
            .find(|artifact| artifact.file_extension == "md")
            .expect("expected editable markdown artifact");

        let before_content = fs::read_to_string(&editable.stored_path)
            .expect("failed to read artifact before conversion");

        let created = create_subtask_and_start(
            &store,
            &KeywordEmbeddingProvider,
            Arc::new(Phase7ReasoningProvider),
            &runner,
            task_id,
            "Draft stronger intro",
            "Draft intro with citizenship framing.",
            "draft_generation",
            "text",
        )
        .expect("failed to create subtask");

        let completed = wait_for_terminal_subtask(&store, created.subtask_id);
        assert_eq!(completed.status, "completed");

        let proposal = convert_subtask_result_to_revision(
            &store,
            &KeywordEmbeddingProvider,
            &Phase7ReasoningProvider,
            task_id,
            completed.subtask_id,
            editable.id,
            None,
        )
        .expect("failed to convert subtask result to revision proposal");

        let after_convert_content = fs::read_to_string(&editable.stored_path)
            .expect("failed to read artifact after conversion");
        assert_eq!(
            before_content, after_convert_content,
            "artifact should remain unchanged until revision accept"
        );

        let apply = apply_revision(
            &store,
            &KeywordEmbeddingProvider,
            proposal.proposal.revision_id,
        )
        .expect("failed to apply revision");
        assert_ne!(apply.artifact_content.content, before_content);

        let versions = list_artifact_versions_for_artifact(&store, editable.id)
            .expect("failed to list artifact versions");
        assert!(!versions.is_empty());

        let reverted =
            revert_artifact_to_version(&store, &KeywordEmbeddingProvider, versions[0].version_id)
                .expect("failed to revert artifact");
        assert_eq!(reverted.content, before_content);
    }

    #[test]
    fn scenario_and_regression_checks_pass_after_subtasks() {
        let (_temp, store, task_id, _artifact_id) = setup_storymap_store();
        let runner = SubTaskRunner::new();

        let suggested = suggest_subtask_for_task(
            &store,
            &KeywordEmbeddingProvider,
            &Phase7ReasoningProvider,
            task_id,
        )
        .expect("failed to get suggestion")
        .expect("expected bounded subtask suggestion");
        assert_eq!(suggested.execution_type, "draft_generation");

        let created = create_subtask_and_start(
            &store,
            &KeywordEmbeddingProvider,
            Arc::new(Phase7ReasoningProvider),
            &runner,
            task_id,
            "Draft a stronger intro",
            "Draft a stronger intro and connect to citizenship and readings.",
            "draft_generation",
            "text",
        )
        .expect("failed to create scenario subtask");

        let completed = wait_for_terminal_subtask(&store, created.subtask_id);
        assert!(completed
            .result_payload
            .as_deref()
            .unwrap_or_default()
            .to_lowercase()
            .contains("course readings"));

        let refined = refine_subtask_and_start(
            &store,
            &KeywordEmbeddingProvider,
            Arc::new(Phase7ReasoningProvider),
            &runner,
            completed.subtask_id,
            "tighten this further",
            "voice",
        )
        .expect("failed to start refinement subtask");
        let refined_done = wait_for_terminal_subtask(&store, refined.subtask_id);
        assert_eq!(refined_done.status, "completed");

        let chunks = retrieve_relevant_chunks(
            &store,
            &KeywordEmbeddingProvider,
            task_id,
            "primary source requirement",
        )
        .expect("retrieval failed after subtask flow");
        assert!(!chunks.is_empty());

        let mut runtime = CoworkingRuntime::default();
        runtime.set_proactive_mode(true, 0);
        runtime.set_user_typing(false, 0);
        runtime.set_user_speaking(false, 0);
        let proactive = evaluate_proactive_nudge_for_task(
            &store,
            &KeywordEmbeddingProvider,
            &Phase7ReasoningProvider,
            &mut runtime,
            task_id,
            100,
        )
        .expect("proactive evaluation failed after subtask flow");
        assert!(
            matches!(
                proactive.decision_event_type.as_str(),
                "assistant_nudge" | "system_status_event"
            ),
            "unexpected proactive event: {}",
            proactive.decision_event_type
        );

        let listed = list_subtasks_for_task(&store, task_id).expect("failed to list subtasks");
        assert!(listed
            .iter()
            .any(|item| item.subtask_id == completed.subtask_id));
    }

    // phase 16 chain executor tests

    #[test]
    fn subtask_chain_runs_and_all_steps_complete() {
        let (_temp, store, task_id, _artifact_id) = setup_storymap_store();
        let runner = SubTaskRunner::new();

        let plan_json = r#"{"steps":[{"step_type":"retrieval","description":"find context chunks"},{"step_type":"llm_call","description":"summarize themes from retrieved context"}]}"#.to_string();
        let provider = ChainReasoningProvider { plan_json };

        let created = create_chain_subtask_and_start(
            &store,
            Arc::new(KeywordEmbeddingProvider),
            Arc::new(provider),
            &runner,
            task_id,
            "Chain summary task",
            "Retrieve context and summarize key themes.",
            "synthesis",
            "text",
        )
        .expect("failed to create chain subtask");

        let terminal = wait_for_terminal_subtask(&store, created.subtask_id);
        assert_eq!(
            terminal.status, "completed",
            "chain subtask should complete"
        );

        let steps = store
            .list_subtask_steps(created.subtask_id)
            .expect("failed to list chain steps");
        assert_eq!(steps.len(), 2, "expected 2 stored steps");
        for step in &steps {
            assert_eq!(step.status, "completed", "each step should be completed");
        }
    }

    #[test]
    fn subtask_chain_file_write_proposal_creates_db_record_not_disk_file() {
        let (_temp, store, task_id, _artifact_id) = setup_storymap_store();
        let runner = SubTaskRunner::new();

        let plan_json = r#"{"steps":[{"step_type":"file_write_proposal","description":"path:output.md|intent:Draft the summary output","proposed_path":"output.md"}]}"#.to_string();
        let provider = ChainReasoningProvider { plan_json };

        let created = create_chain_subtask_and_start(
            &store,
            Arc::new(KeywordEmbeddingProvider),
            Arc::new(provider),
            &runner,
            task_id,
            "File write chain task",
            "Generate and propose output.md.",
            "draft_generation",
            "text",
        )
        .expect("failed to create file-write chain subtask");

        let terminal = wait_for_terminal_subtask(&store, created.subtask_id);
        assert_eq!(terminal.status, "completed");

        let proposals = store
            .list_file_write_proposals_for_subtask(created.subtask_id)
            .expect("failed to list file write proposals");
        assert_eq!(
            proposals.len(),
            1,
            "expected exactly one file write proposal in db"
        );
        assert_eq!(
            proposals[0].status, "pending_approval",
            "proposal should await approval, never auto-written"
        );
        assert_eq!(proposals[0].proposed_path, "output.md");

        // the file must NOT exist on disk — proposals are never auto-applied
        assert!(
            !std::path::Path::new("output.md").exists(),
            "file must not be written to disk without explicit approval"
        );
    }

    #[test]
    fn subtask_chain_truncates_plan_to_max_steps_limit() {
        let (_temp, store, task_id, _artifact_id) = setup_storymap_store();
        let runner = SubTaskRunner::new();

        // plan has 7 steps — should be truncated to MAX_SUBTASK_STEPS (5)
        let plan_json = r#"{"steps":[
            {"step_type":"llm_call","description":"step 1"},
            {"step_type":"llm_call","description":"step 2"},
            {"step_type":"llm_call","description":"step 3"},
            {"step_type":"llm_call","description":"step 4"},
            {"step_type":"llm_call","description":"step 5"},
            {"step_type":"llm_call","description":"step 6"},
            {"step_type":"llm_call","description":"step 7"}
        ]}"#
        .to_string();
        let provider = ChainReasoningProvider { plan_json };

        let created = create_chain_subtask_and_start(
            &store,
            Arc::new(KeywordEmbeddingProvider),
            Arc::new(provider),
            &runner,
            task_id,
            "Overlong chain task",
            "Seven-step plan that must be truncated.",
            "synthesis",
            "text",
        )
        .expect("failed to create overlong chain subtask");

        let terminal = wait_for_terminal_subtask(&store, created.subtask_id);
        assert_eq!(terminal.status, "completed");

        let steps = store
            .list_subtask_steps(created.subtask_id)
            .expect("failed to list chain steps");
        assert!(
            steps.len() <= MAX_SUBTASK_STEPS,
            "stored steps ({}) must not exceed MAX_SUBTASK_STEPS ({})",
            steps.len(),
            MAX_SUBTASK_STEPS
        );
    }

    #[test]
    fn subtask_chain_cancel_leaves_no_pending_approval_proposals() {
        let (_temp, store, task_id, _artifact_id) = setup_storymap_store();
        let runner = SubTaskRunner::new();

        // slow provider: planning call takes 400ms — cancel fires before any step executes
        let created = create_chain_subtask_and_start(
            &store,
            Arc::new(KeywordEmbeddingProvider),
            Arc::new(SlowReasoningProvider),
            &runner,
            task_id,
            "Cancelable chain task",
            "This chain will be cancelled before any file write proposal can be created.",
            "draft_generation",
            "text",
        )
        .expect("failed to create cancelable chain subtask");

        thread::sleep(Duration::from_millis(30));
        let _ = cancel_subtask(&store, &runner, created.subtask_id)
            .expect("failed to cancel chain subtask");

        let terminal = wait_for_terminal_subtask(&store, created.subtask_id);
        assert_eq!(terminal.status, "cancelled");

        let proposals = store
            .list_file_write_proposals_for_subtask(created.subtask_id)
            .expect("failed to list proposals after cancel");
        // no proposal should be in pending_approval — either none were created or they were auto-rejected
        let pending = proposals
            .iter()
            .filter(|p| p.status == "pending_approval")
            .count();
        assert_eq!(
            pending, 0,
            "no proposals should be pending approval after cancellation"
        );
    }

    #[test]
    fn subtask_chain_rejects_oversized_file_write_proposal_content() {
        let (_temp, store, task_id, _artifact_id) = setup_storymap_store();
        let runner = SubTaskRunner::new();

        let plan_json = r#"{"steps":[{"step_type":"file_write_proposal","description":"path:output.md|intent:Write long output","proposed_path":"output.md"}]}"#.to_string();
        let provider = OversizedFileReasoningProvider { plan_json };

        let created = create_chain_subtask_and_start(
            &store,
            Arc::new(KeywordEmbeddingProvider),
            Arc::new(provider),
            &runner,
            task_id,
            "Oversized file proposal task",
            "Generate a proposal that exceeds configured content limits.",
            "draft_generation",
            "text",
        )
        .expect("failed to create oversized-content chain subtask");

        let terminal = wait_for_terminal_subtask(&store, created.subtask_id);
        assert_eq!(terminal.status, "failed");

        let proposals = store
            .list_file_write_proposals_for_subtask(created.subtask_id)
            .expect("failed to list proposals");
        assert_eq!(
            proposals.len(),
            0,
            "oversized content should fail the step before proposal persistence"
        );
    }

    #[test]
    fn subtask_chain_sanitizes_unsafe_proposed_path_before_persisting_proposal() {
        let (_temp, store, task_id, _artifact_id) = setup_storymap_store();
        let runner = SubTaskRunner::new();

        let plan_json = r#"{"steps":[{"step_type":"file_write_proposal","description":"path:../../nested/../unsafe\\out.md|intent:Draft output","proposed_path":"../../nested/../unsafe\\out.md"}]}"#.to_string();
        let provider = ChainReasoningProvider { plan_json };

        let created = create_chain_subtask_and_start(
            &store,
            Arc::new(KeywordEmbeddingProvider),
            Arc::new(provider),
            &runner,
            task_id,
            "Path sanitize task",
            "Generate a file-write proposal with an unsafe path.",
            "draft_generation",
            "text",
        )
        .expect("failed to create path sanitize subtask");

        let terminal = wait_for_terminal_subtask(&store, created.subtask_id);
        assert_eq!(terminal.status, "completed");

        let proposals = store
            .list_file_write_proposals_for_subtask(created.subtask_id)
            .expect("failed to list proposals");
        assert_eq!(proposals.len(), 1);
        assert!(!proposals[0].proposed_path.contains(".."));
        assert!(!proposals[0].proposed_path.starts_with('/'));
        assert_eq!(proposals[0].proposed_path, "nested/unsafe/out.md");
    }
}
