import { invoke } from "@tauri-apps/api/core";

export interface TaskDto {
  id: number;
  title: string;
  slug: string;
  workspace_path: string;
  created_at: string;
  updated_at: string;
  is_active: boolean;
}

export interface WorkspaceInfoDto {
  task_id: number;
  slug: string;
  workspace_path: string;
  exists_on_disk: boolean;
}

export interface TaskSummaryDto {
  task_id: number;
  summary_text: string;
  updated_at: string;
}

export interface OpenResourceDto {
  id: number;
  task_id: number;
  resource_type: string;
  resource_path_or_url: string;
  label: string;
  position_index: number;
}

export interface ArtifactDto {
  id: number;
  task_id: number;
  file_name: string;
  file_extension: string;
  original_path: string;
  stored_path: string;
  created_at: string;
  updated_at: string;
  chunk_count: number;
}

export interface RetrievedChunkDto {
  chunk_id: number;
  task_id: number;
  artifact_id: number;
  artifact_file_name: string;
  artifact_stored_path: string;
  chunk_text: string;
  position_index: number;
  similarity_score: number;
}

export interface ContextArtifactDto {
  artifact_id: number;
  file_name: string;
  stored_path: string;
}

export interface TaskContextPackDto {
  task_summary: string;
  active_task_id: number;
  recent_transcript: string[];
  active_artifact: ContextArtifactDto | null;
  retrieved_chunks: RetrievedChunkDto[];
}

export interface ChatMessageDto {
  id: number;
  task_id: number;
  session_id: number | null;
  role: "user" | "assistant" | string;
  message_source: "text" | "voice" | "assistant" | string;
  message_kind:
    | "user_direct_question"
    | "user_statement"
    | "assistant_answer"
    | "assistant_nudge"
    | "assistant_revision_proposal"
    | "assistant_revision_status"
    | "system_status_event"
    | string;
  content: string;
  created_at: string;
}

export interface SendMessageResponseDto {
  assistant_response: string;
  retrieved_chunks: RetrievedChunkDto[];
  cancelled: boolean;
}

export interface TranscriptionResultDto {
  text: string;
}

export interface SpeechSynthesisDto {
  audio_base64: string;
  mime_type: string;
}

export interface CoworkingStatusDto {
  state:
    | "idle"
    | "listening"
    | "thinking"
    | "speaking"
    | "silent_observing"
    | "awaiting_user"
    | "suppressed"
    | string;
  proactive_mode: boolean;
  user_typing: boolean;
  user_speaking: boolean;
  session_mode: "discussion" | "quiet" | string;
  pause_threshold_seconds: number;
  nudge_cooldown_seconds: number;
  interruption_suppression_seconds: number;
  low_confidence_suppression_seconds: number;
  cooldown_remaining_seconds: number;
  last_decision_reason: string;
}

export interface ProactiveNudgeDto {
  message: string;
  retrieved_chunks: RetrievedChunkDto[];
  confidence: number;
}

export interface ProactiveEvaluationDto {
  status: CoworkingStatusDto;
  decision_event_type: "assistant_nudge" | "system_status_event" | string;
  decision_reason: string;
  nudge: ProactiveNudgeDto | null;
}

export interface ArtifactContentDto {
  artifact_id: number;
  task_id: number;
  file_name: string;
  file_extension: string;
  stored_path: string;
  content: string;
  is_editable: boolean;
}

export interface RevisionTargetDto {
  start_offset?: number | null;
  end_offset?: number | null;
}

export interface RevisionProposalDto {
  revision_id: number;
  task_id: number;
  artifact_id: number;
  target_start_offset: number;
  target_end_offset: number;
  target_description: string;
  original_text: string;
  proposed_text: string;
  instruction_text: string;
  instruction_source: "typed" | "voice" | string;
  rationale: string | null;
  grounding_notes: string | null;
  retrieval_confidence: number;
  status: "pending" | "accepted" | "rejected" | string;
  created_at: string;
  updated_at: string;
}

