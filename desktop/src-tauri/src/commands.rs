use std::{fs, path::PathBuf, time::Duration};

use tauri::{AppHandle, Manager, Runtime, State};

use crate::{
    ambient,
    chat::send_message_for_task,
    coworking::{evaluate_proactive_nudge_for_task, unix_now_seconds},
    flow::{
        accept_suggestion_for_task, dismiss_suggestion_for_task,
        evaluate_next_suggestions_for_task, explain_suggestion_for_task,
    },
    message_kind::classify_user_message_kind,
    models::{
        ApiKeyValidationDto, ArtifactContentDto, ArtifactDto, ArtifactVersionDto, ChatMessageDto,
        CoworkingStatusDto, DriftFlagDto, EventLogEntryDto, FileWriteProposalDto,
        IntentClassificationDto, IntentLabel, IntentSlotsDto, OnboardingStatusDto, OpenResourceDto,
        ProactiveEvaluationDto, RecentlyLearnedItemDto, ReorientationDto, RetrievedChunkDto,
        RevisionApplyResultDto, RevisionProposalDto, RevisionProposalResultDto, RevisionTargetDto,
        SendMessageResponseDto, SessionModeStateDto, SpeechSynthesisDto, SubTaskDto,
        SubTaskStepDto, SubTaskSuggestionDto, SuggestionAcceptanceDto, SuggestionDto,
        SuggestionEvaluationDto, TaskContextPackDto, TaskDto, TaskSummaryDto,
        TranscriptionResultDto, WatcherStatusDto, WorkspaceInfoDto, WriteAuditEntryDto,
    },
    retrieval::{
        auto_ingest_file_for_task, build_task_context_pack, import_artifact_for_task,
        retrieve_relevant_chunks,
    },
    revision::{
        apply_revision as apply_artifact_revision, get_artifact_content_for_edit,
        list_artifact_versions_for_artifact, list_pending_revisions_for_artifact,
        propose_artifact_revision as propose_revision_for_artifact,
        reject_revision as reject_artifact_revision,
        revert_artifact_to_version as revert_artifact_by_version,
    },
    state::JeffState,
    subtask::{
        accept_subtask_result as accept_subtask_result_by_id,
        cancel_subtask as cancel_subtask_by_id, convert_subtask_result_to_revision,
        create_chain_subtask_and_start, create_subtask_and_start, list_subtasks_for_task,
        refine_subtask_and_start, reject_subtask_result as reject_subtask_result_by_id,
        suggest_subtask_for_task,
    },
};

fn map_jeff_error<E: ToString>(error: E) -> String {
    crate::errors::map_error_message(&error.to_string())
}

#[tauri::command]
pub fn create_task(state: State<'_, JeffState>, title: String) -> Result<TaskDto, String> {
    state.store.create_task(&title).map_err(map_jeff_error)
}

#[tauri::command]
pub fn list_tasks(state: State<'_, JeffState>) -> Result<Vec<TaskDto>, String> {
    state.store.list_tasks().map_err(map_jeff_error)
}

#[tauri::command]
pub fn get_active_task(state: State<'_, JeffState>) -> Result<Option<TaskDto>, String> {
    state.store.get_active_task().map_err(map_jeff_error)
}

#[tauri::command]
pub fn set_active_task(state: State<'_, JeffState>, task_id: i64) -> Result<TaskDto, String> {
    let task = state
        .store
        .set_active_task(task_id)
        .map_err(map_jeff_error)?;

    if let Err(err) = ensure_workspace_awareness_for_task(state.inner(), task_id) {
        eprintln!("[jeff watcher] failed to sync watcher for active task {task_id}: {err}");
    }

    Ok(task)
}

// phase 18: onboarding + secure key setup -----------------------------------

#[tauri::command]
pub fn get_onboarding_status(state: State<'_, JeffState>) -> Result<OnboardingStatusDto, String> {
    let onboarding_complete = state
        .store
        .get_onboarding_complete()
        .map_err(map_jeff_error)?;
    let preferred_workspace_folder = state
        .store
        .get_preferred_workspace_folder()
        .map_err(map_jeff_error)?;

    let resolved = crate::secrets::resolve_openai_api_key();

    Ok(OnboardingStatusDto {
        onboarding_complete,
        has_stored_api_key: resolved.api_key.is_some(),
        api_key_source: resolved.source.to_string(),
        preferred_workspace_folder,
    })
}

