use std::{fs, path::PathBuf, time::Duration};

use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::{
    ambient,
    chat::send_message_for_task,
    context_observer,
    coworking::{evaluate_proactive_nudge_for_task, unix_now_seconds},
    flow::{
        accept_suggestion_for_task, dismiss_suggestion_for_task,
        evaluate_next_suggestions_for_task, explain_suggestion_for_task,
    },
    message_kind::classify_user_message_kind,
    models::{
        ActiveWindowContextDto, ApiKeyValidationDto, ArtifactContentDto, ArtifactDto,
        ArtifactVersionDto, BrowserSelectionCaptureRequestDto, CalendarEventDto, ChatMessageDto,
        CoworkingStatusDto, DataClearResultDto, DriftFlagDto, EventLogEntryDto,
        FileWriteProposalDto, IntentClassificationDto, IntentLabel, IntentSlotsDto,
        LiveEditReceiptDto, OnboardingStatusDto, OpenResourceDto, PendingLiveEditDto,
        PrivacyCenterDashboardDto, ProactiveAuditEntryDto, ProactiveEvaluationDto,
        RecentlyLearnedItemDto, ReorientationDto, RetrievedChunkDto, RevisionApplyResultDto,
        RevisionProposalDto, RevisionProposalResultDto, RevisionTargetDto,
        SelectionBridgeStatusDto, SelectionCaptureIndicatorDto, SendMessageResponseDto,
        SessionModeStateDto, SessionRestoreDto, SpeechSynthesisDto, SubTaskDto, SubTaskStepDto,
        SubTaskSuggestionDto, SuggestionAcceptanceDto, SuggestionDto, SuggestionEvaluationDto,
        SynthesisLogEntryDto, TaskContextPackDto, TaskDto, TaskSummaryDto, TranscriptionResultDto,
        UserProfileSignalDto, WatcherStatusDto, WorkloadSummaryDto, WorkspaceInfoDto,
        WriteAuditEntryDto,
    },
    retrieval::{
        auto_ingest_file_for_task, build_task_context_pack, import_artifact_for_task,
        retrieve_relevant_chunks,
    },
    revision::{
        apply_revision as apply_artifact_revision,
        generate_revision_alternative as generate_revision_alternative_inner,
        get_artifact_content_for_edit, list_artifact_versions_for_artifact,
        list_pending_revisions_for_artifact,
        propose_artifact_revision as propose_revision_for_artifact,
        reject_revision as reject_artifact_revision,
        revert_artifact_to_version as revert_artifact_by_version,
    },
    selection_capture::SelectionCaptureState,
    similarity::cosine_similarity,
    state::{ContextState, JeffState},
    subtask::{
        accept_subtask_result as accept_subtask_result_by_id,
        cancel_subtask as cancel_subtask_by_id, convert_subtask_result_to_revision,
        create_chain_subtask_and_start, create_subtask_and_start, list_subtasks_for_task,
        refine_subtask_and_start, reject_subtask_result as reject_subtask_result_by_id,
        suggest_subtask_for_task,
    },
    user_model, workload,
};

fn map_jeff_error<E: ToString>(error: E) -> String {
    crate::errors::map_error_message(&error.to_string())
}

fn user_profile_memory_enabled(state: &JeffState) -> bool {
    state
        .store
        .get_privacy_user_profile_memory_enabled()
        .unwrap_or(false)
}

// phase 20: build the context prefix string from ContextState.
// returns None when no context is available or both fields are empty.
fn active_context_string(ctx_state: &ContextState) -> Option<String> {
    let ctx = ctx_state.current()?;
    if ctx.app_name.is_empty() && ctx.document_title.is_empty() {
        return None;
    }
    Some(format!(
        "User's active app: {}. Document: {}.",
        ctx.app_name, ctx.document_title
    ))
}

fn active_context_string_if_enabled(state: &JeffState, ctx_state: &ContextState) -> Option<String> {
    if !state
        .store
        .get_privacy_active_window_context_enabled()
        .unwrap_or(true)
    {
        return None;
    }
    active_context_string(ctx_state)
}

fn next_message_context<R: Runtime>(
    app: &AppHandle<R>,
    state: &JeffState,
    ctx_state: &ContextState,
    selection_state: &SelectionCaptureState,
) -> Option<String> {
    let active_context = active_context_string_if_enabled(state, ctx_state);
    let selection_context = selection_state.take_prompt_context();
    if selection_context.is_some() {
        let _ = app.emit(
            crate::selection_capture::EVENT_SELECTION_CLEARED,
            serde_json::json!({}),
        );
    }

    match (active_context, selection_context) {
        (Some(active), Some(selection)) => Some(format!("{active}\n\n{selection}")),
        (Some(active), None) => Some(active),
        (None, Some(selection)) => Some(selection),
        (None, None) => None,
    }
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
pub fn set_active_task<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, JeffState>,
    task_id: i64,
) -> Result<TaskDto, String> {
    let task = state
        .store
        .set_active_task(task_id)
        .map_err(map_jeff_error)?;

    if let Err(err) = ensure_workspace_awareness_for_task(state.inner(), task_id) {
        eprintln!("[jeff watcher] failed to sync watcher for active task {task_id}: {err}");
    }

    let _ = app.emit(
        "task://active-changed",
        serde_json::json!({ "task_id": task.id }),
    );

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
        .map_err(map_jeff_error)?;

    // clear any stale watched_folders entry so the next ensure_workspace call
    // picks up the new preference rather than re-using the old entry.
    if let Some(task) = state.store.get_active_task().map_err(map_jeff_error)? {
        let _ = state.store.clear_watched_folder(task.id);
        let _ = ensure_workspace_awareness_for_task(state.inner(), task.id);
    }

    Ok(())
}

