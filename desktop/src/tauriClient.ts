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
    | "proactive_reorientation"
    | "proactive_drift"
    | "proactive_blocker"
    | "proactive_deadline"
    | "proactive_speculative_subtask"
    | string;
  content: string;
  created_at: string;
}

export interface EpisodeDto {
  id: number;
  task_id: number;
  kind: string;
  text: string;
  salience: number;
  source: string;
  created_at: string;
  consolidated_at: string | null;
}

export interface EpisodeSearchResultDto {
  episode: EpisodeDto;
  similarity_score: number;
}

export interface FactDto {
  id: number;
  text: string;
  kind: string;
  confidence: number;
  evidence_ids_json: string;
  salience: number;
  last_reinforced: string;
  created_at: string;
}

export interface ConsolidationReportDto {
  processed_episode_count: number;
  upserted_fact_count: number;
  merged_fact_count: number;
  decayed_fact_count: number;
  dropped_fact_count: number;
  marked_episode_count: number;
}

export interface MemoryPromptPreviewDto {
  prompt_context: string | null;
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
  parent_revision_id: number | null;
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

export interface AgentJobDto {
  id: number;
  task_id: number;
  goal_contract: string;
  plan_json: string;
  budget_json: string;
  status: string;
  speculative: boolean;
  deliverable_json: string | null;
  verification_transcript: string | null;
  capability_request_json: string | null;
  error_message: string | null;
  created_at: string;
  updated_at: string;
}

export interface AgentJobStepDto {
  id: number;
  job_id: number;
  step_index: number;
  phase: string;
  status: string;
  title: string;
  input_json: string;
  output_json: string | null;
  error_message: string | null;
  started_at: string | null;
  completed_at: string | null;
}

export interface AgentJobArtifactDto {
  id: number;
  job_id: number;
  artifact_type: string;
  title: string;
  content: string;
  metadata_json: string;
  created_at: string;
}

export interface AgentJobEventDto {
  id: number;
  job_id: number;
  event_type: string;
  payload_json: string;
  created_at: string;
}

export interface AgentJobCheckpointDto {
  id: number;
  job_id: number;
  step_index: number;
  phase: string;
  state_json: string;
  created_at: string;
}

export interface AgentJobSteeringDto {
  id: number;
  job_id: number;
  message: string;
  status: string;
  boundary_step_index: number | null;
  created_at: string;
  applied_at: string | null;
}

export interface AgentJobDetailDto {
  job: AgentJobDto;
  steps: AgentJobStepDto[];
  artifacts: AgentJobArtifactDto[];
  events: AgentJobEventDto[];
  checkpoints: AgentJobCheckpointDto[];
  steering: AgentJobSteeringDto[];
}

export interface StandingJobDto {
  id: number;
  task_id: number;
  goal_contract: string;
  schedule_spec: string;
  trigger_kind: string;
  next_run_at: string;
  enabled: boolean;
  critical: boolean;
  last_job_id: number | null;
  created_at: string;
  updated_at: string;
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
  inference_mode: "bundled" | "byok" | string;
  bundled_inference_configured: boolean;
}

export async function setInferenceMode(mode: "bundled" | "byok"): Promise<void> {
  await invoke("set_inference_mode", { mode });
}

export async function configureBundledInference(endpoint?: string): Promise<void> {
  await invoke("configure_bundled_inference", { endpoint: endpoint ?? null });
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

// apex a1: anthropic key management + model router tier config.
// the anthropic key is optional — without it, anthropic-configured tiers
// fall back to openai automatically.

export async function storeAnthropicApiKey(apiKey: string): Promise<void> {
  return invoke<void>("store_anthropic_api_key", { apiKey });
}

export async function deleteAnthropicApiKey(): Promise<void> {
  return invoke<void>("delete_anthropic_api_key");
}

export async function getAnthropicKeyConfigured(): Promise<boolean> {
  return invoke<boolean>("get_anthropic_key_configured");
}

export interface TierConfigDto {
  provider: "local" | "openai" | "anthropic";
  model: string;
}

export interface RouterConfigDto {
  reflex: TierConfigDto;
  conversation: TierConfigDto;
  judgment: TierConfigDto;
  craft: TierConfigDto;
}

export interface LocalRuntimeStatusDto {
  enabled: boolean;
  healthy: boolean;
  running: boolean;
  mode: string;
  sidecar_configured: boolean;
  sidecar_pid: number | null;
  endpoint: string;
  model_dir: string;
  reasoning_model_id: string;
  reasoning_model_path: string;
  reasoning_model_present: boolean;
  embedding_model_id: string;
  embedding_model_path: string;
  embedding_model_present: boolean;
  embedding_mode: string;
  semantic_embedding_available: boolean;
  curated_embedding_url: string;
  curated_embedding_sha256: string;
  curated_embedding_bytes: number;
  deterministic_fallback_enabled: boolean;
  last_error: string | null;
  disk_available_bytes: number | null;
  installed_model_bytes: number;
}

export interface CostGovernorStatusDto {
  today_total_usd: number;
  tiers: CostTierSpendDto[];
  history: CostHistoryEntryDto[];
  last_notice: string | null;
}

// apex c2: weekly interruption self-audit.
export interface InterruptionAuditDto {
  days: number;
  delivered: number;
  engaged: number;
}

export async function getInterruptionAudit(): Promise<InterruptionAuditDto> {
  return invoke<InterruptionAuditDto>("get_interruption_audit");
}

// apex c3: end-of-day debrief opt-in.
export async function getDebriefEnabled(): Promise<boolean> {
  return invoke<boolean>("get_debrief_enabled");
}

export async function setDebriefEnabled(enabled: boolean): Promise<void> {
  return invoke<void>("set_debrief_enabled", { enabled });
}

export interface CostTierSpendDto {
  tier: string;
  budget_key: string;
  budget_usd: number;
  spent_usd: number;
  over_budget: boolean;
  degrade_to: string | null;
}

export interface CostHistoryEntryDto {
  date: string;
  total_usd: number;
}

export async function getTierModelMap(): Promise<RouterConfigDto> {
  return invoke<RouterConfigDto>("get_tier_model_map");
}

export async function setTierModelMap(config: RouterConfigDto): Promise<void> {
  return invoke<void>("set_tier_model_map", { config });
}

export async function getLocalRuntimeStatus(): Promise<LocalRuntimeStatusDto> {
  return invoke<LocalRuntimeStatusDto>("get_local_runtime_status");
}

export async function startLocalRuntime(): Promise<LocalRuntimeStatusDto> {
  return invoke<LocalRuntimeStatusDto>("start_local_runtime");
}

export async function stopLocalRuntime(): Promise<LocalRuntimeStatusDto> {
  return invoke<LocalRuntimeStatusDto>("stop_local_runtime");
}

export async function deleteLocalModel(kind: "reasoning" | "embedding"): Promise<LocalRuntimeStatusDto> {
  return invoke<LocalRuntimeStatusDto>("delete_local_model", { kind });
}

export async function downloadLocalModel(
  kind: "reasoning" | "embedding",
  url: string,
  sha256: string,
  expectedBytes?: number | null
): Promise<LocalRuntimeStatusDto> {
  return invoke<LocalRuntimeStatusDto>("download_local_model", {
    kind,
    url,
    sha256,
    expectedBytes: expectedBytes ?? null,
  });
}

// apex b1: one-click download of the curated semantic embedding model.
export async function downloadCuratedEmbeddingModel(): Promise<LocalRuntimeStatusDto> {
  return invoke<LocalRuntimeStatusDto>("download_curated_embedding_model");
}

export async function getCostGovernorStatus(): Promise<CostGovernorStatusDto> {
  return invoke<CostGovernorStatusDto>("get_cost_governor_status");
}

export async function setLlmDailyBudget(
  budgetKey: string,
  budgetUsd: number
): Promise<CostGovernorStatusDto> {
  return invoke<CostGovernorStatusDto>("set_llm_daily_budget", { budgetKey, budgetUsd });
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

export async function getWorkspacePromptDismissed(): Promise<boolean> {
  return invoke<boolean>("get_workspace_prompt_dismissed");
}

export async function setWorkspacePromptDismissed(dismissed: boolean): Promise<void> {
  return invoke<void>("set_workspace_prompt_dismissed", { dismissed });
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

export async function listEpisodes(taskId: number, limit = 50): Promise<EpisodeDto[]> {
  return invoke<EpisodeDto[]>("list_episodes", { taskId, limit });
}

export async function searchEpisodes(
  taskId: number,
  query: string,
  limit = 10
): Promise<EpisodeSearchResultDto[]> {
  return invoke<EpisodeSearchResultDto[]>("search_episodes", { taskId, query, limit });
}

export async function deleteEpisode(id: number): Promise<void> {
  return invoke<void>("delete_episode", { id });
}

export async function clearMemoryEpisodes(): Promise<void> {
  return invoke<void>("clear_memory_episodes");
}

export async function listFacts(limit = 100): Promise<FactDto[]> {
  return invoke<FactDto[]>("list_facts", { limit });
}

export async function deleteFact(id: number): Promise<void> {
  return invoke<void>("delete_fact", { id });
}

export async function clearMemoryFacts(): Promise<void> {
  return invoke<void>("clear_memory_facts");
}

export async function runMemoryConsolidation(): Promise<ConsolidationReportDto> {
  return invoke<ConsolidationReportDto>("run_memory_consolidation");
}

export async function previewMemoryPromptContext(): Promise<MemoryPromptPreviewDto> {
  return invoke<MemoryPromptPreviewDto>("preview_memory_prompt_context");
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

// apex c4: realtime voice sessions.
export interface VoiceConfigDto {
  enabled: boolean;
  voice: string;
  model: string;
}

export interface VoiceSessionStartDto {
  state: string;
  client_secret: string | null;
  model: string;
  expires_at: number;
  fallback: boolean;
  notice: string | null;
}

export interface VoiceToolResultDto {
  action: string;
  text: string | null;
}

export async function getVoiceConfig(): Promise<VoiceConfigDto> {
  return invoke<VoiceConfigDto>("get_voice_config");
}

export async function setVoiceConfig(enabled: boolean, voice: string): Promise<VoiceConfigDto> {
  return invoke<VoiceConfigDto>("set_voice_config", { enabled, voice });
}

export async function startVoiceSession(): Promise<VoiceSessionStartDto> {
  return invoke<VoiceSessionStartDto>("start_voice_session");
}

export async function persistVoiceTranscript(
  taskId: number,
  role: "user" | "assistant",
  text: string
): Promise<number> {
  return invoke<number>("persist_voice_transcript", { taskId, role, text });
}

export async function handleVoiceToolCall(
  taskId: number,
  name: string,
  args: Record<string, unknown>
): Promise<VoiceToolResultDto> {
  return invoke<VoiceToolResultDto>("handle_voice_tool_call", { taskId, name, arguments: args });
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

export async function generateRevisionAlternative(
  taskId: number,
  revisionId: number,
): Promise<RevisionProposalDto> {
  return invoke<RevisionProposalDto>("generate_revision_alternative", { taskId, revisionId });
}

export async function listRevisionAlternatives(
  revisionId: number,
): Promise<RevisionProposalDto[]> {
  return invoke<RevisionProposalDto[]>("list_revision_alternatives", { revisionId });
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

export async function createAgentJob(args: {
  taskId: number;
  goalContract: string;
  budgetJson?: string | null;
  speculative?: boolean | null;
}): Promise<AgentJobDetailDto> {
  return invoke<AgentJobDetailDto>("create_agent_job", args);
}

export async function listAgentJobs(taskId?: number | null, limit?: number | null): Promise<AgentJobDto[]> {
  return invoke<AgentJobDto[]>("list_agent_jobs", { taskId: taskId ?? null, limit: limit ?? null });
}

export async function getAgentJobDetail(jobId: number): Promise<AgentJobDetailDto> {
  return invoke<AgentJobDetailDto>("get_agent_job_detail", { jobId });
}

export async function runAgentJob(jobId: number): Promise<AgentJobDetailDto> {
  return invoke<AgentJobDetailDto>("run_agent_job", { jobId });
}

export async function sendJobSteering(jobId: number, message: string): Promise<AgentJobDetailDto> {
  return invoke<AgentJobDetailDto>("send_job_steering", { jobId, message });
}

export async function cancelAgentJob(jobId: number): Promise<AgentJobDetailDto> {
  return invoke<AgentJobDetailDto>("cancel_agent_job", { jobId });
}

export async function resumeAgentJobs(): Promise<AgentJobDetailDto[]> {
  return invoke<AgentJobDetailDto[]>("resume_agent_jobs");
}

export async function createStandingJob(args: {
  taskId: number;
  goalContract: string;
  scheduleSpec: string;
  critical?: boolean | null;
}): Promise<StandingJobDto> {
  return invoke<StandingJobDto>("create_standing_job", args);
}

export async function listStandingJobs(taskId?: number | null): Promise<StandingJobDto[]> {
  return invoke<StandingJobDto[]>("list_standing_jobs", { taskId: taskId ?? null });
}

export async function runDueStandingJobs(eventName?: string | null): Promise<AgentJobDetailDto[]> {
  return invoke<AgentJobDetailDto[]>("run_due_standing_jobs", { eventName: eventName ?? null });
}

export async function setStandingJobEnabled(
  standingJobId: number,
  enabled: boolean
): Promise<StandingJobDto> {
  return invoke<StandingJobDto>("set_standing_job_enabled", { standingJobId, enabled });
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

export async function ensureWorkspaceWatcher(taskId: number): Promise<WatcherStatusDto> {
  return invoke<WatcherStatusDto>("ensure_workspace_watcher", { taskId });
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
  resolved_path?: string;
  action_receipt_id?: number | null;
}

export interface ActionReceiptDto {
  id: number;
  task_id: number;
  class: string;
  surface: string;
  level: string;
  description: string;
  payload_excerpt: string;
  status: string;
  failure_reason: string | null;
  undo_ref: string | null;
  created_at: string;
  resolved_at: string | null;
}

export interface NativeDocsStatusDto {
  pages_supported: boolean;
  word_supported: boolean;
  automation_permission_status: string;
  automation_permission_explainer: string;
  ax_buffer_writeback_enabled: boolean;
}

export interface TrustLevelDto {
  class: string;
  level: string;
  max_level: string;
  approval_streak: number;
  graduation_offer: string | null;
  graduation_offered_at: string | null;
  sticky_l1: boolean;
  updated_at: string;
  recent_history: ActionReceiptDto[];
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

export async function listActionReceipts(
  taskId?: number,
  limit?: number
): Promise<ActionReceiptDto[]> {
  return invoke<ActionReceiptDto[]>("list_action_receipts", { taskId: taskId ?? null, limit: limit ?? null });
}

export async function revertActionReceipt(receiptId: number): Promise<ActionReceiptDto> {
  return invoke<ActionReceiptDto>("revert_action_receipt", { receiptId });
}

export async function requestGoogleDocsWrite(args: {
  taskId: number;
  documentTitle: string;
  beforeText: string;
  afterText: string;
  anchorBefore: string;
  anchorAfter: string;
  preferSuggesting: boolean;
}): Promise<ActionReceiptDto> {
  return invoke<ActionReceiptDto>("request_google_docs_write", args);
}

export async function getNativeDocsStatus(): Promise<NativeDocsStatusDto> {
  return invoke<NativeDocsStatusDto>("get_native_docs_status");
}

export async function requestNativeDocWrite(args: {
  taskId: number;
  appName: string;
  documentTitle: string;
  beforeText: string;
  afterText: string;
  anchorBefore: string;
  anchorAfter: string;
  observedText?: string | null;
}): Promise<ActionReceiptDto> {
  return invoke<ActionReceiptDto>("request_native_doc_write", args);
}

export async function listTrustLadder(): Promise<TrustLevelDto[]> {
  return invoke<TrustLevelDto[]>("list_trust_ladder");
}

export async function setTrustLevel(
  actionClass: string,
  level: "L1" | "L2" | "L3"
): Promise<TrustLevelDto[]> {
  return invoke<TrustLevelDto[]>("set_trust_level", { actionClass, level });
}

export async function demoteTrustClass(actionClass: string): Promise<TrustLevelDto[]> {
  return invoke<TrustLevelDto[]>("demote_trust_class", { actionClass });
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

// phase 19: session restore state
// the backend restores session (overlay mode, quiet mode, watcher) in main.rs
// setup before any window is shown. this command is a pure read for the
// frontend to learn what was restored and adapt its initial display.

export interface SessionRestoreDto {
  had_active_task: boolean;
  overlay_expanded: boolean;
  quiet_mode: boolean;
}

export async function getSessionRestoreState(): Promise<SessionRestoreDto> {
  return invoke<SessionRestoreDto>("get_session_restore_state");
}

// phase 20: active window context

export interface ActiveWindowContextDto {
  app_name: string;
  document_title: string;
  captured_at: number;
}

export async function getActiveWindowContext(): Promise<ActiveWindowContextDto | null> {
  return invoke<ActiveWindowContextDto | null>("get_active_window_context");
}

export async function getAccessibilityPermissionStatus(): Promise<boolean> {
  return invoke<boolean>("get_accessibility_permission_status");
}

export async function requestAccessibilityPermission(): Promise<void> {
  return invoke<void>("request_accessibility_permission");
}

// phase 21: privacy and trust control center

export interface PrivacyCenterDashboardDto {
  active_task_id: number | null;
  active_task_title: string | null;
  workspace_watcher_enabled: boolean;
  workspace_folder_path: string | null;
  workspace_watched_file_count: number;
  workspace_watcher_running: boolean;
  clipboard_capture_enabled: boolean;
  clipboard_capture_reminder: string;
  active_window_context_enabled: boolean;
  accessibility_permission_status: string;
  proactive_triggers_enabled: boolean;
  user_profile_memory_enabled: boolean;
  user_profile_signal_count: number;
  calendar_context_enabled: boolean;
  calendar_permission_status: string;
  selection_capture_enabled: boolean;
  typing_activity_enabled: boolean;
  tts_voice: string;
  available_tts_voices: string[];
  wake_word: WakeWordStatusDto;
  crisis_controls: CrisisClassControlDto[];
  action_receipts: ActionReceiptDto[];
  native_docs: NativeDocsStatusDto;
  trust_ladder: TrustLevelDto[];
  // phase 31: content observation
  content_observation_enabled: boolean;
  content_observation_last_captured_at: string | null;
  content_observation_capture_failed: boolean;
  content_observation_failed_app: string | null;
  content_observation_source_origin: string | null;
  content_observation_document_title: string | null;
  local_runtime: LocalRuntimeStatusDto;
  cost_governor: CostGovernorStatusDto;
  speculation: SpeculationStatusDto;
}

export interface SpeculationStatusDto {
  enabled: boolean;
  spent_today_usd: number;
  daily_budget_usd: number;
  within_budget: boolean;
  hit_rate: number;
  predicted_count: number;
  hit_count: number;
  miss_count: number;
  fresh_cached: number;
}

export interface SpeculationCacheDto {
  id: number;
  task_id: number;
  request_text: string;
  request_signature: string;
  job_id: number | null;
  status: string;
  created_at: string;
}

export async function getSpeculationStatus(): Promise<SpeculationStatusDto> {
  return invoke<SpeculationStatusDto>("get_speculation_status");
}

export async function setSpeculationEnabled(enabled: boolean): Promise<SpeculationStatusDto> {
  return invoke<SpeculationStatusDto>("set_speculation_enabled", { enabled });
}

export async function listSpeculationCache(limit?: number): Promise<SpeculationCacheDto[]> {
  return invoke<SpeculationCacheDto[]>("list_speculation_cache", { limit: limit ?? null });
}

export async function discardSpeculationCacheEntry(cacheId: number): Promise<void> {
  await invoke("discard_speculation_cache_entry", { cacheId });
}

export interface CapabilityGapDto {
  id: number;
  surface: string;
  description: string;
  occurrence_count: number;
  created_at: string;
  updated_at: string;
}

export interface CustomToolDto {
  id: number;
  name: string;
  kind: string;
  purpose: string;
  target_allowlist: string[];
  code: string;
  test_transcript: string | null;
  status: string;
  created_at: string;
}

export interface CustomToolRunResultDto {
  status: string;
  output: string | null;
  message: string;
  receipt_id: number | null;
}

export async function listCapabilityGaps(): Promise<CapabilityGapDto[]> {
  return invoke<CapabilityGapDto[]>("list_capability_gaps");
}

export async function listCustomTools(): Promise<CustomToolDto[]> {
  return invoke<CustomToolDto[]>("list_custom_tools");
}

export async function proposeCustomTool(gapId: number): Promise<CustomToolDto> {
  return invoke<CustomToolDto>("propose_custom_tool", { gapId });
}

export async function approveCustomTool(toolId: number): Promise<CustomToolDto> {
  return invoke<CustomToolDto>("approve_custom_tool", { toolId });
}

export async function killCustomTool(toolId: number): Promise<CustomToolDto> {
  return invoke<CustomToolDto>("kill_custom_tool", { toolId });
}

export async function runCustomTool(
  taskId: number,
  name: string,
  input: string
): Promise<CustomToolRunResultDto> {
  return invoke<CustomToolRunResultDto>("run_custom_tool", { taskId, name, input });
}

export async function approveCustomToolRun(receiptId: number): Promise<CustomToolRunResultDto> {
  return invoke<CustomToolRunResultDto>("approve_custom_tool_run", { receiptId });
}

export async function rejectCustomToolRun(receiptId: number): Promise<CustomToolRunResultDto> {
  return invoke<CustomToolRunResultDto>("reject_custom_tool_run", { receiptId });
}

export interface ToolConnectionDto {
  id: number;
  name: string;
  transport: string;
  endpoint: string;
  scopes: string[];
  enabled: boolean;
  created_at: string;
}

export interface ToolCallLogDto {
  id: number;
  connection_name: string;
  tool_name: string;
  argument_summary: string;
  status: string;
  created_at: string;
}

export interface ToolDescriptorDto {
  id: number;
  connection_id: number;
  tool_name: string;
  description: string;
}

export interface ConnectedActionResult {
  receipt_id: number;
  status: string;
  tool_result: unknown | null;
}

export async function listToolConnections(): Promise<ToolConnectionDto[]> {
  return invoke<ToolConnectionDto[]>("list_tool_connections");
}

export async function addToolConnection(args: {
  name: string;
  transport: "stdio" | "http";
  endpoint: string;
  scopes: string[];
}): Promise<ToolConnectionDto> {
  return invoke<ToolConnectionDto>("add_tool_connection", args);
}

export async function discoverConnectionTools(connectionId: number): Promise<ToolDescriptorDto[]> {
  return invoke<ToolDescriptorDto[]>("discover_connection_tools", { connectionId });
}

export async function approveConnectedAction(receiptId: number): Promise<ConnectedActionResult> {
  return invoke<ConnectedActionResult>("approve_connected_action", { receiptId });
}

export async function rejectConnectedAction(receiptId: number): Promise<ConnectedActionResult> {
  return invoke<ConnectedActionResult>("reject_connected_action", { receiptId });
}

export async function pullRemoteDoc(taskId: number, query: string): Promise<RemoteDocDto> {
  return invoke<RemoteDocDto>("pull_remote_doc", { taskId, query });
}

export async function setToolConnectionEnabled(
  connectionId: number,
  enabled: boolean
): Promise<ToolConnectionDto> {
  return invoke<ToolConnectionDto>("set_tool_connection_enabled", { connectionId, enabled });
}

export async function removeToolConnection(connectionId: number): Promise<void> {
  await invoke("remove_tool_connection", { connectionId });
}

export async function listToolCallLog(limit?: number): Promise<ToolCallLogDto[]> {
  return invoke<ToolCallLogDto[]>("list_tool_call_log", { limit: limit ?? null });
}

export interface WebQueryLogDto {
  id: number;
  query: string;
  tool: string;
  result_count: number;
  status: string;
  created_at: string;
}

export interface WebSourceDto {
  url: string;
  title: string;
  snippet: string;
}

export interface WebDocumentDto {
  url: string;
  title: string;
  content: string;
}

export async function webSearch(query: string): Promise<{
  sources: WebSourceDto[];
  source_ledger: Array<{ url: string; title: string; file_name: string }>;
}> {
  return invoke("web_search", { query });
}

export async function webFetch(url: string): Promise<WebDocumentDto> {
  return invoke<WebDocumentDto>("web_fetch", { url });
}

export async function listWebQueryLog(limit?: number): Promise<WebQueryLogDto[]> {
  return invoke<WebQueryLogDto[]>("list_web_query_log", { limit: limit ?? null });
}

export async function setWebUserNameGuard(name: string): Promise<void> {
  await invoke("set_web_user_name_guard", { name });
}

export interface EmailReplyWatchDto {
  id: number;
  task_id: number | null;
  sender: string;
  thread_hint: string;
  status: string;
  created_at: string;
}

export async function listEmailReplyWatches(): Promise<EmailReplyWatchDto[]> {
  return invoke<EmailReplyWatchDto[]>("list_email_reply_watches");
}

export async function registerEmailReplyWatch(
  taskId: number,
  sender: string,
  threadHint?: string
): Promise<EmailReplyWatchDto> {
  return invoke<EmailReplyWatchDto>("register_email_reply_watch", {
    taskId,
    sender,
    threadHint: threadHint ?? null
  });
}

export interface RemoteDocDto {
  id: number;
  task_id: number;
  title: string;
  url: string;
  provenance: string;
  artifact_id: number | null;
  created_at: string;
}

export async function listRemoteDocs(): Promise<RemoteDocDto[]> {
  return invoke<RemoteDocDto[]>("list_remote_docs");
}

export async function removeRemoteDoc(id: number): Promise<void> {
  await invoke("remove_remote_doc", { id });
}

export interface WakeWordStatusDto {
  enabled: boolean;
  configured: boolean;
  armed: boolean;
  running: boolean;
  sidecar_pid: number | null;
  phrase: string;
  last_detected_at: number | null;
  last_error: string | null;
  no_raw_audio_ipc: boolean;
}

export interface CrisisClassControlDto {
  class: string;
  label: string;
  enabled: boolean;
}

export interface CrisisCardDto {
  task_id: number;
  class: string;
  title: string;
  message: string;
  evidence: string;
  delivery_channel: string;
  quiet_downgraded: boolean;
  voice_if_session_open: boolean;
}

export type SelectionCaptureStatus = "captured" | "failed";

export interface SelectionCaptureIndicatorDto {
  status: SelectionCaptureStatus;
  app_name: string;
  document_title: string | null;
  captured_at: number;
  word_count: number;
  source_type: string;
  message: string;
}

export interface BrowserSelectionCaptureRequestDto {
  token: string;
  text: string;
  app_name: string;
  document_title: string | null;
  source_url: string | null;
  captured_at: number | null;
}

export interface SelectionBridgeStatusDto {
  enabled: boolean;
  port: number;
  token: string;
}

export interface ProactiveAuditEntryDto {
  id: number;
  task_id: number;
  trigger_type: string;
  fired_at: string;
  suppressed: boolean;
}

export interface SynthesisLogEntryDto {
  id: number;
  task_id: number | null;
  reason_type: string;
  reason_detail: string | null;
  snapshot_confidence: number;
  snapshot_attention_state: string;
  message: string | null;
  delivered: boolean;
  delivered_at: string | null;
  created_at: string;
}

export interface DataClearResultDto {
  cleared: boolean;
  active_task_id: number | null;
  message: string;
}

export async function getPrivacyCenterDashboard(): Promise<PrivacyCenterDashboardDto> {
  return invoke<PrivacyCenterDashboardDto>("get_privacy_center_dashboard");
}

export async function setPrivacySurfaceEnabled(
  surface: string,
  enabled: boolean
): Promise<PrivacyCenterDashboardDto> {
  return invoke<PrivacyCenterDashboardDto>("set_privacy_surface_enabled", {
    surface,
    enabled,
  });
}

export async function getWakeWordStatus(): Promise<WakeWordStatusDto> {
  return invoke<WakeWordStatusDto>("get_wake_word_status");
}

export async function setWakeWordEnabled(enabled: boolean): Promise<WakeWordStatusDto> {
  return invoke<WakeWordStatusDto>("set_wake_word_enabled", { enabled });
}

export async function setCrisisClassEnabled(
  className: string,
  enabled: boolean
): Promise<PrivacyCenterDashboardDto> {
  return invoke<PrivacyCenterDashboardDto>("set_crisis_class_enabled", {
    className,
    enabled,
  });
}

export async function recordCrisisFeedback(
  taskId: number,
  className: string,
  evidence: string
): Promise<void> {
  return invoke<void>("record_crisis_feedback", { taskId, className, evidence });
}

export async function getSelectionCaptureIndicator(): Promise<SelectionCaptureIndicatorDto | null> {
  return invoke<SelectionCaptureIndicatorDto | null>("get_selection_capture_indicator");
}

export async function dismissSelectionCapture(): Promise<SelectionCaptureIndicatorDto | null> {
  return invoke<SelectionCaptureIndicatorDto | null>("dismiss_selection_capture");
}

export async function getSelectionBridgeStatus(): Promise<SelectionBridgeStatusDto> {
  return invoke<SelectionBridgeStatusDto>("get_selection_bridge_status");
}

export async function captureBrowserSelection(
  request: BrowserSelectionCaptureRequestDto
): Promise<SelectionCaptureIndicatorDto> {
  return invoke<SelectionCaptureIndicatorDto>("capture_browser_selection", { request });
}

export async function setTtsVoice(voice: string): Promise<PrivacyCenterDashboardDto> {
  return invoke<PrivacyCenterDashboardDto>("set_tts_voice", { voice });
}

export async function clearUserProfileMemory(): Promise<PrivacyCenterDashboardDto> {
  return invoke<PrivacyCenterDashboardDto>("clear_user_profile_memory");
}

export async function setContentObservationEnabled(
  taskId: number,
  enabled: boolean
): Promise<PrivacyCenterDashboardDto> {
  return invoke<PrivacyCenterDashboardDto>("set_content_observation_enabled", { taskId, enabled });
}

export async function getContentObservationEnabled(taskId: number): Promise<boolean> {
  return invoke<boolean>("get_content_observation_enabled", { taskId });
}

export async function clearContentObservation(): Promise<void> {
  return invoke<void>("clear_content_observation");
}

export async function listProactiveTriggerAuditLog(
  taskId: number
): Promise<ProactiveAuditEntryDto[]> {
  return invoke<ProactiveAuditEntryDto[]>("list_proactive_trigger_audit_log", { taskId });
}

export async function getSynthesisLog(taskId: number): Promise<SynthesisLogEntryDto[]> {
  return invoke<SynthesisLogEntryDto[]>("get_synthesis_log", { taskId });
}

export async function clearActiveTaskData(): Promise<DataClearResultDto> {
  return invoke<DataClearResultDto>("clear_active_task_data");
}

export async function clearAllJeffData(): Promise<DataClearResultDto> {
  return invoke<DataClearResultDto>("clear_all_jeff_data");
}

// phase 23 types and commands

export interface UserProfileSignalDto {
  key: string;
  label: string;
  value: string;
  updated_at: string;
}

export type GoalStatus = "active" | "completed" | "abandoned";

export interface StatedGoalDto {
  id: number;
  task_id: number;
  goal_text: string;
  stated_at: string;
  status: GoalStatus;
  updated_at: string;
}

export interface StrugglePatternDto {
  id: number;
  pattern_text: string;
  task_ids_json: string;
  first_seen: string;
  last_seen: string;
  occurrence_count: number;
}

export interface CollaborationStyleDto {
  prefers_opinions: number;
  wants_explanations: number;
  delegation_comfort: number;
  interruption_tolerance: number;
}

export interface TrustMetricsDto {
  times_accepted_opinion: number;
  times_pushed_back: number;
  times_asked_for_more: number;
}

export interface RelationalProfileDto {
  stated_goals: StatedGoalDto[];
  struggle_patterns: StrugglePatternDto[];
  collaboration_style: CollaborationStyleDto;
  trust_metrics: TrustMetricsDto;
}

export interface WorkloadTaskDto {
  id: number;
  title: string;
  last_focused_at: string | null;
  days_since_focus: number | null;
  pending_item_count: number;
  is_active: boolean;
}

export interface WorkloadSummaryDto {
  active_tasks: WorkloadTaskDto[];
  stale_tasks: WorkloadTaskDto[];
}

export interface CalendarEventDto {
  title: string;
  starts_at: string;
  minutes_until: number;
}

export interface LiveEditReceiptDto {
  id: number;
  task_id: number | null;
  editor_surface: string;
  document_title: string;
  before_hash: string;
  after_hash: string;
  timestamp: string;
  status: string;
}

export interface PendingLiveEditDto {
  receipt_id: number;
  task_id: number | null;
  editor_surface: string;
  document_title: string;
  before_text: string;
  after_text: string;
  timestamp: string;
  status: "pending_approval" | "fallback" | "failed";
}

export async function getUserProfileSignals(): Promise<UserProfileSignalDto[]> {
  return invoke<UserProfileSignalDto[]>("get_user_profile_signals");
}

export async function getRelationalProfile(): Promise<RelationalProfileDto> {
  return invoke<RelationalProfileDto>("get_relational_profile");
}

export async function deleteStatedGoal(id: number): Promise<RelationalProfileDto> {
  return invoke<RelationalProfileDto>("delete_stated_goal", { id });
}

export async function deleteStrugglePattern(id: number): Promise<RelationalProfileDto> {
  return invoke<RelationalProfileDto>("delete_struggle_pattern", { id });
}

export async function clearRelationalProfile(): Promise<RelationalProfileDto> {
  return invoke<RelationalProfileDto>("clear_relational_profile");
}

export async function addQualityRubric(text: string): Promise<UserProfileSignalDto[]> {
  return invoke<UserProfileSignalDto[]>("add_quality_rubric", { text });
}

export async function deleteQualityRubric(key: string): Promise<UserProfileSignalDto[]> {
  return invoke<UserProfileSignalDto[]>("delete_quality_rubric", { key });
}

export async function deleteUserProfileSignal(key: string): Promise<UserProfileSignalDto[]> {
  return invoke<UserProfileSignalDto[]>("delete_user_profile_signal", { key });
}

export async function getWorkloadSummary(): Promise<WorkloadSummaryDto> {
  return invoke<WorkloadSummaryDto>("get_workload_summary");
}

export async function switchActiveTaskFromCompanion(taskId: number): Promise<TaskDto> {
  return invoke<TaskDto>("switch_active_task_from_companion", { taskId });
}

export async function requestCalendarPermission(): Promise<boolean> {
  return invoke<boolean>("request_calendar_permission");
}

export async function getCalendarPermissionStatus(): Promise<string> {
  return invoke<string>("get_calendar_permission_status");
}

export async function getCalendarNextEvent(): Promise<CalendarEventDto | null> {
  return invoke<CalendarEventDto | null>("get_calendar_next_event");
}

export async function approveLiveEdit(receiptId: number): Promise<LiveEditReceiptDto> {
  return invoke<LiveEditReceiptDto>("approve_live_edit", { receiptId });
}

export async function rejectLiveEdit(receiptId: number): Promise<LiveEditReceiptDto> {
  return invoke<LiveEditReceiptDto>("reject_live_edit", { receiptId });
}

export async function listLiveEditReceipts(taskId: number): Promise<LiveEditReceiptDto[]> {
  return invoke<LiveEditReceiptDto[]>("list_live_edit_receipts", { taskId });
}

export async function getPendingLiveEdits(): Promise<PendingLiveEditDto[]> {
  return invoke<PendingLiveEditDto[]>("get_pending_live_edits");
}

// apex f1b-3b: background daemon (Privacy Center control).
export interface BackgroundDaemonDto {
  enabled: boolean;
  running: boolean;
  pending_restart: boolean;
}

export async function getBackgroundDaemon(): Promise<BackgroundDaemonDto> {
  return invoke<BackgroundDaemonDto>("get_background_daemon");
}

export async function setBackgroundDaemonEnabled(
  enabled: boolean,
): Promise<BackgroundDaemonDto> {
  return invoke<BackgroundDaemonDto>("set_background_daemon_enabled", { enabled });
}

// apex f2b: overnight morning-readiness -- whether today's briefing was prepared
// ahead of first engagement.
export interface MorningReadinessDto {
  prepared_today: boolean;
  prepared_at: number | null;
  delivered: boolean;
}

export async function getMorningReadiness(): Promise<MorningReadinessDto> {
  return invoke<MorningReadinessDto>("get_morning_readiness");
}