export interface ArtifactVersionDto {
  version_id: number;
  task_id: number;
  artifact_id: number;
  revision_id: number | null;
  version_reason: string;
  content_preview: string;
  content_length: number;
  created_at: string;
}

export interface RevisionProposalResultDto {
  proposal: RevisionProposalDto;
  retrieved_chunks: RetrievedChunkDto[];
  active_artifact_id: number;
  used_start_offset: number;
  used_end_offset: number;
  selection_source: string;
  confidence: number;
  grounding_notes: string;
  context_source: string;
}

export interface RevisionApplyResultDto {
  revision: RevisionProposalDto;
  artifact_content: ArtifactContentDto;
  version_snapshot: ArtifactVersionDto;
}

export interface SubTaskDto {
  subtask_id: number;
  task_id: number;
  title: string;
  description: string;
  execution_type:
    | "draft_generation"
    | "expansion"
    | "synthesis"
    | "targeted_research_synthesis"
    | string;
  status: "pending" | "running" | "completed" | "failed" | "cancelled" | string;
  result_review_status: "unreviewed" | "accepted" | "rejected" | "converted" | string;
  created_at: string;
  updated_at: string;
  result_summary: string | null;
  result_payload: string | null;
  instruction_source: "text" | "voice" | "system" | string;
  parent_context_snapshot: string;
  error_message: string | null;
}

export interface SubTaskSuggestionDto {
  task_id: number;
  title: string;
  description: string;
  execution_type:
    | "draft_generation"
    | "expansion"
    | "synthesis"
    | "targeted_research_synthesis"
    | string;
  instruction_source: "text" | "voice" | "system" | string;
  reason: string;
  parent_context_snapshot: string;
  retrieved_chunks: RetrievedChunkDto[];
}

export interface SessionModeStateDto {
  task_id: number;
  current_mode:
    | "brainstorming"
    | "outlining"
    | "writing"
    | "revising"
    | "evidence_gathering"
    | "stuck"
    | "quiet_observing"
    | string;
  mode_reason: string;
  waiting_on_user_decision: boolean;
  last_engine_decision: string;
  active_artifact_id: number | null;
  updated_at: string;
}

export interface SuggestionDto {
  suggestion_id: number;
  task_id: number;
  title: string;
  description: string;
  suggestion_type:
    | "ask_followup"
    | "propose_revision"
    | "propose_subtask"
    | "highlight_gap"
    | "connect_to_source"
    | "tighten_argument"
    | string;
  source_reason: string;
  status: "pending" | "accepted" | "dismissed" | "expired" | string;
  suggestion_key: string;
  linked_context: string | null;
  linked_subtask_type: string | null;
  linked_revision_intent: string | null;
  created_at: string;
  updated_at: string;
}

export interface SuggestionEvaluationDto {
  mode_state: SessionModeStateDto;
  suggestions: SuggestionDto[];
  generated_suggestions: SuggestionDto[];
  decision_reason: string;
  no_op: boolean;
  evidence_score: number;
  active_artifact_id: number | null;
  suppression_state: string;
  retrieved_chunks: RetrievedChunkDto[];
}

export interface SuggestionAcceptanceDto {
  suggestion: SuggestionDto;
  action_type: string;
  followup_message: string | null;
  revision_result: RevisionProposalResultDto | null;
  subtask: SubTaskDto | null;
}

export interface EventLogEntryDto {
  id: number;
  task_id: number;
  event_type: string;
  payload_json: string;
  created_at: string;
}

// phase 18: onboarding + secure key management

export interface OnboardingStatusDto {
  onboarding_complete: boolean;
  has_stored_api_key: boolean;
  api_key_source: "keychain" | "env" | "none" | string;
  preferred_workspace_folder: string | null;
}

export interface ApiKeyValidationDto {
  is_valid: boolean;
  message: string;
}

export async function getOnboardingStatus(): Promise<OnboardingStatusDto> {
  return invoke<OnboardingStatusDto>("get_onboarding_status");
}

export async function validateOpenAiApiKey(
  apiKey: string
): Promise<ApiKeyValidationDto> {
  return invoke<ApiKeyValidationDto>("validate_openai_api_key", { apiKey });
}

