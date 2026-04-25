use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskDto {
    pub id: i64,
    pub title: String,
    pub slug: String,
    pub workspace_path: String,
    pub created_at: String,
    pub updated_at: String,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceInfoDto {
    pub task_id: i64,
    pub slug: String,
    pub workspace_path: String,
    pub exists_on_disk: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskSummaryDto {
    pub task_id: i64,
    pub summary_text: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenResourceDto {
    pub id: i64,
    pub task_id: i64,
    pub resource_type: String,
    pub resource_path_or_url: String,
    pub label: String,
    pub position_index: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactDto {
    pub id: i64,
    pub task_id: i64,
    pub file_name: String,
    pub file_extension: String,
    pub original_path: String,
    pub stored_path: String,
    pub created_at: String,
    pub updated_at: String,
    pub chunk_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetrievedChunkDto {
    pub chunk_id: i64,
    pub task_id: i64,
    pub artifact_id: i64,
    pub artifact_file_name: String,
    pub artifact_stored_path: String,
    pub chunk_text: String,
    pub position_index: i64,
    pub similarity_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextArtifactDto {
    pub artifact_id: i64,
    pub file_name: String,
    pub stored_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskContextPackDto {
    pub task_summary: String,
    pub active_task_id: i64,
    pub recent_transcript: Vec<String>,
    pub active_artifact: Option<ContextArtifactDto>,
    pub retrieved_chunks: Vec<RetrievedChunkDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessageDto {
    pub id: i64,
    pub task_id: i64,
    pub session_id: Option<i64>,
    pub role: String,
    pub message_source: String,
    pub message_kind: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SendMessageResponseDto {
    pub assistant_response: String,
    pub retrieved_chunks: Vec<RetrievedChunkDto>,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TranscriptionResultDto {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpeechSynthesisDto {
    pub audio_base64: String,
    pub mime_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CoworkingStatusDto {
    pub state: String,
    pub proactive_mode: bool,
    pub user_typing: bool,
    pub user_speaking: bool,
    pub session_mode: String,
    pub pause_threshold_seconds: u64,
    pub nudge_cooldown_seconds: u64,
    pub interruption_suppression_seconds: u64,
    pub low_confidence_suppression_seconds: u64,
    pub cooldown_remaining_seconds: u64,
    pub last_decision_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProactiveNudgeDto {
    pub message: String,
    pub retrieved_chunks: Vec<RetrievedChunkDto>,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProactiveEvaluationDto {
    pub status: CoworkingStatusDto,
    pub decision_event_type: String,
    pub decision_reason: String,
    pub nudge: Option<ProactiveNudgeDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactContentDto {
    pub artifact_id: i64,
    pub task_id: i64,
    pub file_name: String,
    pub file_extension: String,
    pub stored_path: String,
    pub content: String,
    pub is_editable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RevisionTargetDto {
    pub start_offset: Option<i64>,
    pub end_offset: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RevisionProposalDto {
    pub revision_id: i64,
    pub task_id: i64,
    pub artifact_id: i64,
    pub target_start_offset: i64,
    pub target_end_offset: i64,
    pub target_description: String,
    pub original_text: String,
    pub proposed_text: String,
    pub instruction_text: String,
    pub instruction_source: String,
    pub rationale: Option<String>,
    pub grounding_notes: Option<String>,
    pub retrieval_confidence: f32,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactVersionDto {
    pub version_id: i64,
    pub task_id: i64,
    pub artifact_id: i64,
    pub revision_id: Option<i64>,
    pub version_reason: String,
    pub content_preview: String,
    pub content_length: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RevisionProposalResultDto {
    pub proposal: RevisionProposalDto,
    pub retrieved_chunks: Vec<RetrievedChunkDto>,
    pub active_artifact_id: i64,
    pub used_start_offset: i64,
    pub used_end_offset: i64,
    pub selection_source: String,
    pub confidence: f32,
    pub grounding_notes: String,
    pub context_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RevisionApplyResultDto {
    pub revision: RevisionProposalDto,
    pub artifact_content: ArtifactContentDto,
    pub version_snapshot: ArtifactVersionDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubTaskDto {
    pub subtask_id: i64,
    pub task_id: i64,
    pub title: String,
    pub description: String,
    pub execution_type: String,
    pub status: String,
    pub result_review_status: String,
    pub created_at: String,
    pub updated_at: String,
    pub result_summary: Option<String>,
    pub result_payload: Option<String>,
    pub instruction_source: String,
    pub parent_context_snapshot: String,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubTaskSuggestionDto {
    pub task_id: i64,
    pub title: String,
    pub description: String,
    pub execution_type: String,
    pub instruction_source: String,
    pub reason: String,
    pub parent_context_snapshot: String,
    pub retrieved_chunks: Vec<RetrievedChunkDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionModeStateDto {
    pub task_id: i64,
    pub current_mode: String,
    pub mode_reason: String,
    pub waiting_on_user_decision: bool,
    pub last_engine_decision: String,
    pub active_artifact_id: Option<i64>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SuggestionDto {
    pub suggestion_id: i64,
    pub task_id: i64,
    pub title: String,
    pub description: String,
    pub suggestion_type: String,
    pub source_reason: String,
    pub status: String,
    pub suggestion_key: String,
    pub linked_context: Option<String>,
    pub linked_subtask_type: Option<String>,
    pub linked_revision_intent: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SuggestionEvaluationDto {
    pub mode_state: SessionModeStateDto,
    pub suggestions: Vec<SuggestionDto>,
    pub generated_suggestions: Vec<SuggestionDto>,
    pub decision_reason: String,
    pub no_op: bool,
    pub evidence_score: f32,
    pub active_artifact_id: Option<i64>,
    pub suppression_state: String,
    pub retrieved_chunks: Vec<RetrievedChunkDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SuggestionAcceptanceDto {
    pub suggestion: SuggestionDto,
    pub action_type: String,
    pub followup_message: Option<String>,
    pub revision_result: Option<RevisionProposalResultDto>,
    pub subtask: Option<SubTaskDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventLogEntryDto {
    pub id: i64,
    pub task_id: i64,
    pub event_type: String,
    pub payload_json: String,
    pub created_at: String,
}

// phase 13: workspace awareness

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WatchedFolderDto {
    pub task_id: i64,
    pub folder_path: String,
    pub is_active: bool,
    pub ignore_rules_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WatchedFileRegistryEntry {
    pub id: i64,
    pub task_id: i64,
    pub canonical_path: String,
    pub artifact_id: Option<i64>,
    pub last_modified_at: String,
    pub ingested_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecentlyLearnedItemDto {
    pub id: i64,
    pub task_id: i64,
    pub source: String,
    pub display_label: String,
    pub preview_text: String,
    pub ingested_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WatcherStatusDto {
    pub task_id: i64,
    pub is_watching: bool,
    pub watched_path: Option<String>,
}

// phase 14: intent classification

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum IntentLabel {
    Answer,
    Revision,
    Subtask,
    Suggestion,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct IntentSlotsDto {
    pub target_description: Option<String>,
    pub instruction: Option<String>,
    pub draft_type: Option<String>,
    pub topic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IntentClassificationDto {
    pub intent: IntentLabel,
    pub confidence: f32,
    pub slots: IntentSlotsDto,
}

// phase 15: proactive initiation

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReorientationDto {
    pub task_id: i64,
    pub summary: String,
    pub fired_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DriftFlagDto {
    pub task_id: i64,
    pub is_drifting: bool,
    pub flag_reason: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(dead_code)]
pub struct ProactiveTriggerDto {
    pub task_id: i64,
    pub trigger_type: String,
    pub fired: bool,
    pub suppressed_reason: Option<String>,
}

// phase 16: richer parallel work

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubTaskStepDto {
    pub id: i64,
    pub subtask_id: i64,
    pub step_index: i64,
    pub step_type: String,
    pub status: String,
    pub description: String,
    pub result_summary: Option<String>,
    pub result_payload: Option<String>,
    pub error_message: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileWriteProposalDto {
    pub id: i64,
    pub subtask_id: i64,
    pub step_id: Option<i64>,
    pub task_id: i64,
    pub proposed_path: String,
    pub proposed_content: String,
    pub status: String,
    pub proposed_at: String,
    pub resolved_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WriteAuditEntryDto {
    pub id: i64,
    pub task_id: i64,
    pub subtask_id: i64,
    pub proposal_id: i64,
    pub action: String,
    pub proposed_path: String,
    pub resolved_at: String,
}

// phase 18: onboarding and secure key setup

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OnboardingStatusDto {
    pub onboarding_complete: bool,
    pub has_stored_api_key: bool,
    pub api_key_source: String,
    pub preferred_workspace_folder: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiKeyValidationDto {
    pub is_valid: bool,
    pub message: String,
}

// phase 19: session restore

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionRestoreDto {
    pub had_active_task: bool,
    pub overlay_expanded: bool,
    pub quiet_mode: bool,
}

// phase 20: active window context DTO for frontend serialization.
// i64 instead of f64 for captured_at so JSON serialization is exact.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveWindowContextDto {
    pub app_name: String,
    pub document_title: String,
    pub captured_at: i64,
}

// phase 21: privacy and trust control center

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrivacyCenterDashboardDto {
    pub active_task_id: Option<i64>,
    pub active_task_title: Option<String>,
    pub workspace_watcher_enabled: bool,
    pub workspace_folder_path: Option<String>,
    pub workspace_watched_file_count: i64,
    pub workspace_watcher_running: bool,
    pub clipboard_capture_enabled: bool,
    pub clipboard_capture_reminder: String,
    pub active_window_context_enabled: bool,
    pub accessibility_permission_status: String,
    pub proactive_triggers_enabled: bool,
    pub user_profile_memory_enabled: bool,
    pub user_profile_signal_count: i64,
    pub calendar_context_enabled: bool,
    pub calendar_permission_status: String,
    pub selection_capture_enabled: bool,
    pub typing_activity_enabled: bool,
    pub tts_voice: String,
    pub available_tts_voices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProactiveAuditEntryDto {
    pub id: i64,
    pub task_id: i64,
    pub trigger_type: String,
    pub fired_at: String,
    pub suppressed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DataClearResultDto {
    pub cleared: bool,
    pub active_task_id: Option<i64>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SelectionCaptureStatus {
    Captured,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelectionCaptureIndicatorDto {
    pub status: SelectionCaptureStatus,
    pub app_name: String,
    pub document_title: Option<String>,
    pub captured_at: i64,
    pub word_count: usize,
    pub source_type: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserSelectionCaptureRequestDto {
    pub token: String,
    pub text: String,
    pub app_name: String,
    pub document_title: Option<String>,
    pub source_url: Option<String>,
    pub captured_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelectionBridgeStatusDto {
    pub enabled: bool,
    pub port: u16,
    pub token: String,
}

// phase 23 DTOs

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserProfileSignalDto {
    pub key: String,
    pub label: String,
    pub value: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkloadTaskDto {
    pub id: i64,
    pub title: String,
    pub last_focused_at: Option<String>,
    pub days_since_focus: Option<i64>,
    pub pending_item_count: i64,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkloadSummaryDto {
    pub active_tasks: Vec<WorkloadTaskDto>,
    pub stale_tasks: Vec<WorkloadTaskDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalendarEventDto {
    pub title: String,
    pub starts_at: String,
    pub minutes_until: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LiveEditReceiptDto {
    pub id: i64,
    pub editor_surface: String,
    pub document_title: String,
    pub before_hash: String,
    pub after_hash: String,
    pub timestamp: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingLiveEditDto {
    pub receipt_id: i64,
    pub editor_surface: String,
    pub document_title: String,
    pub before_text: String,
    pub after_text: String,
    pub timestamp: String,
}