#[tauri::command]
pub fn complete_onboarding(state: State<'_, JeffState>) -> Result<(), String> {
    state
        .store
        .set_onboarding_complete(true)
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn set_preferred_workspace_folder(
    state: State<'_, JeffState>,
    folder_path: String,
) -> Result<(), String> {
    let clean = folder_path.trim();
    if clean.is_empty() {
        return Err("folder_path cannot be empty".to_string());
    }
    state
        .store
        .set_preferred_workspace_folder(Some(clean))
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn clear_preferred_workspace_folder(state: State<'_, JeffState>) -> Result<(), String> {
    state
        .store
        .set_preferred_workspace_folder(None)
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn validate_openai_api_key(api_key: String) -> Result<ApiKeyValidationDto, String> {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return Ok(ApiKeyValidationDto {
            is_valid: false,
            message: "API key cannot be empty.".to_string(),
        });
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .connect_timeout(Duration::from_secs(8))
        .build()
        .map_err(map_jeff_error)?;

    let response = client
        .get("https://api.openai.com/v1/models")
        .bearer_auth(trimmed)
        .send();

    let response = match response {
        Ok(resp) => resp,
        Err(err) => {
            let message = if err.is_timeout() {
                crate::errors::JeffError::ApiTimeout.to_string()
            } else {
                format!("OpenAI key validation failed: {err}")
            };
            return Ok(ApiKeyValidationDto {
                is_valid: false,
                message,
            });
        }
    };

    if response.status().is_success() {
        return Ok(ApiKeyValidationDto {
            is_valid: true,
            message: "API key validated successfully.".to_string(),
        });
    }

    if response.status().as_u16() == 401 {
        return Ok(ApiKeyValidationDto {
            is_valid: false,
            message: crate::errors::JeffError::InvalidApiKey.to_string(),
        });
    }

    Ok(ApiKeyValidationDto {
        is_valid: false,
        message: format!("OpenAI rejected the key (status {}).", response.status()),
    })
}

#[tauri::command]
pub fn store_openai_api_key(api_key: String) -> Result<(), String> {
    crate::secrets::store_openai_api_key(&api_key).map_err(map_jeff_error)
}

#[tauri::command]
pub fn delete_openai_api_key() -> Result<(), String> {
    crate::secrets::delete_openai_api_key().map_err(map_jeff_error)
}

#[tauri::command]
pub fn get_task_workspace(
    state: State<'_, JeffState>,
    task_id: i64,
) -> Result<WorkspaceInfoDto, String> {
    state
        .store
        .get_task_workspace(task_id)
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn get_task_summary(
    state: State<'_, JeffState>,
    task_id: i64,
) -> Result<TaskSummaryDto, String> {
    state
        .store
        .get_task_summary(task_id)
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn list_open_resources(
    state: State<'_, JeffState>,
    task_id: i64,
) -> Result<Vec<OpenResourceDto>, String> {
    state
        .store
        .list_open_resources(task_id)
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn import_artifact(
    state: State<'_, JeffState>,
    task_id: i64,
    file_path: String,
) -> Result<ArtifactDto, String> {
    import_artifact_for_task(&state.store, state.embeddings.as_ref(), task_id, &file_path)
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn list_artifacts(
    state: State<'_, JeffState>,
    task_id: i64,
) -> Result<Vec<ArtifactDto>, String> {
    state.store.list_artifacts(task_id).map_err(map_jeff_error)
}

#[tauri::command]
pub fn retrieve_context(
    state: State<'_, JeffState>,
    task_id: i64,
    query: String,
) -> Result<Vec<RetrievedChunkDto>, String> {
    retrieve_relevant_chunks(&state.store, state.embeddings.as_ref(), task_id, &query)
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn build_context_pack(
    state: State<'_, JeffState>,
    task_id: i64,
    query: String,
) -> Result<TaskContextPackDto, String> {
    build_task_context_pack(&state.store, state.embeddings.as_ref(), task_id, &query)
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn list_messages(
    state: State<'_, JeffState>,
    task_id: i64,
) -> Result<Vec<ChatMessageDto>, String> {
    state
        .store
        .list_chat_messages(task_id)
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn send_message<R: Runtime>(
    state: State<'_, JeffState>,
    app: AppHandle<R>,
    task_id: i64,
    message: String,
    source: Option<String>,
) -> Result<SendMessageResponseDto, String> {
    let now = unix_now_seconds().map_err(map_jeff_error)?;
    let user_message_kind = classify_user_message_kind(&message);
    {
        let mut runtime = state
            .coworking
            .lock()
            .map_err(|_| "failed to lock coworking runtime".to_string())?;
        runtime.note_user_message(user_message_kind, now);
    }

    let epoch = state.next_interaction_epoch();
    let message_source = normalize_message_source(source);

    let response = send_message_for_task(
        &state.store,
        state.embeddings.as_ref(),
        state.reasoning.as_ref(),
        task_id,
        &message,
        &message_source,
        || state.current_interaction_epoch() != epoch,
    );

    match response {
        Ok(value) => {
            let now = unix_now_seconds().map_err(map_jeff_error)?;
            let mut runtime = state
                .coworking
                .lock()
                .map_err(|_| "failed to lock coworking runtime".to_string())?;
            if value.cancelled {
                runtime.note_interruption(now);
            } else {
                runtime.note_assistant_answer(now);
                notify_if_backgrounded(
                    &app,
                    "Jeff finished a response",
                    &value.assistant_response,
                    Some("assistant_answer".to_string()),
                    None,
                );
            }
            Ok(value)
        }
        Err(error) => Err(map_jeff_error(error)),
    }
}

fn is_window_visible<R: Runtime>(app: &AppHandle<R>, label: &str) -> bool {
    app.get_webview_window(label)
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false)
}

fn should_notify_when_backgrounded<R: Runtime>(app: &AppHandle<R>) -> bool {
    let overlay_visible = is_window_visible(app, ambient::OVERLAY_WINDOW_LABEL);
    let main_visible = is_window_visible(app, ambient::MAIN_WINDOW_LABEL);
    !(overlay_visible || main_visible)
}

fn compact_notification_body(content: &str, max_chars: usize) -> String {
    let compact = content.split_whitespace().collect::<Vec<&str>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }

    let mut trimmed: String = compact.chars().take(max_chars).collect();
    while trimmed.ends_with(' ') {
        trimmed.pop();
    }
    format!("{trimmed}...")
}

fn notify_if_backgrounded<R: Runtime>(
    app: &AppHandle<R>,
    title: &str,
    body: &str,
    context_kind: Option<String>,
    context_id: Option<i64>,
) {
    if !should_notify_when_backgrounded(app) {
        return;
    }
    let text = compact_notification_body(body, 160);
    if text.trim().is_empty() {
        return;
    }
    let _ = ambient::dispatch_notification(
        app,
        ambient::NotificationPayload {
            title: title.to_string(),
            body: text,
            context_kind,
            context_id,
        },
    );
}

fn normalize_message_source(source: Option<String>) -> String {
    source
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| value == "text" || value == "voice")
        .unwrap_or_else(|| "text".to_string())
}

fn normalize_subtask_instruction_source(source: Option<String>) -> String {
    source
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| value == "text" || value == "voice" || value == "system")
        .unwrap_or_else(|| "text".to_string())
}

fn active_artifact_setting_key(task_id: i64) -> String {
    format!("active_artifact_task_{task_id}")
}

// phase 12: streaming send. starts an async llm stream for the given task
// and returns the turn_id so the frontend can subscribe to stream:// events.
#[tauri::command]
pub async fn send_message_streaming<R: Runtime>(
    state: State<'_, JeffState>,
    app: AppHandle<R>,
    task_id: i64,
    message: String,
    source: Option<String>,
) -> Result<String, String> {
    use crate::{
        chat_streaming::start_streaming_turn,
        streaming::{new_turn_id, InteractionToken},
    };

    let turn_id = new_turn_id();
    let token = InteractionToken::new(turn_id.clone());
    state.interactions.register(&token);

    // start_streaming_turn spawns the async work and returns immediately.
    if let Err(err) = start_streaming_turn(
        &state,
        app,
        task_id,
        message,
        source.unwrap_or_else(|| "text".to_string()),
        token.clone(),
        state.interactions.clone(),
    )
    .await
    {
        state.interactions.remove(&turn_id);
        return Err(map_jeff_error(err));
    }

    Ok(turn_id)
}

// cancels an active streaming turn by turn_id. safe to call even if the
// turn has already completed (returns false without error).
#[tauri::command]
pub fn cancel_streaming_turn(
    state: State<'_, JeffState>,
    turn_id: String,
    reason: Option<String>,
) -> bool {
    state
        .interactions
        .cancel(&turn_id, reason.as_deref().map(str::trim))
}

#[tauri::command]
pub fn cancel_interaction(state: State<'_, JeffState>) -> u64 {
    if let Ok(now) = unix_now_seconds() {
        if let Ok(mut runtime) = state.coworking.lock() {
            runtime.note_interruption(now);
        }
    }
    state.next_interaction_epoch()
}

#[tauri::command]
pub fn transcribe_audio(
    state: State<'_, JeffState>,
    audio_base64: String,
    mime_type: String,
) -> Result<TranscriptionResultDto, String> {
    state
        .voice
        .transcribe_audio_base64(&audio_base64, &mime_type)
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn synthesize_speech(
    state: State<'_, JeffState>,
    text: String,
) -> Result<SpeechSynthesisDto, String> {
    state.voice.synthesize_speech(&text).map_err(map_jeff_error)
}

#[tauri::command]
pub fn get_coworking_status(state: State<'_, JeffState>) -> Result<CoworkingStatusDto, String> {
    let now = unix_now_seconds().map_err(map_jeff_error)?;
    let mut runtime = state
        .coworking
        .lock()
        .map_err(|_| "failed to lock coworking runtime".to_string())?;
    Ok(runtime.status(now))
}

#[tauri::command]
pub fn set_proactive_mode(
    state: State<'_, JeffState>,
    enabled: bool,
) -> Result<CoworkingStatusDto, String> {
    let now = unix_now_seconds().map_err(map_jeff_error)?;
    let mut runtime = state
        .coworking
        .lock()
        .map_err(|_| "failed to lock coworking runtime".to_string())?;
    let status = runtime.set_proactive_mode(enabled, now);
    state
        .store
        .set_app_setting("proactive_mode", if enabled { "1" } else { "0" })
        .map_err(map_jeff_error)?;
    Ok(status)
}

#[tauri::command]
pub fn set_user_typing(
    state: State<'_, JeffState>,
    is_typing: bool,
) -> Result<CoworkingStatusDto, String> {
    let now = unix_now_seconds().map_err(map_jeff_error)?;
    let mut runtime = state
        .coworking
        .lock()
        .map_err(|_| "failed to lock coworking runtime".to_string())?;
    Ok(runtime.set_user_typing(is_typing, now))
}

#[tauri::command]
pub fn set_user_speaking(
    state: State<'_, JeffState>,
    is_speaking: bool,
) -> Result<CoworkingStatusDto, String> {
    let now = unix_now_seconds().map_err(map_jeff_error)?;
    let mut runtime = state
        .coworking
        .lock()
        .map_err(|_| "failed to lock coworking runtime".to_string())?;
    Ok(runtime.set_user_speaking(is_speaking, now))
}

#[tauri::command]
pub fn set_assistant_speaking(
    state: State<'_, JeffState>,
    is_speaking: bool,
) -> Result<CoworkingStatusDto, String> {
    let now = unix_now_seconds().map_err(map_jeff_error)?;
    let mut runtime = state
        .coworking
        .lock()
        .map_err(|_| "failed to lock coworking runtime".to_string())?;
    Ok(runtime.set_assistant_speaking(is_speaking, now))
}

#[tauri::command]
pub fn evaluate_proactive_nudge<R: Runtime>(
    state: State<'_, JeffState>,
    app: AppHandle<R>,
    task_id: i64,
) -> Result<ProactiveEvaluationDto, String> {
    let now = unix_now_seconds().map_err(map_jeff_error)?;
    let evaluation = {
        let mut runtime = state
            .coworking
            .lock()
            .map_err(|_| "failed to lock coworking runtime".to_string())?;

        evaluate_proactive_nudge_for_task(
            &state.store,
            state.embeddings.as_ref(),
            state.reasoning.as_ref(),
            &mut runtime,
            task_id,
            now,
        )
        .map_err(map_jeff_error)?
    };

    if let Some(nudge) = evaluation.nudge.as_ref() {
        notify_if_backgrounded(
            &app,
            "Jeff has a nudge",
            &nudge.message,
            Some("assistant_nudge".to_string()),
            None,
        );
    }

    Ok(evaluation)
}

#[tauri::command]
pub fn get_artifact_content(
    state: State<'_, JeffState>,
    artifact_id: i64,
) -> Result<ArtifactContentDto, String> {
    get_artifact_content_for_edit(&state.store, artifact_id).map_err(map_jeff_error)
}

#[tauri::command]
pub fn propose_artifact_revision(
    state: State<'_, JeffState>,
    task_id: i64,
    artifact_id: i64,
    selection_or_range: Option<RevisionTargetDto>,
    instruction: String,
    instruction_source: Option<String>,
) -> Result<RevisionProposalResultDto, String> {
    let source = instruction_source.unwrap_or_else(|| "typed".to_string());
    propose_revision_for_artifact(
        &state.store,
        state.embeddings.as_ref(),
        state.reasoning.as_ref(),
        task_id,
        artifact_id,
        selection_or_range,
        &instruction,
        &source,
    )
    .map_err(map_jeff_error)
}

#[tauri::command]
pub fn list_pending_revisions(
    state: State<'_, JeffState>,
    task_id: i64,
    artifact_id: i64,
) -> Result<Vec<RevisionProposalDto>, String> {
    list_pending_revisions_for_artifact(&state.store, task_id, artifact_id).map_err(map_jeff_error)
}

#[tauri::command]
pub fn list_task_pending_revisions(
    state: State<'_, JeffState>,
    task_id: i64,
) -> Result<Vec<RevisionProposalDto>, String> {
    state
        .store
        .list_pending_revisions_for_task(task_id)
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn apply_revision(
    state: State<'_, JeffState>,
    revision_id: i64,
) -> Result<RevisionApplyResultDto, String> {
    apply_artifact_revision(&state.store, state.embeddings.as_ref(), revision_id)
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn reject_revision(
    state: State<'_, JeffState>,
    revision_id: i64,
) -> Result<RevisionProposalDto, String> {
    reject_artifact_revision(&state.store, revision_id).map_err(map_jeff_error)
}

#[tauri::command]
pub fn list_artifact_versions(
    state: State<'_, JeffState>,
    artifact_id: i64,
) -> Result<Vec<ArtifactVersionDto>, String> {
    list_artifact_versions_for_artifact(&state.store, artifact_id).map_err(map_jeff_error)
}

#[tauri::command]
pub fn revert_artifact_to_version(
    state: State<'_, JeffState>,
    version_id: i64,
) -> Result<ArtifactContentDto, String> {
    revert_artifact_by_version(&state.store, state.embeddings.as_ref(), version_id)
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn create_subtask(
    state: State<'_, JeffState>,
    task_id: i64,
    title: String,
    description: String,
    execution_type: String,
    instruction_source: Option<String>,
) -> Result<SubTaskDto, String> {
    create_subtask_and_start(
        &state.store,
        state.embeddings.as_ref(),
        state.reasoning.clone(),
        state.subtasks.as_ref(),
        task_id,
        &title,
        &description,
        &execution_type,
        &normalize_subtask_instruction_source(instruction_source),
    )
    .map_err(map_jeff_error)
}

#[tauri::command]
pub fn list_subtasks(state: State<'_, JeffState>, task_id: i64) -> Result<Vec<SubTaskDto>, String> {
    list_subtasks_for_task(&state.store, task_id).map_err(map_jeff_error)
}

#[tauri::command]
pub fn cancel_subtask(state: State<'_, JeffState>, subtask_id: i64) -> Result<SubTaskDto, String> {
    cancel_subtask_by_id(&state.store, state.subtasks.as_ref(), subtask_id).map_err(map_jeff_error)
}

#[tauri::command]
pub fn accept_subtask_result(
    state: State<'_, JeffState>,
    subtask_id: i64,
) -> Result<SubTaskDto, String> {
    accept_subtask_result_by_id(&state.store, subtask_id).map_err(map_jeff_error)
}

#[tauri::command]
pub fn reject_subtask_result(
    state: State<'_, JeffState>,
    subtask_id: i64,
) -> Result<SubTaskDto, String> {
    reject_subtask_result_by_id(&state.store, subtask_id).map_err(map_jeff_error)
}

#[tauri::command]
pub fn suggest_subtask(
    state: State<'_, JeffState>,
    task_id: i64,
) -> Result<Option<SubTaskSuggestionDto>, String> {
    suggest_subtask_for_task(
        &state.store,
        state.embeddings.as_ref(),
        state.reasoning.as_ref(),
        task_id,
    )
    .map_err(map_jeff_error)
}

#[tauri::command]
pub fn refine_subtask(
    state: State<'_, JeffState>,
    subtask_id: i64,
    instruction: String,
    instruction_source: Option<String>,
) -> Result<SubTaskDto, String> {
    refine_subtask_and_start(
        &state.store,
        state.embeddings.as_ref(),
        state.reasoning.clone(),
        state.subtasks.as_ref(),
        subtask_id,
        &instruction,
        &normalize_subtask_instruction_source(instruction_source),
    )
    .map_err(map_jeff_error)
}

#[tauri::command]
pub fn convert_subtask_to_revision(
    state: State<'_, JeffState>,
    task_id: i64,
    subtask_id: i64,
    artifact_id: i64,
    selection_or_range: Option<RevisionTargetDto>,
) -> Result<RevisionProposalResultDto, String> {
    convert_subtask_result_to_revision(
        &state.store,
        state.embeddings.as_ref(),
        state.reasoning.as_ref(),
        task_id,
        subtask_id,
        artifact_id,
        selection_or_range,
    )
    .map_err(map_jeff_error)
}

#[tauri::command]
pub fn evaluate_next_suggestions(
    state: State<'_, JeffState>,
    task_id: i64,
    active_artifact_id: Option<i64>,
) -> Result<SuggestionEvaluationDto, String> {
    let now = unix_now_seconds().map_err(map_jeff_error)?;
    let mut runtime = state
        .coworking
        .lock()
        .map_err(|_| "failed to lock coworking runtime".to_string())?;
    let status = runtime.status(now);

    evaluate_next_suggestions_for_task(
        &state.store,
        state.embeddings.as_ref(),
        task_id,
        active_artifact_id,
        &status,
    )
    .map_err(map_jeff_error)
}

#[tauri::command]
pub fn list_suggestions(
    state: State<'_, JeffState>,
    task_id: i64,
) -> Result<Vec<SuggestionDto>, String> {
    state
        .store
        .list_suggestions(task_id, false)
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn dismiss_suggestion(
    state: State<'_, JeffState>,
    task_id: i64,
    suggestion_id: i64,
) -> Result<SuggestionDto, String> {
    dismiss_suggestion_for_task(&state.store, task_id, suggestion_id).map_err(map_jeff_error)
}

#[tauri::command]
pub fn explain_suggestion(
    state: State<'_, JeffState>,
    task_id: i64,
    suggestion_id: i64,
) -> Result<String, String> {
    explain_suggestion_for_task(&state.store, task_id, suggestion_id).map_err(map_jeff_error)
}

#[tauri::command]
pub fn accept_suggestion(
    state: State<'_, JeffState>,
    task_id: i64,
    suggestion_id: i64,
    active_artifact_id: Option<i64>,
    selection_or_range: Option<RevisionTargetDto>,
) -> Result<SuggestionAcceptanceDto, String> {
    accept_suggestion_for_task(
        &state.store,
        state.embeddings.as_ref(),
        state.reasoning.clone(),
        state.subtasks.as_ref(),
        task_id,
        suggestion_id,
        active_artifact_id,
        selection_or_range,
    )
    .map_err(map_jeff_error)
}

#[tauri::command]
pub fn get_session_mode_state(
    state: State<'_, JeffState>,
    task_id: i64,
) -> Result<Option<SessionModeStateDto>, String> {
    state
        .store
        .get_session_mode_state(task_id)
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn list_recent_events(
    state: State<'_, JeffState>,
    task_id: i64,
    limit: Option<usize>,
) -> Result<Vec<EventLogEntryDto>, String> {
    state
        .store
        .list_recent_events(task_id, limit.unwrap_or(20))
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn get_active_artifact_selection(
    state: State<'_, JeffState>,
    task_id: i64,
) -> Result<Option<i64>, String> {
    let key = active_artifact_setting_key(task_id);
    let raw = state.store.get_app_setting(&key).map_err(map_jeff_error)?;

    let Some(value) = raw else {
        return Ok(None);
    };

    if value.trim().is_empty() {
        return Ok(None);
    }

    value
        .trim()
        .parse::<i64>()
        .map(Some)
        .map_err(|error| format!("invalid active artifact selection for task {task_id}: {error}"))
}

#[tauri::command]
pub fn set_active_artifact_selection(
    state: State<'_, JeffState>,
    task_id: i64,
    artifact_id: Option<i64>,
) -> Result<Option<i64>, String> {
    let key = active_artifact_setting_key(task_id);
    if let Some(value) = artifact_id {
        if value <= 0 {
            return Err(
                "artifact_id must be positive when setting active artifact selection".to_string(),
            );
        }

        state
            .store
            .set_app_setting(&key, &value.to_string())
            .map_err(map_jeff_error)?;
        Ok(Some(value))
    } else {
        state
            .store
            .delete_app_setting(&key)
            .map_err(map_jeff_error)?;
        Ok(None)
    }
}

// phase 13: workspace watcher commands --------------------------------------

fn start_watcher_and_persist_folder(
    state: &crate::state::JeffState,
    task_id: i64,
    folder_path: PathBuf,
) -> Result<WatcherStatusDto, String> {
    let status = crate::watcher::start_watcher(
        state.watcher.clone(),
        task_id,
        folder_path,
        state.store.clone(),
        state.embeddings.clone(),
    )
    .map_err(map_jeff_error)?;

    if let Some(watched_path) = status.watched_path.as_deref() {
        state
            .store
            .set_watched_folder(task_id, watched_path)
            .map_err(map_jeff_error)?;
    }

    Ok(status)
}

fn sync_clipboard_poll_for_active_task(
    state: &crate::state::JeffState,
    task_id: i64,
) -> Result<(), String> {
    let enabled = state
        .store
        .get_clipboard_capture(task_id)
        .map_err(map_jeff_error)?;

    if enabled {
        crate::watcher::start_clipboard_poll(
            state.watcher.clone(),
            task_id,
            state.store.clone(),
            state.embeddings.clone(),
        );
    } else {
        crate::watcher::stop_clipboard_poll(state.watcher.clone(), task_id);
    }

    Ok(())
}

pub fn ensure_workspace_awareness_for_task(
    state: &crate::state::JeffState,
    task_id: i64,
) -> Result<WatcherStatusDto, String> {
    crate::watcher::stop_all_except(state.watcher.clone(), Some(task_id));

    let workspace_path = state
        .store
        .get_task_workspace_path(task_id)
        .map_err(map_jeff_error)?;

    let configured_path = state
        .store
        .get_watched_folder(task_id)
        .map_err(map_jeff_error)?
        .map(|entry| entry.folder_path.trim().to_string())
        .filter(|path| !path.is_empty());

    let mut candidates = Vec::<PathBuf>::new();
    if let Some(path) = configured_path {
        let candidate = PathBuf::from(path);
        if !candidate.as_os_str().is_empty() {
            candidates.push(candidate);
        }
    }
    if !candidates
        .iter()
        .any(|candidate| candidate == &workspace_path)
    {
        candidates.push(workspace_path);
    }

    let mut last_error: Option<String> = None;
    for candidate in candidates {
        match start_watcher_and_persist_folder(state, task_id, candidate) {
            Ok(status) => {
                sync_clipboard_poll_for_active_task(state, task_id)?;
                return Ok(status);
            }
            Err(err) => {
                last_error = Some(err);
            }
        }
    }

    crate::watcher::stop_clipboard_poll(state.watcher.clone(), task_id);
    Err(last_error.unwrap_or_else(|| {
        format!("unable to start watcher for task {task_id} with any candidate path")
    }))
}

pub fn restore_workspace_awareness_for_active_task(
    state: &crate::state::JeffState,
) -> Result<(), String> {
    let active_task = state.store.get_active_task().map_err(map_jeff_error)?;
    if let Some(task) = active_task {
        ensure_workspace_awareness_for_task(state, task.id)?;
    } else {
        crate::watcher::stop_all_except(state.watcher.clone(), None);
    }
    Ok(())
}

#[tauri::command]
pub fn start_workspace_watcher(
    state: State<'_, crate::state::JeffState>,
    task_id: i64,
    folder_path: String,
) -> Result<WatcherStatusDto, String> {
    let path = PathBuf::from(folder_path.trim());
    if path.as_os_str().is_empty() {
        return Err("folder_path cannot be empty".to_string());
    }

    crate::watcher::stop_all_except(state.watcher.clone(), Some(task_id));
    let status = start_watcher_and_persist_folder(state.inner(), task_id, path)?;
    sync_clipboard_poll_for_active_task(state.inner(), task_id)?;
    Ok(status)
}

#[tauri::command]
pub fn stop_workspace_watcher(
    state: State<'_, crate::state::JeffState>,
    task_id: i64,
) -> Result<WatcherStatusDto, String> {
    state
        .store
        .clear_watched_folder(task_id)
        .map_err(map_jeff_error)?;

    Ok(crate::watcher::stop_watcher(state.watcher.clone(), task_id))
}

#[tauri::command]
pub fn get_watcher_status(
    state: State<'_, crate::state::JeffState>,
    task_id: i64,
) -> WatcherStatusDto {
    crate::watcher::get_watcher_status(state.watcher.clone(), task_id)
}

#[tauri::command]
pub fn list_recently_learned(
    state: State<'_, crate::state::JeffState>,
    task_id: i64,
    limit: Option<usize>,
) -> Result<Vec<RecentlyLearnedItemDto>, String> {
    state
        .store
        .list_recently_learned(task_id, limit.unwrap_or(10))
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn set_clipboard_capture(
    state: State<'_, crate::state::JeffState>,
    task_id: i64,
    enabled: bool,
) -> Result<(), String> {
    state
        .store
        .set_clipboard_capture(task_id, enabled)
        .map_err(map_jeff_error)?;

    let active_task_id = state
        .store
        .get_active_task()
        .map_err(map_jeff_error)?
        .map(|task| task.id);

    if active_task_id == Some(task_id) {
        sync_clipboard_poll_for_active_task(state.inner(), task_id)?;
    } else if !enabled {
        crate::watcher::stop_clipboard_poll(state.watcher.clone(), task_id);
    }

    Ok(())
}

#[tauri::command]
pub fn get_clipboard_capture_setting(
    state: State<'_, crate::state::JeffState>,
    task_id: i64,
) -> Result<bool, String> {
    state
        .store
        .get_clipboard_capture(task_id)
        .map_err(map_jeff_error)
}

// phase 14: intent classification command ------------------------------------

#[tauri::command]
pub fn classify_message_intent(
    state: State<'_, crate::state::JeffState>,
    task_id: i64,
    message_text: String,
) -> Result<IntentClassificationDto, String> {
    let trimmed = message_text.trim();
    if trimmed.is_empty() {
        return Ok(IntentClassificationDto {
            intent: IntentLabel::Unknown,
            confidence: 0.0,
            slots: IntentSlotsDto::default(),
        });
    }

    state
        .store
        .get_task_summary(task_id)
        .map_err(map_jeff_error)?;

    let api_key = crate::secrets::resolve_openai_api_key_required().map_err(map_jeff_error)?;
    crate::classifier::classify_intent(trimmed, &api_key).map_err(map_jeff_error)
}

// phase 15: proactive initiation commands

#[tauri::command]
pub fn trigger_task_resume(
    state: State<'_, JeffState>,
    ambient: State<'_, crate::ambient::AmbientState>,
    task_id: i64,
) -> Result<ReorientationDto, String> {
    if ambient.is_quiet_mode() {
        return Ok(ReorientationDto {
            task_id,
            summary: String::new(),
            fired_at: String::new(),
        });
    }
    crate::proactive::generate_reorientation(&state.store, state.reasoning.as_ref(), task_id)
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn check_task_drift(
    state: State<'_, JeffState>,
    ambient: State<'_, crate::ambient::AmbientState>,
    task_id: i64,
    current_text: String,
) -> Result<DriftFlagDto, String> {
    if ambient.is_quiet_mode() {
        return Ok(DriftFlagDto {
            task_id,
            is_drifting: false,
            flag_reason: String::new(),
            confidence: 0.0,
        });
    }
    crate::proactive::evaluate_drift(
        &state.store,
        state.reasoning.as_ref(),
        state.embeddings.as_ref(),
        task_id,
        &current_text,
    )
    .map_err(map_jeff_error)
}

#[tauri::command]
pub fn trigger_speculative_subtask(
    state: State<'_, JeffState>,
    ambient: State<'_, crate::ambient::AmbientState>,
    task_id: i64,
) -> Result<Option<SubTaskDto>, String> {
    if ambient.is_quiet_mode() {
        return Ok(None);
    }
    crate::proactive::propose_speculative_subtask(
        &state.store,
        state.embeddings.as_ref(),
        std::sync::Arc::clone(&state.reasoning),
        &state.subtasks,
        task_id,
    )
    .map_err(map_jeff_error)
}

#[tauri::command]
pub fn dismiss_proactive_trigger(
    state: State<'_, JeffState>,
    task_id: i64,
    trigger_type: String,
) -> Result<(), String> {
    state
        .store
        .record_proactive_trigger(task_id, &trigger_type, true)
        .map(|_| ())
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn record_task_focus(state: State<'_, JeffState>, task_id: i64) -> Result<(), String> {
    state
        .store
        .record_task_focus(task_id)
        .map_err(map_jeff_error)
}

// phase 16: richer parallel work commands ------------------------------------

#[tauri::command]
pub fn list_subtask_steps(
    state: State<'_, JeffState>,
    task_id: i64,
    subtask_id: i64,
) -> Result<Vec<SubTaskStepDto>, String> {
    // verify subtask belongs to this task before exposing steps
    let subtask = state
        .store
        .get_subtask_by_id(subtask_id)
        .map_err(map_jeff_error)?
        .ok_or_else(|| format!("subtask id={subtask_id} not found"))?;
    if subtask.task_id != task_id {
        return Err(format!(
            "subtask id={subtask_id} does not belong to task id={task_id}"
        ));
    }
    state
        .store
        .list_subtask_steps(subtask_id)
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn list_file_write_proposals(
    state: State<'_, JeffState>,
    task_id: i64,
) -> Result<Vec<FileWriteProposalDto>, String> {
    state
        .store
        .list_pending_file_write_proposals(task_id)
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn approve_subtask_file_write(
    state: State<'_, JeffState>,
    task_id: i64,
    proposal_id: i64,
) -> Result<WriteAuditEntryDto, String> {
    let proposal = state
        .store
        .get_file_write_proposal_by_id(proposal_id)
        .map_err(map_jeff_error)?
        .ok_or_else(|| format!("file write proposal id={proposal_id} not found"))?;

    if proposal.task_id != task_id {
        return Err(format!(
            "proposal id={proposal_id} does not belong to task id={task_id}"
        ));
    }
    if proposal.status != "pending_approval" {
        return Err(format!(
            "proposal id={proposal_id} is not pending approval (status={})",
            proposal.status
        ));
    }

    // parent subtask must be completed before any file write can be approved
    let parent_subtask = state
        .store
        .get_subtask_by_id(proposal.subtask_id)
        .map_err(map_jeff_error)?
        .ok_or_else(|| {
            format!(
                "parent subtask id={} for proposal id={} not found",
                proposal.subtask_id, proposal_id
            )
        })?;
    if parent_subtask.status != "completed" {
        return Err(format!(
            "cannot approve proposal id={proposal_id} while parent subtask id={} is {} (must be completed)",
            parent_subtask.subtask_id, parent_subtask.status
        ));
    }

    // path safety: reject absolute paths and any non-normal components (no ..)
    let raw_path = std::path::Path::new(&proposal.proposed_path);
    if raw_path.is_absolute() {
        return Err("proposed_path must be relative, not absolute".to_string());
    }
    for component in raw_path.components() {
        match component {
            std::path::Component::Normal(_) => {}
            _ => {
                return Err(format!(
                    "proposed_path '{}' contains invalid path component — only simple relative paths allowed",
                    proposal.proposed_path
                ));
            }
        }
    }

    // resolve destination: workspace_path / proposed_path, then enforce canonical parent containment.
    let workspace_uncanonical = state
        .store
        .get_task_workspace_path(task_id)
        .map_err(map_jeff_error)?;
    fs::create_dir_all(&workspace_uncanonical).map_err(|e| {
        format!(
            "failed to create task workspace '{}': {e}",
            workspace_uncanonical.display()
        )
    })?;
    let workspace = fs::canonicalize(&workspace_uncanonical).map_err(|e| {
        format!(
            "failed to canonicalize task workspace '{}': {e}",
            workspace_uncanonical.display()
        )
    })?;
    let dest = workspace.join(raw_path);

    // create parent directories if needed, then write
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "failed to create parent directories for '{}': {e}",
                dest.display()
            )
        })?;
        let canonical_parent = fs::canonicalize(parent).map_err(|e| {
            format!(
                "failed to canonicalize target parent '{}' for proposal id={proposal_id}: {e}",
                parent.display()
            )
        })?;
        if !canonical_parent.starts_with(&workspace) {
            return Err(format!(
                "proposed_path '{}' escapes task workspace",
                proposal.proposed_path
            ));
        }
    } else {
        return Err(format!(
            "proposed_path '{}' does not have a valid parent directory",
            proposal.proposed_path
        ));
    }

    // proposal state machine: pending_approval -> applying -> approved.
    // if file write fails we rollback applying -> pending_approval so the proposal can be retried.
    state
        .store
        .begin_file_write_proposal_apply(proposal_id)
        .map_err(map_jeff_error)?;

    if let Err(write_err) = fs::write(&dest, &proposal.proposed_content) {
        let rollback_err = state.store.rollback_file_write_proposal_apply(proposal_id);
        let _ = state.store.append_write_audit_entry(
            proposal.task_id,
            proposal.subtask_id,
            proposal_id,
            "apply_failed",
            &proposal.proposed_path,
        );
        return match rollback_err {
            Ok(_) => Err(format!("failed to write file '{}': {write_err}", dest.display())),
            Err(revert_err) => Err(format!(
                "failed to write file '{}' ({write_err}); also failed to rollback proposal state: {revert_err}",
                dest.display()
            )),
        };
    }

    state
        .store
        .complete_file_write_proposal_apply(proposal_id)
        .map_err(|e| {
            format!(
                "file '{}' was written but proposal finalize failed; manual review recommended: {e}",
                dest.display()
            )
        })?;

    // record audit entry
    state
        .store
        .append_write_audit_entry(
            proposal.task_id,
            proposal.subtask_id,
            proposal_id,
            "approved",
            &proposal.proposed_path,
        )
        .map_err(map_jeff_error)?;

    // ingest approved write so retrieval state is up to date.
    if let Err(err) = auto_ingest_file_for_task(
        &state.store,
        state.embeddings.as_ref(),
        proposal.task_id,
        &dest,
    ) {
        eprintln!(
            "[jeff] file write approved for proposal id={} but auto-ingest failed: {}",
            proposal_id, err
        );
    }

    // fetch and return audit entry (last insert)
    let entries = state
        .store
        .list_write_audit_log(task_id, 1)
        .map_err(map_jeff_error)?;
    entries
        .into_iter()
        .next()
        .ok_or_else(|| "audit entry was written but could not be loaded".to_string())
}

#[tauri::command]
pub fn reject_subtask_file_write(
    state: State<'_, JeffState>,
    task_id: i64,
    proposal_id: i64,
) -> Result<WriteAuditEntryDto, String> {
    let proposal = state
        .store
        .get_file_write_proposal_by_id(proposal_id)
        .map_err(map_jeff_error)?
        .ok_or_else(|| format!("file write proposal id={proposal_id} not found"))?;

    if proposal.task_id != task_id {
        return Err(format!(
            "proposal id={proposal_id} does not belong to task id={task_id}"
        ));
    }
    if proposal.status != "pending_approval" {
        return Err(format!(
            "proposal id={proposal_id} is not pending approval (status={})",
            proposal.status
        ));
    }

    state
        .store
        .resolve_file_write_proposal(proposal_id, "rejected")
        .map_err(map_jeff_error)?;

    state
        .store
        .append_write_audit_entry(
            proposal.task_id,
            proposal.subtask_id,
            proposal_id,
            "rejected",
            &proposal.proposed_path,
        )
        .map_err(map_jeff_error)?;

    let entries = state
        .store
        .list_write_audit_log(task_id, 1)
        .map_err(map_jeff_error)?;
    entries
        .into_iter()
        .next()
        .ok_or_else(|| "audit entry was written but could not be loaded".to_string())
}

#[tauri::command]
pub fn list_write_audit_log(
    state: State<'_, JeffState>,
    task_id: i64,
) -> Result<Vec<WriteAuditEntryDto>, String> {
    state
        .store
        .list_write_audit_log(task_id, 100)
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn start_subtask_chain(
    state: State<'_, JeffState>,
    task_id: i64,
    title: String,
    description: String,
    execution_type: String,
    instruction_source: Option<String>,
) -> Result<SubTaskDto, String> {
    let source = instruction_source.unwrap_or_else(|| "text".to_string());
    create_chain_subtask_and_start(
        &state.store,
        state.embeddings.clone(),
        state.reasoning.clone(),
        &state.subtasks,
        task_id,
        &title,
        &description,
        &execution_type,
        &source,
    )
    .map_err(map_jeff_error)
}

#[cfg(test)]
mod tests {
    use super::normalize_message_source;

    #[test]
    fn normalize_message_source_accepts_text_and_voice() {
        assert_eq!(normalize_message_source(Some("text".to_string())), "text");
        assert_eq!(normalize_message_source(Some("voice".to_string())), "voice");
        assert_eq!(normalize_message_source(Some("VOICE".to_string())), "voice");
    }

    #[test]
    fn normalize_message_source_defaults_for_invalid_values() {
        assert_eq!(normalize_message_source(None), "text");
        assert_eq!(
            normalize_message_source(Some("assistant".to_string())),
            "text"
        );
        assert_eq!(normalize_message_source(Some("".to_string())), "text");
    }
}