export async function storeOpenAiApiKey(apiKey: string): Promise<void> {
  return invoke<void>("store_openai_api_key", { apiKey });
}

export async function deleteOpenAiApiKey(): Promise<void> {
  return invoke<void>("delete_openai_api_key");
}

export async function completeOnboarding(): Promise<void> {
  return invoke<void>("complete_onboarding");
}

export async function setPreferredWorkspaceFolder(
  folderPath: string
): Promise<void> {
  return invoke<void>("set_preferred_workspace_folder", { folderPath });
}

export async function clearPreferredWorkspaceFolder(): Promise<void> {
  return invoke<void>("clear_preferred_workspace_folder");
}

export async function createTask(title: string): Promise<TaskDto> {
  return invoke<TaskDto>("create_task", { title });
}

export async function listTasks(): Promise<TaskDto[]> {
  return invoke<TaskDto[]>("list_tasks");
}

export async function getActiveTask(): Promise<TaskDto | null> {
  return invoke<TaskDto | null>("get_active_task");
}

export async function setActiveTask(taskId: number): Promise<TaskDto> {
  return invoke<TaskDto>("set_active_task", { taskId });
}

export async function getTaskWorkspace(taskId: number): Promise<WorkspaceInfoDto> {
  return invoke<WorkspaceInfoDto>("get_task_workspace", { taskId });
}

export async function getTaskSummary(taskId: number): Promise<TaskSummaryDto> {
  return invoke<TaskSummaryDto>("get_task_summary", { taskId });
}

export async function listOpenResources(taskId: number): Promise<OpenResourceDto[]> {
  return invoke<OpenResourceDto[]>("list_open_resources", { taskId });
}

export async function importArtifact(taskId: number, filePath: string): Promise<ArtifactDto> {
  return invoke<ArtifactDto>("import_artifact", { taskId, filePath });
}

export async function listArtifacts(taskId: number): Promise<ArtifactDto[]> {
  return invoke<ArtifactDto[]>("list_artifacts", { taskId });
}

export async function retrieveContext(taskId: number, query: string): Promise<RetrievedChunkDto[]> {
  return invoke<RetrievedChunkDto[]>("retrieve_context", { taskId, query });
}

export async function buildContextPack(taskId: number, query: string): Promise<TaskContextPackDto> {
  return invoke<TaskContextPackDto>("build_context_pack", { taskId, query });
}

export async function listMessages(taskId: number): Promise<ChatMessageDto[]> {
  return invoke<ChatMessageDto[]>("list_messages", { taskId });
}

export async function sendMessage(
  taskId: number,
  message: string,
  source: "text" | "voice"
): Promise<SendMessageResponseDto> {
  return invoke<SendMessageResponseDto>("send_message", { taskId, message, source });
}

export async function cancelInteraction(): Promise<number> {
  return invoke<number>("cancel_interaction");
}

// phase 12: streaming send. returns a turn_id the caller uses to correlate
// stream:// events (see streamClient.ts).
export async function sendMessageStreaming(
  taskId: number,
  message: string,
  source: "text" | "voice"
): Promise<string> {
  return invoke<string>("send_message_streaming", { taskId, message, source });
}

// cancels an active streaming turn. safe to call on already-completed turns.
export async function cancelStreamingTurn(
  turnId: string,
  reason?: "user_barge_in" | "jeff_barge_in" | "explicit" | "error"
): Promise<boolean> {
  return invoke<boolean>("cancel_streaming_turn", { turnId, reason });
}

export async function transcribeAudio(audioBase64: string, mimeType: string): Promise<TranscriptionResultDto> {
  return invoke<TranscriptionResultDto>("transcribe_audio", { audioBase64, mimeType });
}

export async function synthesizeSpeech(text: string): Promise<SpeechSynthesisDto> {
  return invoke<SpeechSynthesisDto>("synthesize_speech", { text });
}

export async function getCoworkingStatus(): Promise<CoworkingStatusDto> {
  return invoke<CoworkingStatusDto>("get_coworking_status");
}