#[tauri::command]
pub fn clear_preferred_workspace_folder(state: State<'_, JeffState>) -> Result<(), String> {
    state
        .store
        .set_preferred_workspace_folder(None)
        .map_err(map_jeff_error)
}

// runs the blocking reqwest call on a thread-pool thread so the tauri command
// thread is not blocked for the full 8-second timeout window.
#[tauri::command]
pub async fn validate_openai_api_key(api_key: String) -> Result<ApiKeyValidationDto, String> {
    let trimmed = api_key.trim().to_string();
    if trimmed.is_empty() {
        return Ok(ApiKeyValidationDto {
            is_valid: false,
            message: "API key cannot be empty.".to_string(),
        });
    }

    tokio::task::spawn_blocking(move || {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(8))
            .connect_timeout(Duration::from_secs(8))
            .build()
            .map_err(|err| format!("failed to build http client: {err}"))?;

        let response = client
            .get("https://api.openai.com/v1/models")
            .bearer_auth(&trimmed)
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
    })
    .await
    .map_err(|join_err| format!("validation task panicked: {join_err}"))?
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
pub fn get_workspace_prompt_dismissed(state: State<'_, JeffState>) -> Result<bool, String> {
    state
        .store
        .get_workspace_prompt_dismissed()
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn set_workspace_prompt_dismissed(
    state: State<'_, JeffState>,
    dismissed: bool,
) -> Result<(), String> {
    state
        .store
        .set_workspace_prompt_dismissed(dismissed)
        .map_err(map_jeff_error)
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
    context_state: State<'_, ContextState>,
    selection_state: State<'_, SelectionCaptureState>,
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
    let active_ctx = next_message_context(&app, state.inner(), &context_state, &selection_state);
    let current_snapshot = state.awareness_core.snapshot_immediate();
    let current_snapshot_summary = crate::awareness_core::snapshot_summary(&current_snapshot);

    let response = send_message_for_task(
        &state.store,
        state.embeddings.as_ref(),
        state.reasoning.as_ref(),
        task_id,
        &message,
        &message_source,
        active_ctx.as_deref(),
        Some(current_snapshot_summary.as_str()).filter(|value| !value.is_empty()),
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
                if user_profile_memory_enabled(state.inner()) {
                    let word_count = value.assistant_response.split_whitespace().count();
                    let _ = user_model::record_response_length(&state.store, word_count);
                }
                notify_if_backgrounded(
                    &app,
                    "Jeff finished a response",
                    &value.assistant_response,
                    Some("assistant_answer".to_string()),
                    None,
                );
                crate::awareness_core::spawn_awareness_update(
                    &app,
                    crate::awareness_core::SnapshotTrigger::NewTurn,
                    task_id,
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
    !overlay_visible
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
    context_state: State<'_, ContextState>,
    selection_state: State<'_, SelectionCaptureState>,
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

    let active_ctx = next_message_context(&app, state.inner(), &context_state, &selection_state);

    // start_streaming_turn spawns the async work and returns immediately.
    if let Err(err) = start_streaming_turn(
        &state,
        app,
        task_id,
        message,
        source.unwrap_or_else(|| "text".to_string()),
        token.clone(),
        state.interactions.clone(),
        active_ctx,
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
    let voice = state.store.get_tts_voice().map_err(map_jeff_error)?;
    let spoken_text = crate::voice_naturalness::prepare_tts_text(&text, "non-streaming");
    state
        .voice
        .synthesize_speech(&spoken_text, &voice)
        .map_err(map_jeff_error)
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
    state
        .store
        .set_privacy_proactive_triggers_enabled(enabled)
        .map_err(map_jeff_error)?;
    Ok(status)
}

#[tauri::command]
pub fn set_user_typing(
    state: State<'_, JeffState>,
    is_typing: bool,
) -> Result<CoworkingStatusDto, String> {
    let now = unix_now_seconds().map_err(map_jeff_error)?;
    let allowed = state
        .store
        .get_privacy_typing_activity_enabled()
        .map_err(map_jeff_error)?;
    let mut runtime = state
        .coworking
        .lock()
        .map_err(|_| "failed to lock coworking runtime".to_string())?;
    Ok(runtime.set_user_typing(is_typing && allowed, now))
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
    if !state
        .store
        .get_privacy_proactive_triggers_enabled()
        .map_err(map_jeff_error)?
    {
        let now = unix_now_seconds().map_err(map_jeff_error)?;
        let status = state
            .coworking
            .lock()
            .map_err(|_| "failed to lock coworking runtime".to_string())?
            .status(now);
        return Ok(ProactiveEvaluationDto {
            status,
            decision_event_type: "skip".to_string(),
            decision_reason: "privacy_proactive_triggers_disabled".to_string(),
            nudge: None,
        });
    }

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
    let snapshot_summary = {
        let snap = state.awareness_core.snapshot_immediate();
        let summary = crate::awareness_core::snapshot_summary(&snap);
        if summary.is_empty() {
            None
        } else {
            Some(summary)
        }
    };
    propose_revision_for_artifact(
        &state.store,
        state.embeddings.as_ref(),
        state.reasoning.as_ref(),
        task_id,
        artifact_id,
        selection_or_range,
        &instruction,
        &source,
        snapshot_summary.as_deref(),
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
    // optional: the user-edited text (if the user modified the suggestion before accepting).
    // when provided and significantly different from the accepted content, we update style
    // signals toward the user's edits rather than the raw suggestion.
    user_edited_text: Option<String>,
) -> Result<RevisionApplyResultDto, String> {
    let result = apply_artifact_revision(&state.store, state.embeddings.as_ref(), revision_id)
        .map_err(map_jeff_error)?;

    if user_profile_memory_enabled(state.inner()) {
        // phase 23: update style signals from the accepted text.
        let accepted_text = &result.artifact_content.content;
        if let Some(edited) = user_edited_text {
            if user_model::word_level_diff_ratio(accepted_text, &edited) > 0.30 {
                let _ = user_model::record_revision_rewrite(&state.store, &edited);
            } else {
                let _ = user_model::record_revision_accepted(&state.store, &edited);
            }
        } else {
            let _ = user_model::record_revision_accepted(&state.store, accepted_text);
        }
    }

    Ok(result)
}

#[tauri::command]
pub fn reject_revision(
    state: State<'_, JeffState>,
    revision_id: i64,
) -> Result<RevisionProposalDto, String> {
    reject_artifact_revision(&state.store, revision_id).map_err(map_jeff_error)
}

#[tauri::command]
pub fn generate_revision_alternative(
    state: State<'_, JeffState>,
    task_id: i64,
    revision_id: i64,
) -> Result<RevisionProposalDto, String> {
    let snapshot_summary = {
        let snap = state.awareness_core.snapshot_immediate();
        let summary = crate::awareness_core::snapshot_summary(&snap);
        if summary.is_empty() {
            None
        } else {
            Some(summary)
        }
    };
    generate_revision_alternative_inner(
        &state.store,
        state.reasoning.as_ref(),
        task_id,
        revision_id,
        snapshot_summary.as_deref(),
    )
    .map_err(map_jeff_error)
}

#[tauri::command]
pub fn list_revision_alternatives(
    state: State<'_, JeffState>,
    revision_id: i64,
) -> Result<Vec<RevisionProposalDto>, String> {
    state
        .store
        .list_alternative_revisions(revision_id)
        .map_err(map_jeff_error)
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
    let result = accept_subtask_result_by_id(&state.store, subtask_id).map_err(map_jeff_error)?;
    if user_profile_memory_enabled(state.inner()) {
        // phase 23: record delegation acceptance pattern
        let _ = user_model::record_subtask_accepted(&state.store, &result.execution_type);
    }
    Ok(result)
}

#[tauri::command]
pub fn reject_subtask_result(
    state: State<'_, JeffState>,
    subtask_id: i64,
) -> Result<SubTaskDto, String> {
    let result = reject_subtask_result_by_id(&state.store, subtask_id).map_err(map_jeff_error)?;
    if user_profile_memory_enabled(state.inner()) {
        // phase 23: record delegation rejection pattern
        let _ = user_model::record_subtask_rejected(&state.store, &result.execution_type);
    }
    Ok(result)
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
    let global_enabled = state
        .store
        .get_privacy_clipboard_capture_enabled()
        .map_err(map_jeff_error)?;
    let enabled = state
        .store
        .get_clipboard_capture(task_id)
        .map_err(map_jeff_error)?;

    if global_enabled && enabled {
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
    if !state
        .store
        .get_privacy_workspace_watcher_enabled()
        .map_err(map_jeff_error)?
    {
        crate::watcher::stop_all_except(state.watcher.clone(), None);
        return Ok(WatcherStatusDto {
            task_id,
            is_watching: false,
            watched_path: None,
        });
    }

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
        .filter(|path| !path.is_empty())
        .or_else(|| {
            // no folder has been explicitly configured for this task yet.
            // fall back to the global preferred_workspace_folder set during
            // onboarding so that the first task the user creates automatically
            // watches their chosen folder without a manual watcher start call.
            state
                .store
                .get_preferred_workspace_folder()
                .ok()
                .flatten()
                .filter(|path| !path.trim().is_empty())
        });

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
        match start_watcher_and_persist_folder(state, task_id, candidate.clone()) {
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

    state
        .store
        .set_privacy_workspace_watcher_enabled(true)
        .map_err(map_jeff_error)?;
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
        .set_privacy_workspace_watcher_enabled(false)
        .map_err(map_jeff_error)?;
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

// let the backend determine the correct folder (preferred_workspace_folder fallback,
// then internal task dir) rather than having the frontend prescribe a path.
#[tauri::command]
pub fn ensure_workspace_watcher(
    state: State<'_, crate::state::JeffState>,
    task_id: i64,
) -> Result<WatcherStatusDto, String> {
    ensure_workspace_awareness_for_task(state.inner(), task_id)
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
        .set_privacy_clipboard_capture_enabled(enabled)
        .map_err(map_jeff_error)?;
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
    context_state: State<'_, ContextState>,
    task_id: i64,
) -> Result<ReorientationDto, String> {
    if ambient.is_quiet_mode()
        || !state
            .store
            .get_privacy_proactive_triggers_enabled()
            .map_err(map_jeff_error)?
    {
        return Ok(ReorientationDto {
            task_id,
            summary: String::new(),
            fired_at: String::new(),
        });
    }
    let active_ctx = active_context_string_if_enabled(state.inner(), &context_state);
    let current_snapshot = state.awareness_core.snapshot_immediate();
    let current_snapshot_summary = crate::awareness_core::snapshot_summary(&current_snapshot);
    crate::proactive::generate_reorientation(
        &state.store,
        state.reasoning.as_ref(),
        task_id,
        active_ctx.as_deref(),
        None, // calendar context injected at the command layer when CalendarState is available
        Some(current_snapshot_summary.as_str()).filter(|value| !value.is_empty()),
    )
    .map_err(map_jeff_error)
}

#[tauri::command]
pub fn check_task_drift(
    state: State<'_, JeffState>,
    ambient: State<'_, crate::ambient::AmbientState>,
    context_state: State<'_, ContextState>,
    task_id: i64,
    current_text: String,
) -> Result<DriftFlagDto, String> {
    if ambient.is_quiet_mode()
        || !state
            .store
            .get_privacy_proactive_triggers_enabled()
            .map_err(map_jeff_error)?
    {
        return Ok(DriftFlagDto {
            task_id,
            is_drifting: false,
            flag_reason: String::new(),
            confidence: 0.0,
        });
    }
    let active_ctx = active_context_string_if_enabled(state.inner(), &context_state);
    crate::proactive::evaluate_drift(
        &state.store,
        state.reasoning.as_ref(),
        state.embeddings.as_ref(),
        task_id,
        &current_text,
        active_ctx.as_deref(),
    )
    .map_err(map_jeff_error)
}

#[tauri::command]
pub async fn trigger_speculative_subtask<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, JeffState>,
    ambient: State<'_, crate::ambient::AmbientState>,
    task_id: i64,
) -> Result<Option<SubTaskDto>, String> {
    if ambient.is_quiet_mode()
        || !state
            .store
            .get_privacy_proactive_triggers_enabled()
            .map_err(map_jeff_error)?
    {
        return Ok(None);
    }
    let store = state.store.clone();
    let subtask = crate::proactive::propose_speculative_subtask(
        &state.store,
        state.embeddings.as_ref(),
        std::sync::Arc::clone(&state.reasoning),
        &state.subtasks,
        task_id,
    )
    .map_err(map_jeff_error)?;

    if let Some(subtask) = subtask.as_ref() {
        let description = subtask
            .description
            .trim()
            .strip_suffix('.')
            .unwrap_or_else(|| subtask.description.trim());
        let subject = if description.is_empty() {
            subtask.title.trim()
        } else {
            description
        };
        let message = format!("I started {subject} in the background.");
        crate::proactive::deliver_proactive_as_chat_message(
            &store,
            &app,
            task_id,
            &message,
            "proactive_speculative_subtask",
        )
        .await
        .map_err(map_jeff_error)?;
        let _ = app.emit(
            "proactive://speculative_subtask",
            &serde_json::json!({
                "subtask_id": subtask.subtask_id,
                "task_id": subtask.task_id,
                "title": subtask.title.clone(),
                "description": subtask.description.clone(),
            }),
        );
    }

    Ok(subtask)
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
        .map_err(map_jeff_error)?;
    if user_profile_memory_enabled(state.inner()) {
        // phase 23: down-weight the trigger type so dismissals reduce future frequency
        let _ = user_model::record_trigger_dismissed(&state.store, &trigger_type);
    }
    Ok(())
}

#[tauri::command]
pub fn record_task_focus<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, JeffState>,
    ambient: State<'_, crate::ambient::AmbientState>,
    task_id: i64,
) -> Result<(), String> {
    state
        .store
        .record_task_focus(task_id)
        .map_err(map_jeff_error)?;
    if user_profile_memory_enabled(state.inner()) {
        // phase 23: update work-rhythm profile signal
        let _ = user_model::record_focus_hour(&state.store);
    }
    crate::awareness_core::spawn_awareness_update(
        &app,
        crate::awareness_core::SnapshotTrigger::FocusEvent,
        task_id,
    );
    let _ = workload::check_stale_task_notifications(&state.store, &app, ambient.is_quiet_mode());
    Ok(())
}

#[tauri::command]
pub async fn get_situational_snapshot(
    state: State<'_, JeffState>,
    task_id: i64,
) -> Result<crate::awareness_core::SituationalSnapshot, String> {
    #[cfg(debug_assertions)]
    {
        let snapshot = state.awareness_core.snapshot().await;
        if snapshot.trigger == "initial" {
            let ambient_placeholder = crate::ambient::AmbientState::new();
            return Ok(state
                .awareness_core
                .update(
                    crate::awareness_core::SnapshotTrigger::TimeTick,
                    task_id,
                    state.inner(),
                    &ambient_placeholder,
                )
                .await);
        }
        Ok(snapshot)
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = task_id;
        Err("situational snapshot is available only in debug builds".to_string())
    }
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

    // ensure the internal task workspace exists (used for auto-ingest even if writing elsewhere)
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

    // prefer the user's connected watcher folder as the write destination; fall back to
    // the internal workspace so approval never silently disappears into a hidden directory.
    let write_root_uncanonical: std::path::PathBuf =
        match state.store.get_watched_folder(task_id).ok().flatten() {
            Some(folder) => std::path::PathBuf::from(folder.folder_path),
            None => workspace_uncanonical.clone(),
        };
    fs::create_dir_all(&write_root_uncanonical).map_err(|e| {
        format!(
            "failed to create write root '{}': {e}",
            write_root_uncanonical.display()
        )
    })?;
    let write_root = fs::canonicalize(&write_root_uncanonical).map_err(|e| {
        format!(
            "failed to canonicalize write root '{}': {e}",
            write_root_uncanonical.display()
        )
    })?;
    let dest = write_root.join(raw_path);

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
        if !canonical_parent.starts_with(&write_root) {
            return Err(format!(
                "proposed_path '{}' escapes write root",
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

    // fetch and return audit entry (last insert); attach the resolved absolute path
    // so the frontend can show the user exactly where the file landed.
    let entries = state
        .store
        .list_write_audit_log(task_id, 1)
        .map_err(map_jeff_error)?;
    let mut audit = entries
        .into_iter()
        .next()
        .ok_or_else(|| "audit entry was written but could not be loaded".to_string())?;
    audit.resolved_path = Some(dest.display().to_string());
    Ok(audit)
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

fn build_privacy_center_dashboard(
    state: &JeffState,
    ambient: &ambient::AmbientState,
) -> Result<PrivacyCenterDashboardDto, String> {
    let active_task = state.store.get_active_task().map_err(map_jeff_error)?;
    let active_task_id = active_task.as_ref().map(|task| task.id);

    let (workspace_folder_path, workspace_watched_file_count, workspace_watcher_running) =
        if let Some(task) = active_task.as_ref() {
            let watcher_status = crate::watcher::get_watcher_status(state.watcher.clone(), task.id);
            let watched_folder = state
                .store
                .get_watched_folder(task.id)
                .map_err(map_jeff_error)?
                .map(|folder| folder.folder_path);
            (
                watcher_status
                    .watched_path
                    .or(watched_folder)
                    .or_else(|| Some(task.workspace_path.clone())),
                state
                    .store
                    .count_watched_files(task.id)
                    .map_err(map_jeff_error)?,
                watcher_status.is_watching,
            )
        } else {
            (None, 0, false)
        };

    let clipboard_task_enabled = if let Some(task_id) = active_task_id {
        state
            .store
            .get_clipboard_capture(task_id)
            .map_err(map_jeff_error)?
    } else {
        false
    };
    let clipboard_privacy_enabled = state
        .store
        .get_privacy_clipboard_capture_enabled()
        .map_err(map_jeff_error)?;
    let active_window_context_enabled = state
        .store
        .get_privacy_active_window_context_enabled()
        .map_err(map_jeff_error)?;
    let accessibility_permission_status = if context_observer::is_accessibility_trusted() {
        "granted"
    } else {
        "not granted"
    };
    let proactive_triggers_enabled = state
        .store
        .get_privacy_proactive_triggers_enabled()
        .map_err(map_jeff_error)?
        && !ambient.is_quiet_mode();

    Ok(PrivacyCenterDashboardDto {
        active_task_id,
        active_task_title: active_task.map(|task| task.title),
        workspace_watcher_enabled: state
            .store
            .get_privacy_workspace_watcher_enabled()
            .map_err(map_jeff_error)?,
        workspace_folder_path,
        workspace_watched_file_count,
        workspace_watcher_running,
        clipboard_capture_enabled: clipboard_privacy_enabled && clipboard_task_enabled,
        clipboard_capture_reminder: "Clipboard capture is off by default.".to_string(),
        active_window_context_enabled,
        accessibility_permission_status: accessibility_permission_status.to_string(),
        proactive_triggers_enabled,
        user_profile_memory_enabled: state
            .store
            .get_privacy_user_profile_memory_enabled()
            .map_err(map_jeff_error)?,
        user_profile_signal_count: state
            .store
            .count_user_profile_signals()
            .map_err(map_jeff_error)?,
        calendar_context_enabled: state
            .store
            .get_privacy_calendar_context_enabled()
            .map_err(map_jeff_error)?,
        calendar_permission_status: "not requested".to_string(),
        selection_capture_enabled: state
            .store
            .get_privacy_selection_capture_enabled()
            .map_err(map_jeff_error)?,
        typing_activity_enabled: state
            .store
            .get_privacy_typing_activity_enabled()
            .map_err(map_jeff_error)?,
        tts_voice: state.store.get_tts_voice().map_err(map_jeff_error)?,
        available_tts_voices: crate::voice_naturalness::available_tts_voices(),
    })
}

#[tauri::command]
pub fn get_privacy_center_dashboard(
    state: State<'_, JeffState>,
    ambient: State<'_, ambient::AmbientState>,
) -> Result<PrivacyCenterDashboardDto, String> {
    build_privacy_center_dashboard(state.inner(), &ambient)
}

#[tauri::command]
pub fn set_privacy_surface_enabled(
    state: State<'_, JeffState>,
    ambient: State<'_, ambient::AmbientState>,
    context_state: State<'_, ContextState>,
    surface: String,
    enabled: bool,
) -> Result<PrivacyCenterDashboardDto, String> {
    let surface_key = surface.trim();
    match surface_key {
        "workspace_watcher" => {
            state
                .store
                .set_privacy_workspace_watcher_enabled(enabled)
                .map_err(map_jeff_error)?;
            if enabled {
                let _ = restore_workspace_awareness_for_active_task(state.inner());
            } else {
                crate::watcher::stop_all_except(state.watcher.clone(), None);
            }
        }
        "clipboard_capture" => {
            state
                .store
                .set_privacy_clipboard_capture_enabled(enabled)
                .map_err(map_jeff_error)?;
            if let Some(task) = state.store.get_active_task().map_err(map_jeff_error)? {
                state
                    .store
                    .set_clipboard_capture(task.id, enabled)
                    .map_err(map_jeff_error)?;
                if enabled {
                    sync_clipboard_poll_for_active_task(state.inner(), task.id)?;
                } else {
                    crate::watcher::stop_clipboard_poll(state.watcher.clone(), task.id);
                }
            }
        }
        "active_window_context" => {
            state
                .store
                .set_privacy_active_window_context_enabled(enabled)
                .map_err(map_jeff_error)?;
            if !enabled {
                context_state.update(None);
            }
        }
        "proactive_triggers" => {
            // privacy toggle for proactive triggers: suppress unsolicited initiation
            // (reorientation, drift, speculative subtask, proactive notifications).
            // this is intentionally distinct from quiet mode, which suppresses all
            // output including responses to explicit user messages and tts playback.
            // do NOT touch quiet_mode here.
            state
                .store
                .set_privacy_proactive_triggers_enabled(enabled)
                .map_err(map_jeff_error)?;
            state
                .store
                .set_app_setting("proactive_mode", if enabled { "1" } else { "0" })
                .map_err(map_jeff_error)?;
            let now = unix_now_seconds().map_err(map_jeff_error)?;
            let mut runtime = state
                .coworking
                .lock()
                .map_err(|_| "failed to lock coworking runtime".to_string())?;
            runtime.set_proactive_mode(enabled, now);
        }
        "user_profile_memory" => {
            state
                .store
                .set_privacy_user_profile_memory_enabled(enabled)
                .map_err(map_jeff_error)?;
        }
        "calendar_context" => {
            state
                .store
                .set_privacy_calendar_context_enabled(enabled)
                .map_err(map_jeff_error)?;
        }
        "selection_capture" => {
            state
                .store
                .set_privacy_selection_capture_enabled(enabled)
                .map_err(map_jeff_error)?;
        }
        "typing_activity" => {
            state
                .store
                .set_privacy_typing_activity_enabled(enabled)
                .map_err(map_jeff_error)?;
            if let Ok(now) = unix_now_seconds() {
                let mut runtime = state
                    .coworking
                    .lock()
                    .map_err(|_| "failed to lock coworking runtime".to_string())?;
                let _ = runtime.set_user_typing(false, now);
            }
        }
        _ => return Err(format!("unknown privacy surface '{surface_key}'")),
    }

    build_privacy_center_dashboard(state.inner(), &ambient)
}

#[tauri::command]
pub fn clear_user_profile_memory(
    state: State<'_, JeffState>,
    ambient: State<'_, ambient::AmbientState>,
) -> Result<PrivacyCenterDashboardDto, String> {
    state.store.clear_user_profile().map_err(map_jeff_error)?;
    build_privacy_center_dashboard(state.inner(), &ambient)
}

#[tauri::command]
pub fn get_selection_capture_indicator(
    selection_state: State<'_, SelectionCaptureState>,
) -> Option<SelectionCaptureIndicatorDto> {
    selection_state.current_indicator()
}

#[tauri::command]
pub fn dismiss_selection_capture<R: Runtime>(
    selection_state: State<'_, SelectionCaptureState>,
    app: AppHandle<R>,
) -> Option<SelectionCaptureIndicatorDto> {
    selection_state.dismiss();
    let _ = app.emit(
        crate::selection_capture::EVENT_SELECTION_CLEARED,
        serde_json::json!({}),
    );
    None
}

#[tauri::command]
pub fn get_selection_bridge_status(
    selection_state: State<'_, SelectionCaptureState>,
) -> SelectionBridgeStatusDto {
    selection_state.bridge_status()
}

#[tauri::command]
pub fn capture_browser_selection<R: Runtime>(
    app: AppHandle<R>,
    request: BrowserSelectionCaptureRequestDto,
) -> Result<SelectionCaptureIndicatorDto, String> {
    crate::selection_capture::capture_browser_selection_request(&app, request)
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn set_tts_voice(
    state: State<'_, JeffState>,
    ambient: State<'_, ambient::AmbientState>,
    voice: String,
) -> Result<PrivacyCenterDashboardDto, String> {
    state.store.set_tts_voice(&voice).map_err(map_jeff_error)?;
    build_privacy_center_dashboard(state.inner(), &ambient)
}

#[tauri::command]
pub fn list_proactive_trigger_audit_log(
    state: State<'_, JeffState>,
    task_id: i64,
) -> Result<Vec<ProactiveAuditEntryDto>, String> {
    state
        .store
        .list_proactive_trigger_audit_log(task_id, 100)
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn get_synthesis_log(
    state: State<'_, JeffState>,
    task_id: i64,
) -> Result<Vec<SynthesisLogEntryDto>, String> {
    state
        .store
        .list_synthesis_log(task_id, 100)
        .map_err(map_jeff_error)
}

fn request_cancel_all_subtasks(state: &JeffState) {
    if let Ok(tasks) = state.store.list_tasks() {
        for task in tasks {
            if let Ok(subtasks) = state.store.list_subtasks(task.id) {
                for subtask in subtasks {
                    if matches!(subtask.status.as_str(), "pending" | "running") {
                        let _ = state.subtasks.request_cancel(subtask.subtask_id);
                    }
                }
            }
        }
    }
}

#[tauri::command]
pub fn clear_active_task_data(
    state: State<'_, JeffState>,
    selection_state: State<'_, SelectionCaptureState>,
) -> Result<DataClearResultDto, String> {
    let Some(task) = state.store.get_active_task().map_err(map_jeff_error)? else {
        return Ok(DataClearResultDto {
            cleared: false,
            active_task_id: None,
            message: "No active task to clear.".to_string(),
        });
    };

    if let Ok(subtasks) = state.store.list_subtasks(task.id) {
        for subtask in subtasks {
            if matches!(subtask.status.as_str(), "pending" | "running") {
                let _ = state.subtasks.request_cancel(subtask.subtask_id);
            }
        }
    }
    crate::watcher::stop_watcher(state.watcher.clone(), task.id);
    crate::watcher::stop_clipboard_poll(state.watcher.clone(), task.id);
    selection_state.dismiss();
    state
        .store
        .clear_task_data(task.id)
        .map_err(map_jeff_error)?;

    Ok(DataClearResultDto {
        cleared: true,
        active_task_id: Some(task.id),
        message: "Active task data cleared. The task record was kept.".to_string(),
    })
}

#[tauri::command]
pub fn clear_all_jeff_data(
    state: State<'_, JeffState>,
    ambient: State<'_, ambient::AmbientState>,
    context_state: State<'_, ContextState>,
    selection_state: State<'_, SelectionCaptureState>,
) -> Result<DataClearResultDto, String> {
    request_cancel_all_subtasks(state.inner());
    crate::watcher::stop_all_except(state.watcher.clone(), None);
    context_state.update(None);
    selection_state.dismiss();
    let _ = crate::login_item::set_login_item_enabled(false);
    crate::secrets::delete_openai_api_key().map_err(map_jeff_error)?;
    state.store.clear_all_data().map_err(map_jeff_error)?;

    ambient.set_quiet_mode(false);
    let now = unix_now_seconds().map_err(map_jeff_error)?;
    if let Ok(mut runtime) = state.coworking.lock() {
        runtime.set_proactive_mode(true, now);
    }

    Ok(DataClearResultDto {
        cleared: true,
        active_task_id: None,
        message: "All Jeff data cleared. Jeff is back in first-run state.".to_string(),
    })
}

#[tauri::command]
pub fn start_subtask_chain<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, JeffState>,
    task_id: i64,
    title: String,
    description: String,
    execution_type: String,
    instruction_source: Option<String>,
) -> Result<SubTaskDto, String> {
    let source = instruction_source.unwrap_or_else(|| "text".to_string());

    // phase 23: cross-task collision detection — emit a soft notice if a
    // recently-completed subtask from another task is semantically similar.
    let instruction = format!("{title}\n{description}");
    if let Ok(embedding) = state.embeddings.embed_text(&instruction) {
        if let Ok(recent) = state.store.get_recent_cross_task_subtasks(task_id, 30) {
            for (task_title, hist_instruction) in recent {
                if let Ok(hist_embedding) = state.embeddings.embed_text(&hist_instruction) {
                    let score = cosine_similarity(&embedding, &hist_embedding);
                    if score > 0.8 {
                        let _ = app.emit(
                            "subtask://collision-detected",
                            serde_json::json!({
                                "matching_task_title": task_title,
                                "similarity_score": score
                            }),
                        );
                        break; // emit at most one notice
                    }
                }
            }
        }
    }

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

// phase 20: active window context commands ------------------------------------

/// returns the current frontmost-app context from ContextState (in-memory, polled
/// every 3 s). returns null when permission is not granted or no context available.
#[tauri::command]
pub fn get_active_window_context(
    state: State<'_, JeffState>,
    context_state: State<'_, ContextState>,
) -> Option<ActiveWindowContextDto> {
    if !state
        .store
        .get_privacy_active_window_context_enabled()
        .unwrap_or(true)
    {
        return None;
    }
    context_state.current().map(|ctx| ActiveWindowContextDto {
        app_name: ctx.app_name,
        document_title: ctx.document_title,
        captured_at: ctx.captured_at,
    })
}

/// returns true if the macOS Accessibility permission has been granted.
#[tauri::command]
pub fn get_accessibility_permission_status() -> bool {
    context_observer::is_accessibility_trusted()
}

/// surfaces the macOS Accessibility permission dialog. no-op on non-macos.
#[tauri::command]
pub fn request_accessibility_permission() {
    context_observer::request_accessibility_permission();
}

// phase 19: session persistence commands --------------------------------------

/// returns the SMAppService-backed launch-at-login state.
/// reads OS state directly and falls back to the persisted store value on error.
/// does NOT write to the store — reconciliation happens only at startup and on
/// explicit set, so this function is safe to call on every render.
#[tauri::command]
pub fn get_launch_at_login(state: State<'_, JeffState>) -> Result<bool, String> {
    crate::login_item::login_item_enabled_or_pending()
        .or_else(|_| state.store.get_launch_at_login().map_err(map_jeff_error))
}

/// syncs the macOS SMAppService main-app login item, then persists only after
/// the OS state reaches the requested state or requires user approval.
#[tauri::command]
pub fn set_launch_at_login(state: State<'_, JeffState>, enabled: bool) -> Result<bool, String> {
    let status = crate::login_item::set_login_item_enabled(enabled)?;
    let persisted = if enabled {
        status.is_enabled_or_pending()
    } else {
        false
    };
    state
        .store
        .set_launch_at_login(persisted)
        .map_err(map_jeff_error)?;
    Ok(persisted)
}

/// returns the session state that was restored on startup, for the frontend to
/// read and display. actual restoration (overlay mode, quiet mode, watcher) is
/// performed by the backend in main.rs setup before any window is shown. this
/// command is a pure read — it does not perform restoration itself.
#[tauri::command]
pub fn get_session_restore_state(state: State<'_, JeffState>) -> Result<SessionRestoreDto, String> {
    let active_task = state.store.get_active_task().map_err(map_jeff_error)?;
    let overlay_expanded = state.store.get_overlay_expanded().map_err(map_jeff_error)?;
    let quiet_mode = state.store.get_quiet_mode().map_err(map_jeff_error)?;

    Ok(SessionRestoreDto {
        had_active_task: active_task.is_some(),
        overlay_expanded,
        quiet_mode,
    })
}

// phase 23: user profile commands ----------------------------------------------

#[tauri::command]
pub fn get_user_profile_signals(
    state: State<'_, JeffState>,
) -> Result<Vec<UserProfileSignalDto>, String> {
    if !user_profile_memory_enabled(state.inner()) {
        return Ok(Vec::new());
    }
    let signals = user_model::get_readable_signals(&state.store).map_err(map_jeff_error)?;
    Ok(signals
        .into_iter()
        .map(|s| UserProfileSignalDto {
            key: s.key,
            label: s.label,
            value: s.value,
            updated_at: s.updated_at,
        })
        .collect())
}

#[tauri::command]
pub fn add_quality_rubric(
    state: State<'_, JeffState>,
    text: String,
) -> Result<Vec<UserProfileSignalDto>, String> {
    if !user_profile_memory_enabled(state.inner()) {
        return Err("user profile memory is off in Privacy Center".to_string());
    }
    user_model::add_quality_rubric(&state.store, text.trim()).map_err(map_jeff_error)?;
    get_user_profile_signals(state)
}

#[tauri::command]
pub fn delete_quality_rubric(
    state: State<'_, JeffState>,
    key: String,
) -> Result<Vec<UserProfileSignalDto>, String> {
    state
        .store
        .delete_profile_signal(&key)
        .map_err(map_jeff_error)?;
    get_user_profile_signals(state)
}

#[tauri::command]
pub fn delete_user_profile_signal(
    state: State<'_, JeffState>,
    key: String,
) -> Result<Vec<UserProfileSignalDto>, String> {
    state
        .store
        .delete_profile_signal(&key)
        .map_err(map_jeff_error)?;
    get_user_profile_signals(state)
}

// phase 23: workload commands --------------------------------------------------

#[tauri::command]
pub fn get_workload_summary(state: State<'_, JeffState>) -> Result<WorkloadSummaryDto, String> {
    workload::compute_workload_summary(&state.store).map_err(map_jeff_error)
}

#[tauri::command]
pub fn switch_active_task_from_companion<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, JeffState>,
    task_id: i64,
) -> Result<TaskDto, String> {
    // stop current watcher on any task, then switch active task and start watcher
    let current = state.store.get_active_task().map_err(map_jeff_error)?;
    if let Some(ref current_task) = current {
        crate::watcher::stop_watcher(state.watcher.clone(), current_task.id);
        crate::watcher::stop_clipboard_poll(state.watcher.clone(), current_task.id);
    }

    let new_task = state
        .store
        .set_active_task(task_id)
        .map_err(map_jeff_error)?;

    // restart watcher on the correct folder for this task (preferred_workspace_folder
    // fallback, then internal task dir) — same logic as set_active_task.
    if let Err(err) = ensure_workspace_awareness_for_task(state.inner(), new_task.id) {
        eprintln!(
            "[jeff watcher] failed to sync watcher after companion task switch to {}: {err}",
            new_task.id
        );
    }

    let _ = app.emit(
        "task://active-changed",
        serde_json::json!({ "task_id": new_task.id }),
    );

    Ok(new_task)
}

// phase 23: calendar commands --------------------------------------------------

#[tauri::command]
pub fn request_calendar_permission() -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        crate::calendar::request_calendar_permission().map_err(map_jeff_error)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(false)
    }
}

#[tauri::command]
pub fn get_calendar_permission_status() -> String {
    #[cfg(target_os = "macos")]
    {
        crate::calendar::get_calendar_permission_status()
    }
    #[cfg(not(target_os = "macos"))]
    {
        "not_determined".to_string()
    }
}

#[tauri::command]
pub fn get_calendar_next_event(
    state: State<'_, JeffState>,
    calendar_state: State<'_, crate::state::CalendarState>,
) -> Result<Option<CalendarEventDto>, String> {
    let enabled = state
        .store
        .get_privacy_calendar_context_enabled()
        .unwrap_or(false);
    if !enabled {
        return Ok(None);
    }
    crate::calendar::get_cached_next_event(&calendar_state).map_err(map_jeff_error)
}

// phase 23: live app action commands ------------------------------------------

#[tauri::command]
pub fn approve_live_edit<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, JeffState>,
    receipt_id: i64,
) -> Result<LiveEditReceiptDto, String> {
    let receipt = state
        .store
        .update_live_edit_status(receipt_id, "approved")
        .map_err(map_jeff_error)?;
    let _ = app.emit(
        crate::selection_capture::EVENT_LIVE_ACTION_APPROVED,
        serde_json::json!({ "receipt_id": receipt_id }),
    );
    Ok(receipt)
}

#[tauri::command]
pub fn reject_live_edit(
    state: State<'_, JeffState>,
    receipt_id: i64,
) -> Result<LiveEditReceiptDto, String> {
    state
        .store
        .update_live_edit_status(receipt_id, "rejected")
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn list_live_edit_receipts(
    state: State<'_, JeffState>,
    task_id: i64,
) -> Result<Vec<LiveEditReceiptDto>, String> {
    state
        .store
        .list_live_edit_receipts(Some(task_id))
        .map_err(map_jeff_error)
}

#[tauri::command]
pub fn get_pending_live_edits(
    state: State<'_, JeffState>,
) -> Result<Vec<PendingLiveEditDto>, String> {
    state
        .store
        .get_unresolved_live_edits()
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