export async function setProactiveMode(enabled: boolean): Promise<CoworkingStatusDto> {
  return invoke<CoworkingStatusDto>("set_proactive_mode", { enabled });
}

export async function setUserTyping(isTyping: boolean): Promise<CoworkingStatusDto> {
  return invoke<CoworkingStatusDto>("set_user_typing", { isTyping });
}

export async function setUserSpeaking(isSpeaking: boolean): Promise<CoworkingStatusDto> {
  return invoke<CoworkingStatusDto>("set_user_speaking", { isSpeaking });
}

export async function setAssistantSpeaking(isSpeaking: boolean): Promise<CoworkingStatusDto> {
  return invoke<CoworkingStatusDto>("set_assistant_speaking", { isSpeaking });
}

export async function evaluateProactiveNudge(taskId: number): Promise<ProactiveEvaluationDto> {
  return invoke<ProactiveEvaluationDto>("evaluate_proactive_nudge", { taskId });
}

export async function getArtifactContent(artifactId: number): Promise<ArtifactContentDto> {
  return invoke<ArtifactContentDto>("get_artifact_content", { artifactId });
}

export async function proposeArtifactRevision(
  taskId: number,
  artifactId: number,
  selectionOrRange: RevisionTargetDto | null,
  instruction: string,
  instructionSource: "typed" | "voice"
): Promise<RevisionProposalResultDto> {
  return invoke<RevisionProposalResultDto>("propose_artifact_revision", {
    taskId,
    artifactId,
    selectionOrRange,
    instruction,
    instructionSource
  });
}

export async function listPendingRevisions(
  taskId: number,
  artifactId: number
): Promise<RevisionProposalDto[]> {
  return invoke<RevisionProposalDto[]>("list_pending_revisions", { taskId, artifactId });
}

export async function listTaskPendingRevisions(taskId: number): Promise<RevisionProposalDto[]> {
  return invoke<RevisionProposalDto[]>("list_task_pending_revisions", { taskId });
}

export async function applyRevision(revisionId: number): Promise<RevisionApplyResultDto> {
  return invoke<RevisionApplyResultDto>("apply_revision", { revisionId });
}

export async function rejectRevision(revisionId: number): Promise<RevisionProposalDto> {
  return invoke<RevisionProposalDto>("reject_revision", { revisionId });
}

export async function listArtifactVersions(artifactId: number): Promise<ArtifactVersionDto[]> {
  return invoke<ArtifactVersionDto[]>("list_artifact_versions", { artifactId });
}

export async function revertArtifactToVersion(versionId: number): Promise<ArtifactContentDto> {
  return invoke<ArtifactContentDto>("revert_artifact_to_version", { versionId });
}

export async function createSubtask(
  taskId: number,
  title: string,
  description: string,
  executionType: "draft_generation" | "expansion" | "synthesis" | "targeted_research_synthesis",
  instructionSource: "text" | "voice" | "system"
): Promise<SubTaskDto> {
  return invoke<SubTaskDto>("create_subtask", {
    taskId,
    title,
    description,
    executionType,
    instructionSource
  });
}

export async function listSubtasks(taskId: number): Promise<SubTaskDto[]> {
  return invoke<SubTaskDto[]>("list_subtasks", { taskId });
}

export async function cancelSubtask(subtaskId: number): Promise<SubTaskDto> {
  return invoke<SubTaskDto>("cancel_subtask", { subtaskId });
}

export async function acceptSubtaskResult(subtaskId: number): Promise<SubTaskDto> {
  return invoke<SubTaskDto>("accept_subtask_result", { subtaskId });
}

export async function rejectSubtaskResult(subtaskId: number): Promise<SubTaskDto> {
  return invoke<SubTaskDto>("reject_subtask_result", { subtaskId });
}

export async function suggestSubtask(taskId: number): Promise<SubTaskSuggestionDto | null> {
  return invoke<SubTaskSuggestionDto | null>("suggest_subtask", { taskId });
}

export async function refineSubtask(
  subtaskId: number,
  instruction: string,
  instructionSource: "text" | "voice" | "system"
): Promise<SubTaskDto> {
  return invoke<SubTaskDto>("refine_subtask", {
    subtaskId,
    instruction,
    instructionSource
  });
}

export async function convertSubtaskToRevision(
  taskId: number,
  subtaskId: number,
  artifactId: number,
  selectionOrRange: RevisionTargetDto | null
): Promise<RevisionProposalResultDto> {
  return invoke<RevisionProposalResultDto>("convert_subtask_to_revision", {
    taskId,
    subtaskId,
    artifactId,
    selectionOrRange
  });
}

export async function evaluateNextSuggestions(
  taskId: number,
  activeArtifactId: number | null
): Promise<SuggestionEvaluationDto> {
  return invoke<SuggestionEvaluationDto>("evaluate_next_suggestions", {
    taskId,
    activeArtifactId
  });
}

export async function listSuggestions(taskId: number): Promise<SuggestionDto[]> {
  return invoke<SuggestionDto[]>("list_suggestions", { taskId });
}

export async function dismissSuggestion(taskId: number, suggestionId: number): Promise<SuggestionDto> {
  return invoke<SuggestionDto>("dismiss_suggestion", { taskId, suggestionId });
}

export async function explainSuggestion(taskId: number, suggestionId: number): Promise<string> {
  return invoke<string>("explain_suggestion", { taskId, suggestionId });
}

export async function acceptSuggestion(
  taskId: number,
  suggestionId: number,
  activeArtifactId: number | null,
  selectionOrRange: RevisionTargetDto | null
): Promise<SuggestionAcceptanceDto> {
  return invoke<SuggestionAcceptanceDto>("accept_suggestion", {
    taskId,
    suggestionId,
    activeArtifactId,
    selectionOrRange
  });
}

export async function getSessionModeState(taskId: number): Promise<SessionModeStateDto | null> {
  return invoke<SessionModeStateDto | null>("get_session_mode_state", { taskId });
}

export async function listRecentEvents(taskId: number, limit = 20): Promise<EventLogEntryDto[]> {
  return invoke<EventLogEntryDto[]>("list_recent_events", { taskId, limit });
}

export async function getActiveArtifactSelection(taskId: number): Promise<number | null> {
  return invoke<number | null>("get_active_artifact_selection", { taskId });
}

export async function setActiveArtifactSelection(
  taskId: number,
  artifactId: number | null
): Promise<number | null> {
  return invoke<number | null>("set_active_artifact_selection", {
    taskId,
    artifactId
  });
}

// phase 13: workspace awareness

export interface WatcherStatusDto {
  task_id: number;
  is_watching: boolean;
  watched_path: string | null;
}

export interface RecentlyLearnedItemDto {
  id: number;
  task_id: number;
  source: "file" | "clipboard";
  display_label: string;
  preview_text: string;
  ingested_at: string;
}

export async function startWorkspaceWatcher(
  taskId: number,
  folderPath: string
): Promise<WatcherStatusDto> {
  return invoke<WatcherStatusDto>("start_workspace_watcher", { taskId, folderPath });
}

export async function stopWorkspaceWatcher(taskId: number): Promise<WatcherStatusDto> {
  return invoke<WatcherStatusDto>("stop_workspace_watcher", { taskId });
}

export async function getWatcherStatus(taskId: number): Promise<WatcherStatusDto> {
  return invoke<WatcherStatusDto>("get_watcher_status", { taskId });
}

export async function listRecentlyLearned(
  taskId: number,
  limit = 10
): Promise<RecentlyLearnedItemDto[]> {
  return invoke<RecentlyLearnedItemDto[]>("list_recently_learned", { taskId, limit });
}

export async function setClipboardCapture(taskId: number, enabled: boolean): Promise<void> {
  return invoke<void>("set_clipboard_capture", { taskId, enabled });
}

export async function getClipboardCaptureSetting(taskId: number): Promise<boolean> {
  return invoke<boolean>("get_clipboard_capture_setting", { taskId });
}

// phase 14: intent classification

export type IntentLabel = "answer" | "revision" | "subtask" | "suggestion" | "unknown";

export interface IntentSlotsDto {
  target_description: string | null;
  instruction: string | null;
  draft_type: string | null;
  topic: string | null;
}

export interface IntentClassificationDto {
  intent: IntentLabel;
  confidence: number;
  slots: IntentSlotsDto;
}

export async function classifyMessageIntent(
  taskId: number,
  messageText: string
): Promise<IntentClassificationDto> {
  return invoke<IntentClassificationDto>("classify_message_intent", {
    taskId,
    messageText,
  });
}

// phase 15: proactive initiation

export interface ReorientationDto {
  task_id: number;
  summary: string;
  fired_at: string;
}

export interface DriftFlagDto {
  task_id: number;
  is_drifting: boolean;
  flag_reason: string;
  confidence: number;
}

export interface ProactiveTriggerDto {
  task_id: number;
  trigger_type: string;
  fired: boolean;
  suppressed_reason: string | null;
}

export async function triggerTaskResume(taskId: number): Promise<ReorientationDto> {
  return invoke<ReorientationDto>("trigger_task_resume", { taskId });
}

export async function checkTaskDrift(
  taskId: number,
  currentText: string
): Promise<DriftFlagDto> {
  return invoke<DriftFlagDto>("check_task_drift", { taskId, currentText });
}

export async function triggerSpeculativeSubtask(
  taskId: number
): Promise<SubTaskDto | null> {
  return invoke<SubTaskDto | null>("trigger_speculative_subtask", { taskId });
}

export async function dismissProactiveTrigger(
  taskId: number,
  triggerType: string
): Promise<void> {
  return invoke<void>("dismiss_proactive_trigger", { taskId, triggerType });
}

export async function recordTaskFocus(taskId: number): Promise<void> {
  return invoke<void>("record_task_focus", { taskId });
}

// phase 16: richer parallel work

export interface SubTaskStepDto {
  id: number;
  subtask_id: number;
  step_index: number;
  step_type: string;
  status: string;
  description: string;
  result_summary: string | null;
  result_payload: string | null;
  error_message: string | null;
  started_at: string | null;
  completed_at: string | null;
}

export interface FileWriteProposalDto {
  id: number;
  subtask_id: number;
  step_id: number | null;
  task_id: number;
  proposed_path: string;
  proposed_content: string;
  status: string;
  proposed_at: string;
  resolved_at: string | null;
}

export interface WriteAuditEntryDto {
  id: number;
  task_id: number;
  subtask_id: number;
  proposal_id: number;
  action: string;
  proposed_path: string;
  resolved_at: string;
}

export async function listSubtaskSteps(
  taskId: number,
  subtaskId: number
): Promise<SubTaskStepDto[]> {
  return invoke<SubTaskStepDto[]>("list_subtask_steps", { taskId, subtaskId });
}

export async function listFileWriteProposals(
  taskId: number
): Promise<FileWriteProposalDto[]> {
  return invoke<FileWriteProposalDto[]>("list_file_write_proposals", { taskId });
}

export async function approveSubtaskFileWrite(
  taskId: number,
  proposalId: number
): Promise<WriteAuditEntryDto> {
  return invoke<WriteAuditEntryDto>("approve_subtask_file_write", {
    taskId,
    proposalId,
  });
}

export async function rejectSubtaskFileWrite(
  taskId: number,
  proposalId: number
): Promise<WriteAuditEntryDto> {
  return invoke<WriteAuditEntryDto>("reject_subtask_file_write", {
    taskId,
    proposalId,
  });
}

export async function listWriteAuditLog(
  taskId: number
): Promise<WriteAuditEntryDto[]> {
  return invoke<WriteAuditEntryDto[]>("list_write_audit_log", { taskId });
}

export async function startSubtaskChain(
  taskId: number,
  title: string,
  description: string,
  executionType: string,
  instructionSource?: string
): Promise<SubTaskDto> {
  return invoke<SubTaskDto>("start_subtask_chain", {
    taskId,
    title,
    description,
    executionType,
    instructionSource: instructionSource ?? "text",
  });
}
