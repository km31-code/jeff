import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  isStreamingEnabled,
  LlmCompletePayload,
  LlmTokenPayload,
  TtsChunkPayload,
  TurnCancelledPayload,
  TurnCompletePayload,
  EVENT_LLM_TOKEN,
  EVENT_LLM_COMPLETE,
  EVENT_TTS_CHUNK,
  EVENT_TURN_CANCELLED,
  EVENT_TURN_COMPLETE,
} from "./streamClient";
import {
  acceptSuggestion,
  acceptSubtaskResult,
  applyRevision,
  ArtifactContentDto,
  ArtifactDto,
  ArtifactVersionDto,
  cancelSubtask,
  cancelInteraction,
  ChatMessageDto,
  ConsolidationReportDto,
  convertSubtaskToRevision,
  CoworkingStatusDto,
  createTask,
  dismissSuggestion,
  EventLogEntryDto,
  EpisodeDto,
  evaluateProactiveNudge,
  evaluateNextSuggestions,
  explainSuggestion,
  FactDto,
  getActiveArtifactSelection,
  refineSubtask,
  getActiveTask,
  getOnboardingStatus,
  getWorkspacePromptDismissed,
  setWorkspacePromptDismissed as persistWorkspacePromptDismissed,
  getArtifactContent,
  getCoworkingStatus,
  getSessionModeState,
  getTaskSummary,
  getTaskWorkspace,
  importArtifact,
  listArtifacts,
  listArtifactVersions,
  listEpisodes,
  listFacts,
  listMessages,
  listOpenResources,
  listPendingRevisions,
  listRecentEvents,
  listSuggestions,
  listSubtasks,
  listTaskPendingRevisions,
  listTasks,
  OnboardingStatusDto,
  OpenResourceDto,
  proposeArtifactRevision,
  rejectRevision,
  rejectSubtaskResult,
  RetrievedChunkDto,
  revertArtifactToVersion,
  RevisionProposalDto,
  RevisionTargetDto,
  SessionModeStateDto,
  sendMessage,
  sendMessageStreaming,
  cancelStreamingTurn,
  setActiveTask,
  setActiveArtifactSelection,
  setAssistantSpeaking,
  setCrisisClassEnabled,
  setProactiveMode,
  setUserSpeaking,
  setUserTyping,
  SuggestionAcceptanceDto,
  SuggestionDto,
  SuggestionEvaluationDto,
  SubTaskDto,
  AgentJobDto,
  AgentJobDetailDto,
  StandingJobDto,
  SubTaskSuggestionDto,
  suggestSubtask,
  synthesizeSpeech,
  TaskDto,
  TaskSummaryDto,
  transcribeAudio,
  WorkspaceInfoDto,
  WatcherStatusDto,
  RecentlyLearnedItemDto,
  startWorkspaceWatcher,
  stopWorkspaceWatcher,
  getWatcherStatus,
  ensureWorkspaceWatcher,
  listRecentlyLearned,
  setClipboardCapture,
  getClipboardCaptureSetting,
  IntentSlotsDto,
  classifyMessageIntent,
  checkTaskDrift,
  dismissProactiveTrigger,
  recordTaskFocus,
  DriftFlagDto,
  FileWriteProposalDto,
  SubTaskStepDto,
  WriteAuditEntryDto,
  listFileWriteProposals,
  listSubtaskSteps,
  approveSubtaskFileWrite,
  rejectSubtaskFileWrite,
  listWriteAuditLog,
  revertActionReceipt,
  setTrustLevel,
  demoteTrustClass,
  createAgentJob,
  listAgentJobs,
  getAgentJobDetail,
  sendJobSteering,
  cancelAgentJob,
  createStandingJob,
  listStandingJobs,
  runDueStandingJobs,
  setStandingJobEnabled,
  ActiveWindowContextDto,
  getActiveWindowContext,
  getAccessibilityPermissionStatus,
  requestAccessibilityPermission,
  PrivacyCenterDashboardDto,
  SelectionCaptureIndicatorDto,
  SelectionBridgeStatusDto,
  ProactiveAuditEntryDto,
  SynthesisLogEntryDto,
  DataClearResultDto,
  SpeculationCacheDto,
  setSpeculationEnabled,
  listSpeculationCache,
  discardSpeculationCacheEntry,
  CapabilityGapDto,
  CustomToolDto,
  listCapabilityGaps,
  listCustomTools,
  proposeCustomTool,
  approveCustomTool,
  killCustomTool,
  ToolConnectionDto,
  ToolCallLogDto,
  listToolConnections,
  setToolConnectionEnabled,
  removeToolConnection,
  listToolCallLog,
  WebQueryLogDto,
  listWebQueryLog,
  setWebUserNameGuard,
  EmailReplyWatchDto,
  listEmailReplyWatches,
  RemoteDocDto,
  listRemoteDocs,
  removeRemoteDoc,
  getPrivacyCenterDashboard,
  getInterruptionAudit,
  type InterruptionAuditDto,
  getDebriefEnabled,
  setDebriefEnabled,
  getVoiceConfig,
  setVoiceConfig,
  setWakeWordEnabled,
  setPrivacySurfaceEnabled,
  getSelectionCaptureIndicator,
  getSelectionBridgeStatus,
  dismissSelectionCapture,
  setTtsVoice,
  clearUserProfileMemory,
  deleteEpisode,
  deleteFact,
  runMemoryConsolidation,
  previewMemoryPromptContext,
  setContentObservationEnabled,
  clearContentObservation,
  deleteLocalModel,
  downloadLocalModel,
  downloadCuratedEmbeddingModel,
  setLlmDailyBudget,
  startLocalRuntime,
  stopLocalRuntime,
  listProactiveTriggerAuditLog,
  getSynthesisLog,
  clearActiveTaskData,
  clearAllJeffData,
  // phase 23
  UserProfileSignalDto,
  RelationalProfileDto,
  getUserProfileSignals,
  getRelationalProfile,
  deleteStatedGoal,
  deleteStrugglePattern,
  addQualityRubric,
  deleteUserProfileSignal,
  WorkloadSummaryDto,
  WorkloadTaskDto,
  getWorkloadSummary,
  switchActiveTaskFromCompanion,
  CalendarEventDto,
  getCalendarNextEvent,
  requestCalendarPermission,
  PendingLiveEditDto,
  getPendingLiveEdits,
  approveLiveEdit,
  rejectLiveEdit,
} from "./tauriClient";
import {
  openOnboarding,
  openOnboardingAtStep,
  setTrayStatus as setAmbientTrayStatus,
  setQuietMode,
  TrayStatus
} from "./ambientClient";

type ViewMode = "home" | "workspace";
type RecordingPurpose = "chat" | "revision" | "subtask" | "cancel_subtask";

// minimal web speech api types — not always present in all lib.dom versions.
interface SpeechRecognitionAlternative {
  transcript: string;
  confidence: number;
}
interface SpeechRecognitionResult {
  isFinal: boolean;
  length: number;
  [index: number]: SpeechRecognitionAlternative;
}
interface SpeechRecognitionResultList {
  length: number;
  [index: number]: SpeechRecognitionResult;
}
interface SpeechRecognitionEvent extends Event {
  resultIndex: number;
  results: SpeechRecognitionResultList;
}
interface PartialSpeechRecognition {
  interimResults: boolean;
  continuous: boolean;
  lang: string;
  onresult: ((event: SpeechRecognitionEvent) => void) | null;
  onerror: (() => void) | null;
  onend: (() => void) | null;
  start(): void;
  stop(): void;
}
type ExecutionType = "draft_generation" | "expansion" | "synthesis" | "targeted_research_synthesis";
type RoutedIntent = "answer" | "revision" | "subtask" | "suggestion" | "unknown";

type RetrievalDebugMeta = {
  decisionEventType: string;
  decisionReason: string;
  confidence: number | null;
};

type RevisionDebugInfo = {
  activeArtifactId: number | null;
  selectedStart: number | null;
  selectedEnd: number | null;
  selectionSource: string;
  confidence: number | null;
  groundingNotes: string;
  contextSource: string;
  instructionSource: string;
  retrievedChunks: RetrievedChunkDto[];
};

type SubTaskDebugInfo = {
  selectedSubtaskId: number | null;
  executionType: string;
  instructionSource: string;
  contextSnapshot: string;
  retrievedChunks: RetrievedChunkDto[];
};

type FlowDebugInfo = {
  decisionReason: string;
  suppressionState: string;
  noOp: boolean;
  evidenceScore: number | null;
  modeReason: string;
  retrievedChunks: RetrievedChunkDto[];
};

interface AppProps {
  onCloseWorkspace?: () => void;
}

const DEBUG_PANELS_STORAGE_KEY = "jeff_show_debug_panels";

function formatSpendUsd(value: number): string {
  if (!Number.isFinite(value)) {
    return "$0.00";
  }
  if (value > 0 && value < 0.01) {
    return `$${value.toFixed(4)}`;
  }
  return `$${value.toFixed(2)}`;
}

function budgetProgressValue(spent: number, budget: number): number {
  if (!Number.isFinite(spent) || spent <= 0) {
    return 0;
  }
  if (!Number.isFinite(budget) || budget <= 0) {
    return 1;
  }
  return Math.min(spent / budget, 1);
}

function formatMemoryKind(kind: string): string {
  return kind.replace(/_/g, " ");
}

function evidenceCount(raw: string): number {
  try {
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.length : 0;
  } catch {
    return 0;
  }
}

function App({ onCloseWorkspace }: AppProps = {}) {
  // when opened as a workspace window, start directly in workspace mode
  // with full panels visible — no home screen needed.
  const [viewMode, setViewMode] = useState<ViewMode>(onCloseWorkspace ? "workspace" : "home");
  const [fullWorkspaceVisible, setFullWorkspaceVisible] = useState(Boolean(onCloseWorkspace));
  const [showDebugPanels, setShowDebugPanels] = useState(() => {
    if (typeof window === "undefined") {
      return false;
    }
    const params = new URLSearchParams(window.location.search);
    return (
      params.get("debug") === "1" ||
      window.localStorage.getItem(DEBUG_PANELS_STORAGE_KEY) === "1"
    );
  });

  const [tasks, setTasks] = useState<TaskDto[]>([]);
  const [activeTask, setActiveTaskState] = useState<TaskDto | null>(null);
  const [taskSummary, setTaskSummary] = useState<TaskSummaryDto | null>(null);
  const [workspaceInfo, setWorkspaceInfo] = useState<WorkspaceInfoDto | null>(null);
  const [openResources, setOpenResources] = useState<OpenResourceDto[]>([]);
  const [artifacts, setArtifacts] = useState<ArtifactDto[]>([]);
  const [messages, setMessages] = useState<ChatMessageDto[]>([]);
  const [retrievalDebugChunks, setRetrievalDebugChunks] = useState<RetrievedChunkDto[]>([]);
  const [retrievalDebugMeta, setRetrievalDebugMeta] = useState<RetrievalDebugMeta>({
    decisionEventType: "system_status_event",
    decisionReason: "awaiting_activity",
    confidence: null
  });

  const [selectedArtifactId, setSelectedArtifactId] = useState<number | null>(null);
  const [artifactContent, setArtifactContent] = useState<ArtifactContentDto | null>(null);
  const [artifactSelectionStart, setArtifactSelectionStart] = useState(0);
  const [artifactSelectionEnd, setArtifactSelectionEnd] = useState(0);
  const [revisionInstruction, setRevisionInstruction] = useState("");
  const [pendingRevisions, setPendingRevisions] = useState<RevisionProposalDto[]>([]);
  const [taskPendingRevisions, setTaskPendingRevisions] = useState<RevisionProposalDto[]>([]);
  const [artifactVersions, setArtifactVersions] = useState<ArtifactVersionDto[]>([]);
  const [subtasks, setSubtasks] = useState<SubTaskDto[]>([]);
  const [agentJobs, setAgentJobs] = useState<AgentJobDto[]>([]);
  const [selectedAgentJob, setSelectedAgentJob] = useState<AgentJobDetailDto | null>(null);
  const [jobSteeringInput, setJobSteeringInput] = useState("");
  const [standingJobs, setStandingJobs] = useState<StandingJobDto[]>([]);
  const [recentEvents, setRecentEvents] = useState<EventLogEntryDto[]>([]);
  const [subtaskInstruction, setSubtaskInstruction] = useState("");
  const [subtaskExecutionType, setSubtaskExecutionType] = useState<ExecutionType>("draft_generation");
  const [subtaskSuggestion, setSubtaskSuggestion] = useState<SubTaskSuggestionDto | null>(null);
  const [subtaskRefinementInputById, setSubtaskRefinementInputById] = useState<Record<number, string>>({});
  const [suggestions, setSuggestions] = useState<SuggestionDto[]>([]);
  const [sessionModeState, setSessionModeState] = useState<SessionModeStateDto | null>(null);
  const [flowDebug, setFlowDebug] = useState<FlowDebugInfo>({
    decisionReason: "not_evaluated",
    suppressionState: "none",
    noOp: true,
    evidenceScore: null,
    modeReason: "Flow engine not evaluated yet.",
    retrievedChunks: []
  });
  const [suggestionActionMessage, setSuggestionActionMessage] = useState<string | null>(null);
  const [suggestionExplainById, setSuggestionExplainById] = useState<Record<number, string>>({});
  const [revisionDebug, setRevisionDebug] = useState<RevisionDebugInfo>({
    activeArtifactId: null,
    selectedStart: null,
    selectedEnd: null,
    selectionSource: "none",
    confidence: null,
    groundingNotes: "No revision proposal yet.",
    contextSource: "none",
    instructionSource: "none",
    retrievedChunks: []
  });
  const [subtaskDebug, setSubtaskDebug] = useState<SubTaskDebugInfo>({
    selectedSubtaskId: null,
    executionType: "none",
    instructionSource: "none",
    contextSnapshot: "No subtask snapshot selected.",
    retrievedChunks: []
  });

  const [newTaskTitle, setNewTaskTitle] = useState("");
  const [artifactPathInput, setArtifactPathInput] = useState("");
  const [chatInput, setChatInput] = useState("");

  // phase 13: workspace awareness state
  const [recentlyLearned, setRecentlyLearned] = useState<RecentlyLearnedItemDto[]>([]);
  const [recentlyLearnedOpen, setRecentlyLearnedOpen] = useState(false);
  const [watcherStatus, setWatcherStatus] = useState<WatcherStatusDto | null>(null);
  const [clipboardCaptureEnabled, setClipboardCaptureEnabled] = useState(false);

  // phase 15: proactive initiation state
  const [driftNotice, setDriftNotice] = useState<string | null>(null);
  const [speculativeSubtask, setSpeculativeSubtask] = useState<SubTaskDto | null>(null);
  const [quietMode, setQuietModeState] = useState(false);

  // phase 16: richer parallel work state
  const [fileWriteProposals, setFileWriteProposals] = useState<FileWriteProposalDto[]>([]);
  const [subtaskStepsById, setSubtaskStepsById] = useState<Record<number, SubTaskStepDto[]>>({});
  const [writeAuditLog, setWriteAuditLog] = useState<WriteAuditEntryDto[]>([]);

  // phase 20: active window context and document-switch nudge
  const [activeContext, setActiveContext] = useState<ActiveWindowContextDto | null>(null);
  const [docSwitchBanner, setDocSwitchBanner] = useState<{ app_name: string; document_title: string } | null>(null);
  const [accessibilityPermissionGranted, setAccessibilityPermissionGranted] = useState<boolean | null>(null);
  const [accessibilityPromptDismissed, setAccessibilityPromptDismissed] = useState(false);
  const docSwitchTimerRef = useRef<number | null>(null);

  // phase 21: privacy and trust control center
  const [privacyCenterOpen, setPrivacyCenterOpen] = useState(false);
  const [privacyDashboard, setPrivacyDashboard] = useState<PrivacyCenterDashboardDto | null>(null);
  const [speculationCache, setSpeculationCache] = useState<SpeculationCacheDto[]>([]);
  const [capabilityGaps, setCapabilityGaps] = useState<CapabilityGapDto[]>([]);
  const [customTools, setCustomTools] = useState<CustomToolDto[]>([]);
  const [toolConnections, setToolConnections] = useState<ToolConnectionDto[]>([]);
  const [toolCallLog, setToolCallLog] = useState<ToolCallLogDto[]>([]);
  const [webQueryLog, setWebQueryLog] = useState<WebQueryLogDto[]>([]);
  const [webUserGuard, setWebUserGuard] = useState("");
  const [emailReplyWatches, setEmailReplyWatches] = useState<EmailReplyWatchDto[]>([]);
  const [remoteDocs, setRemoteDocs] = useState<RemoteDocDto[]>([]);
  const [interruptionAudit, setInterruptionAudit] = useState<InterruptionAuditDto | null>(null);
  const [debriefEnabled, setDebriefEnabledState] = useState(false);
  const [voiceEnabled, setVoiceEnabledState] = useState(false);
  const [proactiveAuditLog, setProactiveAuditLog] = useState<ProactiveAuditEntryDto[]>([]);
  const [synthesisLog, setSynthesisLog] = useState<SynthesisLogEntryDto[]>([]);
  const [memoryFacts, setMemoryFacts] = useState<FactDto[]>([]);
  const [memoryEpisodes, setMemoryEpisodes] = useState<EpisodeDto[]>([]);
  const [memoryPromptPreview, setMemoryPromptPreview] = useState<string | null>(null);
  const [memoryConsolidationBusy, setMemoryConsolidationBusy] = useState(false);
  const [privacyActionMessage, setPrivacyActionMessage] = useState<string | null>(null);
  const [clearAllConfirmation, setClearAllConfirmation] = useState("");
  const [localModelUrl, setLocalModelUrl] = useState("");
  const [localModelSha256, setLocalModelSha256] = useState("");
  const [localModelExpectedBytes, setLocalModelExpectedBytes] = useState("");
  const [localModelBusy, setLocalModelBusy] = useState(false);

  // phase 22: explicit selected-text capture indicator.
  const [selectionCaptureIndicator, setSelectionCaptureIndicator] =
    useState<SelectionCaptureIndicatorDto | null>(null);
  const [selectionBridgeStatus, setSelectionBridgeStatus] = useState<SelectionBridgeStatusDto | null>(null);

  // phase 23: personalization signals ("Jeff remembers" panel)
  const [userProfileSignals, setUserProfileSignals] = useState<UserProfileSignalDto[]>([]);
  const [relationalProfile, setRelationalProfile] = useState<RelationalProfileDto | null>(null);
  const [jeffRemembersOpen, setJeffRemembersOpen] = useState(false);
  const [rubricInput, setRubricInput] = useState("");
  const activeStatedGoals = useMemo(
    () => (relationalProfile?.stated_goals ?? []).filter((goal) => goal.status === "active"),
    [relationalProfile]
  );
  const strugglePatterns = useMemo(
    () => relationalProfile?.struggle_patterns ?? [],
    [relationalProfile]
  );
  const rememberedSignalCount =
    userProfileSignals.length +
    activeStatedGoals.length +
    strugglePatterns.length +
    memoryFacts.length +
    memoryEpisodes.length;

  // phase 23: workload section
  const [workloadSummary, setWorkloadSummary] = useState<WorkloadSummaryDto | null>(null);
  const [workloadOpen, setWorkloadOpen] = useState(false);
  const [collisionNotice, setCollisionNotice] = useState<string | null>(null);

  // phase 23: calendar
  const [calendarEvent, setCalendarEvent] = useState<CalendarEventDto | null>(null);

  // phase 23: live app action pending edits
  const [pendingLiveEdits, setPendingLiveEdits] = useState<PendingLiveEditDto[]>([]);

  const [loading, setLoading] = useState(true);
  const [recording, setRecording] = useState(false);
  const [recordingPurpose, setRecordingPurpose] = useState<RecordingPurpose>("chat");
  const [coworkingStatus, setCoworkingStatus] = useState<CoworkingStatusDto | null>(null);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [onboardingStatus, setOnboardingStatus] = useState<OnboardingStatusDto | null>(null);
  const [workspacePromptDismissed, setWorkspacePromptDismissed] = useState(false);
  const [pendingActionKeys, setPendingActionKeys] = useState<Record<string, boolean>>({});
  const [expandedCompanionRevisionId, setExpandedCompanionRevisionId] = useState<number | null>(null);
  const [expandedCompanionSubtaskId, setExpandedCompanionSubtaskId] = useState<number | null>(null);
  const [lastRoutedIntent, setLastRoutedIntent] = useState<RoutedIntent>("answer");

  // phase 12: streaming state. keyed by turn_id so concurrent turns (e.g.
  // a subtask and a main chat turn) render independently.
  const [streamingTurnId, setStreamingTurnId] = useState<string | null>(null);
  const [streamingText, setStreamingText] = useState<Record<string, string>>({});
  const streamingTurnIdRef = useRef<string | null>(null);

  // phase 12: streaming tts queue. ordered by phrase_id so chunks play in
  // order even when synthesized concurrently and delivered out of order.
  const ttsActiveTurnIdRef = useRef<string | null>(null);
  const streamTtsQueueRef = useRef<Map<number, { audio: HTMLAudioElement; url: string }>>(new Map());
  const streamTtsNextPhraseRef = useRef<number>(1);
  const streamTtsCurrentRef = useRef<HTMLAudioElement | null>(null);
  const streamTtsDelayTimerRef = useRef<number | null>(null);
  const speechDelayTimerRef = useRef<number | null>(null);
  const userIsTypingRef = useRef(false);
  const typingRateTimestampsRef = useRef<number[]>([]);

  // phase 12: partial stt via web speech api.
  const speechRecognitionRef = useRef<PartialSpeechRecognition | null>(null);
  const partialSttSentRef = useRef<boolean>(false);

  const mediaRecorderRef = useRef<MediaRecorder | null>(null);
  const mediaStreamRef = useRef<MediaStream | null>(null);
  const audioChunksRef = useRef<Blob[]>([]);
  const recordingPurposeRef = useRef<RecordingPurpose>("chat");

  const docSwitchTaskCandidates = useMemo(
    () => tasks.filter((task) => !activeTask || task.id !== activeTask.id).slice(0, 3),
    [activeTask, tasks]
  );

  const ttsAudioRef = useRef<HTMLAudioElement | null>(null);
  const ttsObjectUrlRef = useRef<string | null>(null);
  const sendRequestIdRef = useRef(0);
  const proactiveEvaluationInFlightRef = useRef(false);
  const suggestionEvaluationInFlightRef = useRef(false);
  const typingTimeoutRef = useRef<number | null>(null);
  const artifactTextareaRef = useRef<HTMLTextAreaElement | null>(null);
  const activeWatcherTaskIdRef = useRef<number | null>(null);

  const activeTaskLabel = useMemo(() => activeTask?.title ?? "No active task", [activeTask]);

  function updateTrayStatus(status: TrayStatus) {
    void setAmbientTrayStatus(status).catch(() => undefined);
  }

  const handleToggleDebugPanels = useCallback(() => {
    setShowDebugPanels((current) => {
      const next = !current;
      try {
        window.localStorage.setItem(DEBUG_PANELS_STORAGE_KEY, next ? "1" : "0");
      } catch {
        // ignore storage failures; the visible state still updates.
      }
      return next;
    });
  }, []);

  const activeSubtasks = useMemo(
    () => subtasks.filter((subtask) => subtask.status === "pending" || subtask.status === "running"),
    [subtasks]
  );
  const completedSubtasks = useMemo(
    () => subtasks.filter((subtask) => subtask.status !== "pending" && subtask.status !== "running"),
    [subtasks]
  );
  const completedSubtasksAwaitingReview = useMemo(
    () =>
      completedSubtasks.filter(
        (subtask) => subtask.status === "completed" && subtask.result_review_status === "unreviewed"
      ),
    [completedSubtasks]
  );
  const pendingSuggestions = useMemo(
    () => suggestions.filter((suggestion) => suggestion.status === "pending"),
    [suggestions]
  );
  const topPendingSuggestion = useMemo(
    () => pendingSuggestions[0] ?? null,
    [pendingSuggestions]
  );

  useEffect(() => {
    void refreshShellState();

    return () => {
      if (typingTimeoutRef.current !== null) {
        window.clearTimeout(typingTimeoutRef.current);
        typingTimeoutRef.current = null;
      }
      if (streamTtsDelayTimerRef.current !== null) {
        window.clearTimeout(streamTtsDelayTimerRef.current);
        streamTtsDelayTimerRef.current = null;
      }
      if (speechDelayTimerRef.current !== null) {
        window.clearTimeout(speechDelayTimerRef.current);
        speechDelayTimerRef.current = null;
      }

      stopSpeechPlayback();
      stopStreamingTtsPlayback();
      stopPartialStt();
      stopMicrophoneStream();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // phase 12: subscribe to streaming llm events. subscriptions are
  // unconditional (set up once) but the handlers gate on streamingTurnId
  // so they are no-ops when no streaming turn is active.
  useEffect(() => {
    if (!isStreamingEnabled()) return;

    const unlisteners: Promise<() => void>[] = [];

    unlisteners.push(
      listen<LlmTokenPayload>(EVENT_LLM_TOKEN, (event) => {
        const { turn_id, delta } = event.payload;
        if (streamingTurnIdRef.current !== turn_id) return;
        setStreamingText((prev) => ({
          ...prev,
          [turn_id]: (prev[turn_id] ?? "") + delta,
        }));
      })
    );

    unlisteners.push(
      listen<LlmCompletePayload>(EVENT_LLM_COMPLETE, (event) => {
        const { turn_id } = event.payload;
        if (streamingTurnIdRef.current !== turn_id) return;
        // refresh the messages list so the finalized db row replaces the
        // streaming accumulator in the ui.
        void (async () => {
          if (!activeTask) return;
          const messageList = await listMessages(activeTask.id);
          setMessages(messageList);
          await refreshActionCenterState(activeTask.id);
        })();
      })
    );

    unlisteners.push(
      listen<TurnCancelledPayload>(EVENT_TURN_CANCELLED, (event) => {
        const { turn_id, reason } = event.payload;
        if (streamingTurnIdRef.current !== turn_id) return;
        stopStreamingTtsPlayback();
        streamingTurnIdRef.current = null;
        setStreamingTurnId(null);
        setStreamingText((prev) => {
          const next = { ...prev };
          delete next[turn_id];
          return next;
        });
        const errMsg = extractStreamCancelError(reason ?? "");
        if (errMsg) {
          setErrorMessage(mapJeffErrorMessage(errMsg));
        }
        updateTrayStatus("idle");
        void (async () => {
          if (!activeTask) return;
          const messageList = await listMessages(activeTask.id);
          setMessages(messageList);
        })();
      })
    );

    unlisteners.push(
      listen<TurnCompletePayload>(EVENT_TURN_COMPLETE, (event) => {
        const { turn_id } = event.payload;
        if (streamingTurnIdRef.current !== turn_id) return;
        streamingTurnIdRef.current = null;
        setStreamingTurnId(null);
        setStreamingText((prev) => {
          const next = { ...prev };
          delete next[turn_id];
          return next;
        });
        // ttsActiveTurnIdRef stays set so late-arriving tts chunks still play.
        if (streamTtsCurrentRef.current === null && streamTtsQueueRef.current.size === 0) {
          updateTrayStatus("idle");
        }
      })
    );

    // tts_chunk events arrive concurrently with and after llm_complete.
    // gated on ttsActiveTurnIdRef (not streamingTurnIdRef) so chunks that
    // arrive after turn_complete still get played.
    unlisteners.push(
      listen<TtsChunkPayload>(EVENT_TTS_CHUNK, (event) => {
        const { turn_id, phrase_id, audio_b64 } = event.payload;
        if (ttsActiveTurnIdRef.current !== turn_id) return;
        const blob = base64ToBlob(audio_b64, "audio/mpeg");
        const url = URL.createObjectURL(blob);
        const audio = new Audio(url);
        streamTtsQueueRef.current.set(phrase_id, { audio, url });
        scheduleStreamTtsPlayback();
      })
    );

    return () => {
      unlisteners.forEach((p) =>
        p.then((unlisten) => unlisten()).catch(() => undefined)
      );
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeTask?.id]);

  useEffect(() => {
    if (!activeTask || viewMode !== "workspace") {
      return;
    }

    let cancelled = false;
    const intervalId = window.setInterval(() => {
      if (cancelled || proactiveEvaluationInFlightRef.current) {
        return;
      }

      proactiveEvaluationInFlightRef.current = true;
      void runProactiveEvaluation(activeTask.id)
        .catch((error) => {
          if (!cancelled) {
            setErrorMessage(formatError(error));
          }
        })
        .finally(() => {
          proactiveEvaluationInFlightRef.current = false;
        });
    }, 2500);

    return () => {
      cancelled = true;
      window.clearInterval(intervalId);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeTask?.id, viewMode]);

  useEffect(() => {
    if (!activeTask || viewMode !== "workspace") {
      return;
    }

    let cancelled = false;
    const intervalId = window.setInterval(() => {
      if (cancelled) {
        return;
      }

      void refreshSubtasks(activeTask.id).catch(() => undefined);
    }, 1400);

    return () => {
      cancelled = true;
      window.clearInterval(intervalId);
    };
  }, [activeTask?.id, viewMode]);

  // phase 16: poll file write proposals and step state for running subtasks.
  // runs in both companion and workspace views so approval cards appear anywhere.
  useEffect(() => {
    if (!activeTask) {
      setFileWriteProposals([]);
      setSubtaskStepsById({});
      return;
    }

    let cancelled = false;
    const intervalId = window.setInterval(() => {
      if (cancelled) return;
      void listFileWriteProposals(activeTask.id)
        .then((proposals) => { if (!cancelled) setFileWriteProposals(proposals); })
        .catch(() => undefined);
    }, 3000);

    return () => {
      cancelled = true;
      window.clearInterval(intervalId);
    };
  }, [activeTask?.id]);

  // refresh step list whenever running subtasks change
  useEffect(() => {
    if (!activeTask) return;
    const running = subtasks.filter((s) => s.status === "running" || s.status === "pending");
    if (running.length === 0) return;

    let cancelled = false;
    const intervalId = window.setInterval(() => {
      if (cancelled) return;
      running.forEach((subtask) => {
        void listSubtaskSteps(activeTask.id, subtask.subtask_id)
          .then((steps) => {
            if (!cancelled) {
              setSubtaskStepsById((prev) => ({ ...prev, [subtask.subtask_id]: steps }));
            }
          })
          .catch(() => undefined);
      });
    }, 1500);

    return () => {
      cancelled = true;
      window.clearInterval(intervalId);
    };
  }, [activeTask?.id, subtasks]);

  useEffect(() => {
    if (!activeTask || viewMode !== "workspace") {
      return;
    }

    let cancelled = false;
    const intervalId = window.setInterval(() => {
      if (cancelled) {
        return;
      }

      void refreshActionCenterState(activeTask.id).catch(() => undefined);
    }, 2300);

    return () => {
      cancelled = true;
      window.clearInterval(intervalId);
    };
  }, [activeTask?.id, viewMode]);

  // phase 13: load recently learned + watcher state whenever the active task changes.
  useEffect(() => {
    const priorTaskId = activeWatcherTaskIdRef.current;
    if (!activeTask) {
      if (priorTaskId !== null) {
        void stopWorkspaceWatcher(priorTaskId).catch(() => undefined);
        activeWatcherTaskIdRef.current = null;
      }
      setRecentlyLearned([]);
      setWatcherStatus(null);
      setClipboardCaptureEnabled(false);
      return;
    }

    if (priorTaskId !== null && priorTaskId !== activeTask.id) {
      void stopWorkspaceWatcher(priorTaskId).catch(() => undefined);
    }
    activeWatcherTaskIdRef.current = activeTask.id;

    void refreshRecentlyLearned(activeTask.id, { ensureWatcher: true }).catch(() => undefined);
  }, [activeTask?.id]);

  // phase 27: foreground focus only records presence. Proactive speech is owned
  // by the synthesis monitor so resume, drift, and stuck signals are not split.
  useEffect(() => {
    const handleFocus = async () => {
      if (!activeTask) return;
      const taskId = activeTask.id;

      void recordTaskFocus(taskId).catch(() => undefined);
    };

    window.addEventListener("focus", handleFocus);
    return () => window.removeEventListener("focus", handleFocus);
  }, [activeTask?.id]);

  useEffect(() => {
    if (!activeTask || viewMode !== "workspace") {
      return;
    }

    let cancelled = false;
    const intervalId = window.setInterval(() => {
      if (cancelled || suggestionEvaluationInFlightRef.current) {
        return;
      }

      suggestionEvaluationInFlightRef.current = true;
      void runSuggestionEvaluation(activeTask.id)
        .catch((error) => {
          if (!cancelled) {
            setErrorMessage(formatError(error));
          }
        })
        .finally(() => {
          suggestionEvaluationInFlightRef.current = false;
        });
    }, 3200);

    return () => {
      cancelled = true;
      window.clearInterval(intervalId);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeTask?.id, viewMode, selectedArtifactId]);

  // phase 20: subscribe to backend context://context-updated events.
  // the backend emits this after every 3-second poll so no client-side interval
  // is needed. fetch once on mount for the initial state before the first event.
  useEffect(() => {
    let cancelled = false;
    void getActiveWindowContext()
      .then((ctx) => { if (!cancelled) setActiveContext(ctx); })
      .catch(() => undefined);
    const unsub = listen<ActiveWindowContextDto | null>(
      "context://context-updated",
      (event) => {
        if (!cancelled) setActiveContext(event.payload ?? null);
      }
    );
    return () => {
      cancelled = true;
      unsub.then((fn) => fn()).catch(() => undefined);
    };
  }, []);

  // phase 20: check accessibility status without showing the macOS prompt.
  useEffect(() => {
    let cancelled = false;
    const refreshPermission = async () => {
      try {
        const granted = await getAccessibilityPermissionStatus();
        if (!cancelled) {
          setAccessibilityPermissionGranted(granted);
        }
      } catch {
        if (!cancelled) {
          setAccessibilityPermissionGranted(false);
        }
      }
    };
    void refreshPermission();
    const id = window.setInterval(() => void refreshPermission(), 10000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, []);

  // phase 20: subscribe to document-switch nudge events from the backend.
  useEffect(() => {
    const unsubscribe = listen<{ app_name: string; document_title: string }>(
      "context://document-switch",
      (event) => {
        setDocSwitchBanner(event.payload);
        if (docSwitchTimerRef.current !== null) {
          window.clearTimeout(docSwitchTimerRef.current);
        }
        docSwitchTimerRef.current = window.setTimeout(() => {
          setDocSwitchBanner(null);
          docSwitchTimerRef.current = null;
        }, 8000);
      }
    );
    return () => {
      unsubscribe.then((fn) => fn()).catch(() => undefined);
      if (docSwitchTimerRef.current !== null) {
        window.clearTimeout(docSwitchTimerRef.current);
      }
    };
  }, []);

  // phase 21: tray entry opens the same Privacy Center as companion settings.
  useEffect(() => {
    const unsubscribe = listen("privacy://open", () => {
      setPrivacyCenterOpen(true);
      void refreshPrivacyCenter();
    });
    return () => {
      unsubscribe.then((fn) => fn()).catch(() => undefined);
    };
  }, []);

  // phase 22: selection-capture state is event-driven but also loaded on start
  // so an indicator survives frontend reload while the backend process lives.
  useEffect(() => {
    let cancelled = false;
    void getSelectionCaptureIndicator()
      .then((indicator) => {
        if (!cancelled) setSelectionCaptureIndicator(indicator);
      })
      .catch(() => undefined);

    const unlisteners = [
      listen<SelectionCaptureIndicatorDto>("selection://captured", (event) => {
        setSelectionCaptureIndicator(event.payload);
      }),
      listen<SelectionCaptureIndicatorDto>("selection://capture-failed", (event) => {
        setSelectionCaptureIndicator(event.payload);
      }),
      listen("selection://cleared", () => {
        setSelectionCaptureIndicator(null);
      })
    ];

    return () => {
      cancelled = true;
      unlisteners.forEach((promise) => promise.then((fn) => fn()).catch(() => undefined));
    };
  }, []);

  // phase 22: backend global monitor emits only a typing boolean. the frontend
  // uses it exclusively to delay/suppress tts playback.
  useEffect(() => {
    const unsubscribe = listen<{
      is_typing: boolean;
      rate_only: boolean;
      monitor_available: boolean;
      last_error: string | null;
    }>("typing://activity-changed", (event) => {
      const isTyping =
        privacyDashboard?.typing_activity_enabled !== false && Boolean(event.payload.is_typing);
      userIsTypingRef.current = isTyping;
      if (!isTyping) {
        scheduleStreamTtsPlayback();
      }
    });
    return () => {
      unsubscribe.then((fn) => fn()).catch(() => undefined);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [privacyDashboard?.typing_activity_enabled]);

  // phase 22 fallback for platforms where global key-rate events are not
  // available: track only keydown timing inside Jeff's own UI.
  useEffect(() => {
    const onKeyDown = () => {
      noteRateOnlyKeydown();
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [privacyDashboard?.typing_activity_enabled, activeTask?.id]);

  // phase 23: load personalization signals + workload + calendar on mount and
  // when the companion section opens.
  useEffect(() => {
    void getUserProfileSignals()
      .then(setUserProfileSignals)
      .catch(() => undefined);
    void getRelationalProfile()
      .then(setRelationalProfile)
      .catch(() => undefined);
    void getWorkloadSummary()
      .then(setWorkloadSummary)
      .catch(() => undefined);
    void getCalendarNextEvent()
      .then(setCalendarEvent)
      .catch(() => undefined);
    void getPendingLiveEdits()
      .then(setPendingLiveEdits)
      .catch(() => undefined);
  }, [activeTask?.id]);

  // phase 23: listen for live_action events from the backend bridge
  useEffect(() => {
    const unlisten = listen<{ receipt_id: number }>(
      "live_action://apply_requested",
      (_event) => {
        void getPendingLiveEdits()
          .then(setPendingLiveEdits)
          .catch(() => undefined);
      }
    );
    const unlistenApproved = listen<{ receipt_id: number }>(
      "live_action://approved",
      (_event) => {
        void getPendingLiveEdits()
          .then(setPendingLiveEdits)
          .catch(() => undefined);
      }
    );
    const unlistenFallback = listen<{ receipt_id: number }>(
      "live_action://fallback_triggered",
      (_event) => {
        void getPendingLiveEdits()
          .then(setPendingLiveEdits)
          .catch(() => undefined);
      }
    );
    const unlistenResult = listen<{ receipt_id: number; status: string }>(
      "live_action://result",
      (_event) => {
        void getPendingLiveEdits()
          .then(setPendingLiveEdits)
          .catch(() => undefined);
      }
    );
    const unlistenCalendar = listen<CalendarEventDto | null>(
      "calendar://event-updated",
      (event) => {
        setCalendarEvent(event.payload ?? null);
      }
    );
    const unlistenCollision = listen<{ matching_task_title: string; similarity_score: number }>(
      "subtask://collision-detected",
      (event) => {
        setCollisionNotice(
          `Similar work exists in "${event.payload.matching_task_title}" — want me to pull it in?`
        );
      }
    );
    // when the overlay creates or switches a task, refresh so the workspace
    // picks up the new active task without requiring a manual reload.
    const unlistenActiveChanged = listen<{ task_id: number }>(
      "task://active-changed",
      () => {
        void refreshShellState();
      }
    );
    return () => {
      void Promise.all([
        unlisten,
        unlistenApproved,
        unlistenFallback,
        unlistenResult,
        unlistenCalendar,
        unlistenCollision,
        unlistenActiveChanged,
      ]).then((unlisteners) => unlisteners.forEach((u) => u()));
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function refreshShellState() {
    setLoading(true);
    setErrorMessage(null);

    try {
      const [persistedTasks, persistedActiveTask, status, onboarding, promptDismissed] =
        await Promise.all([
          listTasks(),
          getActiveTask(),
          getCoworkingStatus(),
          getOnboardingStatus(),
          getWorkspacePromptDismissed()
        ]);

      setTasks(persistedTasks);
      setActiveTaskState(persistedActiveTask);
      setCoworkingStatus(status);
      setOnboardingStatus(onboarding);
      // reset the dismissal if the user later adds a folder
      if (onboarding.preferred_workspace_folder) {
        setWorkspacePromptDismissed(false);
      } else {
        setWorkspacePromptDismissed(promptDismissed);
      }

      if (persistedActiveTask) {
        await loadWorkspaceState(persistedActiveTask.id);
      } else {
        clearWorkspaceState();
      }
      await refreshPrivacyCenter();
    } catch (error) {
      setErrorMessage(formatError(error));
    } finally {
      setLoading(false);
    }
  }

  async function refreshCoworkingStatus() {
    try {
      const status = await getCoworkingStatus();
      setCoworkingStatus(status);
    } catch {
      // status refresh errors should not break interaction flow
    }
  }

  async function refreshPrivacyCenter() {
    try {
      const [dashboard, bridgeStatus, profile, audit, debrief, voiceConfig, speculation, gaps, tools] =
        await Promise.all([
          getPrivacyCenterDashboard(),
          getSelectionBridgeStatus(),
          getRelationalProfile(),
          getInterruptionAudit().catch(() => null),
          getDebriefEnabled().catch(() => false),
          getVoiceConfig().catch(() => null),
          listSpeculationCache(8).catch(() => []),
          listCapabilityGaps().catch(() => []),
          listCustomTools().catch(() => [])
        ]);
      const [connections, callLog, webLog, replyWatches, remotes] = await Promise.all([
        listToolConnections().catch(() => []),
        listToolCallLog(8).catch(() => []),
        listWebQueryLog(8).catch(() => []),
        listEmailReplyWatches().catch(() => []),
        listRemoteDocs().catch(() => [])
      ]);
      setPrivacyDashboard(dashboard);
      setSpeculationCache(speculation);
      setCapabilityGaps(gaps);
      setCustomTools(tools);
      setToolConnections(connections);
      setToolCallLog(callLog);
      setWebQueryLog(webLog);
      setEmailReplyWatches(replyWatches);
      setRemoteDocs(remotes);
      setSelectionBridgeStatus(bridgeStatus);
      setRelationalProfile(profile);
      setInterruptionAudit(audit);
      setDebriefEnabledState(debrief);
      setVoiceEnabledState(voiceConfig?.enabled ?? false);
      setClipboardCaptureEnabled(dashboard.clipboard_capture_enabled);
      setAccessibilityPermissionGranted(dashboard.accessibility_permission_status === "granted");
      setQuietModeState(!dashboard.proactive_triggers_enabled);
      if (!dashboard.user_profile_memory_enabled) {
        setUserProfileSignals([]);
        setRelationalProfile(null);
        setMemoryFacts([]);
        setMemoryEpisodes([]);
        setMemoryPromptPreview(null);
      } else {
        const [facts, preview] = await Promise.all([
          listFacts(100),
          previewMemoryPromptContext()
        ]);
        setMemoryFacts(facts);
        setMemoryPromptPreview(preview.prompt_context);
      }
      if (!dashboard.calendar_context_enabled) {
        setCalendarEvent(null);
      }
      if (dashboard.active_task_id !== null) {
        const [triggerLog, writeLog, synthesisEntries, episodes] = await Promise.all([
          listProactiveTriggerAuditLog(dashboard.active_task_id),
          listWriteAuditLog(dashboard.active_task_id),
          getSynthesisLog(dashboard.active_task_id),
          dashboard.user_profile_memory_enabled ? listEpisodes(dashboard.active_task_id, 50) : Promise.resolve([])
        ]);
        setProactiveAuditLog(triggerLog);
        setWriteAuditLog(writeLog);
        setSynthesisLog(synthesisEntries);
        setMemoryEpisodes(episodes);
      } else {
        setProactiveAuditLog([]);
        setWriteAuditLog([]);
        setSynthesisLog([]);
        setMemoryEpisodes([]);
      }
    } catch (error) {
      setOperationError("Failed to refresh Privacy Center", error);
    }
  }

  async function loadWorkspaceState(taskId: number) {
    const [
      summary,
      workspace,
      resources,
      artifactList,
      messageList,
      subtaskList,
      jobList,
      standingJobList,
      suggestionList,
      modeState,
      taskPending,
      eventList,
      persistedActiveArtifactId
    ] = await Promise.all([
      getTaskSummary(taskId),
      getTaskWorkspace(taskId),
      listOpenResources(taskId),
      listArtifacts(taskId),
      listMessages(taskId),
      listSubtasks(taskId),
      listAgentJobs(taskId, 50),
      listStandingJobs(taskId),
      listSuggestions(taskId),
      getSessionModeState(taskId),
      listTaskPendingRevisions(taskId),
      listRecentEvents(taskId, 18),
      getActiveArtifactSelection(taskId)
    ]);

    setTaskSummary(summary);
    setWorkspaceInfo(workspace);
    setOpenResources(resources);
    setArtifacts(artifactList);
    setMessages(messageList);
    setSubtasks(subtaskList);
    setAgentJobs(jobList);
    setStandingJobs(standingJobList);
    if (jobList.length > 0 && !selectedAgentJob) {
      void getAgentJobDetail(jobList[0].id).then(setSelectedAgentJob).catch(() => undefined);
    }
    setSuggestions(suggestionList);
    setSessionModeState(modeState);
    setTaskPendingRevisions(taskPending);
    setRecentEvents(eventList);

    const preferredArtifactId = persistedActiveArtifactId ?? modeState?.active_artifact_id ?? selectedArtifactId;
    const nextArtifactId = pickNextEditableArtifactId(artifactList, preferredArtifactId);
    setSelectedArtifactId(nextArtifactId);

    if (nextArtifactId !== null) {
      if (persistedActiveArtifactId !== nextArtifactId) {
        await setActiveArtifactSelection(taskId, nextArtifactId);
      }
      await loadArtifactRevisionState(taskId, nextArtifactId);
    } else {
      await setActiveArtifactSelection(taskId, null);
      clearArtifactRevisionState();
    }

    if (subtaskList.length === 0) {
      setSubtaskDebug({
        selectedSubtaskId: null,
        executionType: "none",
        instructionSource: "none",
        contextSnapshot: "No subtask snapshot selected.",
        retrievedChunks: []
      });
    }
  }

  async function refreshSubtasks(taskId: number) {
    const latestSubtasks = await listSubtasks(taskId);
    setSubtasks(latestSubtasks);
  }

  async function refreshSuggestions(taskId: number) {
    const latestSuggestions = await listSuggestions(taskId);
    setSuggestions(latestSuggestions);
  }

  async function refreshActionCenterState(taskId: number) {
    const [taskPending, eventList, suggestionList, subtaskList, jobList, standingJobList] = await Promise.all([
      listTaskPendingRevisions(taskId),
      listRecentEvents(taskId, 18),
      listSuggestions(taskId),
      listSubtasks(taskId),
      listAgentJobs(taskId, 50),
      listStandingJobs(taskId)
    ]);

    setTaskPendingRevisions(taskPending);
    setRecentEvents(eventList);
    setSuggestions(suggestionList);
    setSubtasks(subtaskList);
    setAgentJobs(jobList);
    setStandingJobs(standingJobList);
    if (selectedAgentJob) {
      const stillPresent = jobList.some((job) => job.id === selectedAgentJob.job.id);
      if (stillPresent) {
        void getAgentJobDetail(selectedAgentJob.job.id).then(setSelectedAgentJob).catch(() => undefined);
      }
    }
  }

  // phase 13: load recently learned items and watcher/clipboard state for a task.
  async function refreshRecentlyLearned(taskId: number, options?: { ensureWatcher?: boolean }) {
    try {
      const [items, statusSnapshot, cbEnabled] = await Promise.all([
        listRecentlyLearned(taskId, 10),
        getWatcherStatus(taskId),
        getClipboardCaptureSetting(taskId)
      ]);

      let status = statusSnapshot;
      if (options?.ensureWatcher && !status.is_watching) {
        status = await ensureWorkspaceWatcher(taskId);
      }

      setRecentlyLearned(items);
      setWatcherStatus(status);
      setClipboardCaptureEnabled(cbEnabled);
    } catch {
      // non-fatal — recently learned is informational only
    }
  }

  async function loadArtifactRevisionState(taskId: number, artifactId: number) {
    const [content, pending, versions, taskPending] = await Promise.all([
      getArtifactContent(artifactId),
      listPendingRevisions(taskId, artifactId),
      listArtifactVersions(artifactId),
      listTaskPendingRevisions(taskId)
    ]);

    setArtifactContent(content);
    setPendingRevisions(pending);
    setTaskPendingRevisions(taskPending);
    setArtifactVersions(versions);
    setArtifactSelectionStart(0);
    setArtifactSelectionEnd(0);

    setRevisionDebug((current) => ({
      ...current,
      activeArtifactId: artifactId
    }));
  }

  function clearWorkspaceState() {
    setFullWorkspaceVisible(false);
    setTaskSummary(null);
    setWorkspaceInfo(null);
    setOpenResources([]);
    setArtifacts([]);
    setMessages([]);
    setSubtasks([]);
    setAgentJobs([]);
    setSelectedAgentJob(null);
    setSuggestions([]);
    setSessionModeState(null);
    setTaskPendingRevisions([]);
    setRecentEvents([]);
    setSubtaskInstruction("");
    setSubtaskSuggestion(null);
    setSuggestionActionMessage(null);
    setSuggestionExplainById({});
    setSubtaskRefinementInputById({});
    setPendingActionKeys({});
    setRetrievalDebugChunks([]);
    setRetrievalDebugMeta({
      decisionEventType: "system_status_event",
      decisionReason: "awaiting_activity",
      confidence: null
    });
    setSubtaskDebug({
      selectedSubtaskId: null,
      executionType: "none",
      instructionSource: "none",
      contextSnapshot: "No subtask snapshot selected.",
      retrievedChunks: []
    });
    setFlowDebug({
      decisionReason: "not_evaluated",
      suppressionState: "none",
      noOp: true,
      evidenceScore: null,
      modeReason: "Flow engine not evaluated yet.",
      retrievedChunks: []
    });
    clearArtifactRevisionState();
  }

  function clearArtifactRevisionState() {
    setSelectedArtifactId(null);
    setArtifactContent(null);
    setArtifactSelectionStart(0);
    setArtifactSelectionEnd(0);
    setRevisionInstruction("");
    setPendingRevisions([]);
    setArtifactVersions([]);
    setRevisionDebug({
      activeArtifactId: null,
      selectedStart: null,
      selectedEnd: null,
      selectionSource: "none",
      confidence: null,
      groundingNotes: "No revision proposal yet.",
      contextSource: "none",
      instructionSource: "none",
      retrievedChunks: []
    });
  }

  async function handleCreateTask(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const trimmedTitle = newTaskTitle.trim();
    if (!trimmedTitle) {
      return;
    }

    setErrorMessage(null);

    try {
      await createTask(trimmedTitle);
      setNewTaskTitle("");
      await refreshShellState();
    } catch (error) {
      setErrorMessage(formatError(error));
    }
  }

  async function handleStartTaskFromPrompt(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const prompt = chatInput.trim();
    if (!prompt) {
      return;
    }

    setErrorMessage(null);

    try {
      const created = await createTask(deriveTaskTitleFromPrompt(prompt));
      const nextActiveTask = await setActiveTask(created.id);
      setActiveTaskState(nextActiveTask);
      setTasks(await listTasks());
      await sendMessage(nextActiveTask.id, prompt, "text");
      await loadWorkspaceState(nextActiveTask.id);
      await refreshCoworkingStatus();
      setChatInput("");
      setFullWorkspaceVisible(false);
      setViewMode("workspace");
    } catch (error) {
      setErrorMessage(formatError(error));
    }
  }

  async function handleOpenOnboarding(step?: number) {
    try {
      if (step !== undefined) {
        await openOnboardingAtStep(step);
      } else {
        await openOnboarding();
      }
    } catch (error) {
      setErrorMessage(formatError(error));
    }
  }

  async function handleDismissWorkspacePrompt() {
    setWorkspacePromptDismissed(true);
    try {
      await persistWorkspacePromptDismissed(true);
    } catch {
      // persist failure is non-fatal; local state already hides the prompt.
    }
  }

  async function handleSetActiveTask(taskId: number) {
    setErrorMessage(null);

    try {
      const nextActiveTask = await setActiveTask(taskId);
      const updatedTasks = await listTasks();
      setTasks(updatedTasks);
      setActiveTaskState(nextActiveTask);
      await loadWorkspaceState(taskId);
      await refreshCoworkingStatus();
      setFullWorkspaceVisible(false);
      setViewMode("workspace");
    } catch (error) {
      setErrorMessage(formatError(error));
    }
  }

  async function handleStartTaskFromDocumentTitle(documentTitle: string) {
    const title = deriveTaskTitleFromPrompt(documentTitle);
    setErrorMessage(null);

    try {
      const created = await createTask(title);
      const nextActiveTask = await setActiveTask(created.id);
      setActiveTaskState(nextActiveTask);
      setTasks(await listTasks());
      await loadWorkspaceState(nextActiveTask.id);
      await refreshCoworkingStatus();
      setDocSwitchBanner(null);
      setFullWorkspaceVisible(false);
      setViewMode("workspace");
    } catch (error) {
      setErrorMessage(formatError(error));
    }
  }

  async function handleRequestAccessibilityPermission() {
    setErrorMessage(null);

    try {
      await requestAccessibilityPermission();
      window.setTimeout(() => {
        getAccessibilityPermissionStatus()
          .then((granted) => setAccessibilityPermissionGranted(granted))
          .catch(() => setAccessibilityPermissionGranted(false));
      }, 800);
    } catch (error) {
      setErrorMessage(formatError(error));
    }
  }

  async function handleRequestCalendarPermission() {
    setErrorMessage(null);

    try {
      await requestCalendarPermission();
      window.setTimeout(() => {
        void refreshPrivacyCenter();
        void getCalendarNextEvent().then(setCalendarEvent).catch(() => undefined);
      }, 800);
    } catch (error) {
      setErrorMessage(formatError(error));
    }
  }

  async function handleContinueTask() {
    if (!activeTask) {
      return;
    }

    setErrorMessage(null);

    try {
      await loadWorkspaceState(activeTask.id);
      await refreshCoworkingStatus();
      setFullWorkspaceVisible(false);
      setViewMode("workspace");
    } catch (error) {
      setErrorMessage(formatError(error));
    }
  }

  async function handleImportArtifact(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();

    if (!activeTask) {
      setErrorMessage("Select an active task before importing artifacts.");
      return;
    }

    const filePath = artifactPathInput.trim();
    if (!filePath) {
      return;
    }

    setErrorMessage(null);

    try {
      await importArtifact(activeTask.id, filePath);
      setArtifactPathInput("");
      await loadWorkspaceState(activeTask.id);
    } catch (error) {
      setErrorMessage(formatError(error));
    }
  }

  async function handleSelectArtifact(artifactId: number) {
    if (!activeTask) {
      return;
    }

    setSelectedArtifactId(artifactId);

    try {
      await setActiveArtifactSelection(activeTask.id, artifactId);
      await loadArtifactRevisionState(activeTask.id, artifactId);
      await refreshSuggestions(activeTask.id);
      setSessionModeState(await getSessionModeState(activeTask.id));
    } catch (error) {
      setErrorMessage(formatError(error));
    }
  }

  async function handleSendTextMessage(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const message = chatInput.trim();
    if (!message || !activeTask) {
      return;
    }

    clearTypingSignal();
    setChatInput("");
    await submitRoutedMessage(activeTask.id, message, "text");
  }

  async function submitRoutedMessage(taskId: number, message: string, source: "text" | "voice") {
    const { intent, slots } = await classifyMessageIntentWithFallback(taskId, message);
    setLastRoutedIntent(intent);

    if (intent === "unknown") {
      setErrorMessage(
        "Jeff needs clarification: do you want an answer, a revision, a bounded subtask, or suggestions?"
      );
      updateTrayStatus("idle");
      return;
    }

    const routedIntent: RoutedIntent = intent;

    try {
      if (routedIntent === "revision") {
        await submitMessage(taskId, message, source);
        await autoCreateRevisionFromIntent(taskId, message, source, slots);
      } else if (routedIntent === "subtask") {
        await submitMessage(taskId, message, source);
        await autoCreateSubtaskFromIntent(taskId, message, source, slots);
      } else if (routedIntent === "suggestion") {
        await submitMessage(taskId, message, source);
        await handleRefreshSuggestions();
      } else {
        await submitMessage(taskId, message, source);
      }
    } catch (error) {
      setOperationError("Conversation routing failed", error);
      return;
    }

    // phase 15: fire drift detection after send (non-blocking, 1s delay)
    window.setTimeout(() => {
      void checkTaskDrift(taskId, message)
        .then((result: DriftFlagDto) => {
          if (result.is_drifting && result.confidence > 0.6) {
            setDriftNotice(result.flag_reason || "Your current writing may be drifting from the task goal.");
          }
        })
        .catch(() => undefined);
    }, 1000);
  }

  async function autoCreateRevisionFromIntent(
    taskId: number,
    message: string,
    source: "text" | "voice",
    slots?: IntentSlotsDto | null
  ) {
    const targetArtifactId =
      selectedArtifactId ?? pickNextEditableArtifactId(artifacts, sessionModeState?.active_artifact_id ?? null);

    if (!targetArtifactId) {
      setErrorMessage(
        "Jeff understood this as a revision request but no editable artifact is selected. Open full workspace to choose one."
      );
      return;
    }

    const instruction = buildRevisionInstruction(message, slots);
    const hasExplicitSelection = artifactSelectionStart !== artifactSelectionEnd;
    const selectedRange = hasExplicitSelection ? currentRevisionTarget() : null;
    const resolvedTarget = deriveRevisionTargetFromDescription(
      artifactContent?.content ?? null,
      slots?.target_description ?? null,
      selectedRange
    );

    const result = await proposeArtifactRevision(
      taskId,
      targetArtifactId,
      resolvedTarget,
      normalizeRevisionInstruction(instruction),
      source === "voice" ? "voice" : "typed"
    );

    setRetrievalDebugMeta({
      decisionEventType: "assistant_revision_proposal",
      decisionReason: "conversation_route_revision",
      confidence: result.confidence
    });
    setRetrievalDebugChunks(result.retrieved_chunks);
    setRevisionDebug({
      activeArtifactId: result.active_artifact_id,
      selectedStart: result.used_start_offset,
      selectedEnd: result.used_end_offset,
      selectionSource: result.selection_source,
      confidence: result.confidence,
      groundingNotes: result.grounding_notes,
      contextSource: result.context_source,
      instructionSource: source === "voice" ? "voice" : "typed",
      retrievedChunks: result.retrieved_chunks
    });

    setSuggestionActionMessage("I tightened that section and queued a revision proposal. You can apply it inline below.");
    setSelectedArtifactId(targetArtifactId);
    await setActiveArtifactSelection(taskId, targetArtifactId);
    await loadArtifactRevisionState(taskId, targetArtifactId);
    await refreshActionCenterState(taskId);
  }

  async function autoCreateSubtaskFromIntent(
    taskId: number,
    message: string,
    source: "text" | "voice",
    slots?: IntentSlotsDto | null
  ) {
    const executionType = slots?.draft_type
      ? inferSubtaskExecutionTypeFromDraftType(slots.draft_type)
      : inferSubtaskExecutionType(message);
    const description = slots?.instruction ?? message;
    const job = await createAgentJob({
      taskId,
      goalContract: JSON.stringify({
        title: deriveSubtaskTitle(description),
        instruction: description,
        execution_type: executionType,
        source: source === "voice" ? "voice" : "text"
      })
    });
    setSelectedAgentJob(job);
    setSuggestionActionMessage("I started an agent job. Plan, steps, verification, and delivery are in the workload view.");
    await refreshActionCenterState(taskId);
    const messageList = await listMessages(taskId);
    setMessages(messageList);
  }

  async function submitMessage(taskId: number, message: string, source: "text" | "voice") {
    setErrorMessage(null);
    await interruptCurrentInteraction("user_barge_in");
    updateTrayStatus("working");

    if (isStreamingEnabled()) {
      await submitMessageStreaming(taskId, message, source);
      return;
    }

    // non-streaming fallback: used in tests and when VITE_JEFF_STREAMING=0.
    const requestId = ++sendRequestIdRef.current;

    try {
      const result = await sendMessage(taskId, message, source);
      if (requestId !== sendRequestIdRef.current) {
        return;
      }

      const messageList = await listMessages(taskId);
      setMessages(messageList);
      await refreshActionCenterState(taskId);
      setRetrievalDebugChunks(result.retrieved_chunks);
      setRetrievalDebugMeta({
        decisionEventType: "assistant_answer",
        decisionReason: "direct_user_request",
        confidence: result.retrieved_chunks[0]?.similarity_score ?? null
      });
      await refreshCoworkingStatus();

      if (result.cancelled) {
        updateTrayStatus("idle");
        return;
      }

      if (result.assistant_response.trim().length > 0) {
        await startSpeechPlayback(result.assistant_response, requestId);
      } else {
        updateTrayStatus("idle");
      }
    } catch (error) {
      if (requestId === sendRequestIdRef.current) {
        setOperationError("Message request failed", error);
      }
      await refreshCoworkingStatus();
      updateTrayStatus("idle");
    }
  }

  async function submitMessageStreaming(
    taskId: number,
    message: string,
    source: "text" | "voice"
  ) {
    // reset the tts queue for this new turn before the backend starts emitting.
    stopStreamingTtsPlayback();
    streamTtsNextPhraseRef.current = 1;

    try {
      const turnId = await sendMessageStreaming(taskId, message, source);
      streamingTurnIdRef.current = turnId;
      ttsActiveTurnIdRef.current = turnId;
      setStreamingTurnId(turnId);
      setStreamingText((prev) => ({ ...prev, [turnId]: "" }));
      updateTrayStatus("working");
      await refreshCoworkingStatus();
    } catch (error) {
      streamingTurnIdRef.current = null;
      ttsActiveTurnIdRef.current = null;
      setStreamingTurnId(null);
      setOperationError("Streaming message request failed", error);
      updateTrayStatus("idle");
      await refreshCoworkingStatus();
    }
  }

  async function runProactiveEvaluation(taskId: number) {
    const evaluation = await evaluateProactiveNudge(taskId);
    setCoworkingStatus(evaluation.status);
    setRetrievalDebugMeta({
      decisionEventType: evaluation.decision_event_type,
      decisionReason: evaluation.decision_reason,
      confidence: evaluation.nudge?.confidence ?? null
    });

    if (!evaluation.nudge) {
      return;
    }

    await interruptCurrentInteraction("jeff_barge_in");

    const requestId = ++sendRequestIdRef.current;
    const updatedMessages = await listMessages(taskId);
    setMessages(updatedMessages);
    await refreshActionCenterState(taskId);
    setRetrievalDebugChunks(evaluation.nudge.retrieved_chunks);

    await startSpeechPlayback(evaluation.nudge.message, requestId);
  }

  async function runSuggestionEvaluation(taskId: number) {
    const evaluation: SuggestionEvaluationDto = await evaluateNextSuggestions(taskId, selectedArtifactId);

    setSessionModeState(evaluation.mode_state);
    setSuggestions(evaluation.suggestions);
    setFlowDebug({
      decisionReason: evaluation.decision_reason,
      suppressionState: evaluation.suppression_state,
      noOp: evaluation.no_op,
      evidenceScore: evaluation.evidence_score,
      modeReason: evaluation.mode_state.mode_reason,
      retrievedChunks: evaluation.retrieved_chunks
    });
  }

  function currentRevisionTarget(): { start_offset: number; end_offset: number } | null {
    if (!artifactContent?.is_editable) {
      return null;
    }

    const start = Math.max(0, Math.min(artifactSelectionStart, artifactSelectionEnd));
    const end = Math.max(start, Math.max(artifactSelectionStart, artifactSelectionEnd));

    return {
      start_offset: start,
      end_offset: end
    };
  }

  function setActionPending(actionKey: string, pending: boolean) {
    setPendingActionKeys((current) => {
      if (pending) {
        return { ...current, [actionKey]: true };
      }

      const next = { ...current };
      delete next[actionKey];
      return next;
    });
  }

  function isActionPending(actionKey: string): boolean {
    return Boolean(pendingActionKeys[actionKey]);
  }

  function setOperationError(prefix: string, error: unknown) {
    setErrorMessage(`${prefix}: ${formatError(error)}`);
  }

  async function handleAcceptSuggestion(suggestion: SuggestionDto) {
    if (!activeTask) {
      return;
    }

    const actionKey = `suggestion-accept-${suggestion.suggestion_id}`;
    if (isActionPending(actionKey)) {
      return;
    }

    setActionPending(actionKey, true);
    setErrorMessage(null);

    try {
      const result: SuggestionAcceptanceDto = await acceptSuggestion(
        activeTask.id,
        suggestion.suggestion_id,
        selectedArtifactId,
        currentRevisionTarget()
      );

      if (result.revision_result) {
        setRetrievalDebugMeta({
          decisionEventType: "assistant_revision_proposal",
          decisionReason: `suggestion_accept_${result.suggestion.suggestion_type}`,
          confidence: result.revision_result.confidence
        });
        setRetrievalDebugChunks(result.revision_result.retrieved_chunks);
        setRevisionDebug({
          activeArtifactId: result.revision_result.active_artifact_id,
          selectedStart: result.revision_result.used_start_offset,
          selectedEnd: result.revision_result.used_end_offset,
          selectionSource: result.revision_result.selection_source,
          confidence: result.revision_result.confidence,
          groundingNotes: result.revision_result.grounding_notes,
          contextSource: result.revision_result.context_source,
          instructionSource: "system",
          retrievedChunks: result.revision_result.retrieved_chunks
        });

        if (selectedArtifactId) {
          await loadArtifactRevisionState(activeTask.id, selectedArtifactId);
        }
      }

      if (result.subtask) {
        updateSubtaskDebugFromSubtask(result.subtask);
      }

      const messageList = await listMessages(activeTask.id);
      setMessages(messageList);
      await refreshActionCenterState(activeTask.id);
      setSessionModeState(await getSessionModeState(activeTask.id));

      if (result.followup_message && result.followup_message.trim().length > 0) {
        const requestId = ++sendRequestIdRef.current;
        await startSpeechPlayback(result.followup_message, requestId);
      }

      setSuggestionActionMessage(formatSuggestionActionMessage(result));
      setSuggestionExplainById((current) => {
        const next = { ...current };
        delete next[suggestion.suggestion_id];
        return next;
      });

      await refreshCoworkingStatus();
    } catch (error) {
      setOperationError("Failed to accept suggestion", error);
    } finally {
      setActionPending(actionKey, false);
    }
  }

  async function handleDismissSuggestion(suggestionId: number) {
    if (!activeTask) {
      return;
    }

    const actionKey = `suggestion-dismiss-${suggestionId}`;
    if (isActionPending(actionKey)) {
      return;
    }
    setActionPending(actionKey, true);
    setErrorMessage(null);

    try {
      const dismissed = await dismissSuggestion(activeTask.id, suggestionId);
      await refreshActionCenterState(activeTask.id);
      setSessionModeState(await getSessionModeState(activeTask.id));
      setSuggestionActionMessage(`Dismissed suggestion: ${dismissed.title}`);
      setSuggestionExplainById((current) => {
        const next = { ...current };
        delete next[suggestionId];
        return next;
      });
    } catch (error) {
      setOperationError("Failed to dismiss suggestion", error);
    } finally {
      setActionPending(actionKey, false);
    }
  }

  async function handleExplainSuggestion(suggestionId: number) {
    if (!activeTask) {
      return;
    }

    setErrorMessage(null);

    try {
      const explanation = await explainSuggestion(activeTask.id, suggestionId);
      setSuggestionExplainById((current) => ({
        ...current,
        [suggestionId]: explanation
      }));
    } catch (error) {
      setOperationError("Failed to explain suggestion", error);
    }
  }

  async function handleRefreshSuggestions() {
    if (!activeTask || suggestionEvaluationInFlightRef.current) {
      return;
    }

    setErrorMessage(null);
    suggestionEvaluationInFlightRef.current = true;

    try {
      await runSuggestionEvaluation(activeTask.id);
      await refreshActionCenterState(activeTask.id);
    } catch (error) {
      setOperationError("Failed to refresh suggestions", error);
    } finally {
      suggestionEvaluationInFlightRef.current = false;
    }
  }

  // phase 13: workspace watcher and clipboard handlers

  async function handleStartWatcher(folderPath: string) {
    if (!activeTask) return;
    try {
      const status = await startWorkspaceWatcher(activeTask.id, folderPath);
      setWatcherStatus(status);
      activeWatcherTaskIdRef.current = activeTask.id;
      await refreshPrivacyCenter();
    } catch (error) {
      setOperationError("Failed to start workspace watcher", error);
    }
  }

  async function handleStopWatcher() {
    if (!activeTask) return;
    try {
      const status = await stopWorkspaceWatcher(activeTask.id);
      setWatcherStatus(status);
      if (activeWatcherTaskIdRef.current === activeTask.id) {
        activeWatcherTaskIdRef.current = null;
      }
      await refreshPrivacyCenter();
    } catch (error) {
      setOperationError("Failed to stop workspace watcher", error);
    }
  }

  async function handleToggleClipboardCapture() {
    if (!activeTask) return;
    const next = !clipboardCaptureEnabled;
    try {
      await setClipboardCapture(activeTask.id, next);
      setClipboardCaptureEnabled(next);
      await refreshPrivacyCenter();
    } catch (error) {
      setOperationError("Failed to update clipboard capture setting", error);
    }
  }

  async function handleOpenPrivacyCenter() {
    setPrivacyCenterOpen(true);
    setPrivacyActionMessage(null);
    await refreshPrivacyCenter();
  }

  async function handleTogglePrivacySurface(surface: string, enabled: boolean) {
    try {
      const dashboard = await setPrivacySurfaceEnabled(surface, enabled);
      setPrivacyDashboard(dashboard);
      setClipboardCaptureEnabled(dashboard.clipboard_capture_enabled);
      setQuietModeState(!dashboard.proactive_triggers_enabled);
      if (!dashboard.typing_activity_enabled) {
        userIsTypingRef.current = false;
      }
      if (!dashboard.user_profile_memory_enabled) {
        setUserProfileSignals([]);
        setRelationalProfile(null);
      } else {
        void getUserProfileSignals().then(setUserProfileSignals).catch(() => undefined);
        void getRelationalProfile().then(setRelationalProfile).catch(() => undefined);
      }
      if (!dashboard.calendar_context_enabled) {
        setCalendarEvent(null);
      } else if (surface === "calendar_context") {
        void getCalendarNextEvent().then(setCalendarEvent).catch(() => undefined);
      }
      if (dashboard.active_task_id !== null) {
        const [triggerLog, writeLog, synthesisEntries] = await Promise.all([
          listProactiveTriggerAuditLog(dashboard.active_task_id),
          listWriteAuditLog(dashboard.active_task_id),
          getSynthesisLog(dashboard.active_task_id)
        ]);
        setProactiveAuditLog(triggerLog);
        setWriteAuditLog(writeLog);
        setSynthesisLog(synthesisEntries);
      } else {
        setProactiveAuditLog([]);
        setWriteAuditLog([]);
        setSynthesisLog([]);
      }
    } catch (error) {
      setOperationError("Failed to update privacy setting", error);
    }
  }

  async function handleToggleWakeWord(enabled: boolean) {
    try {
      const wakeWord = await setWakeWordEnabled(enabled);
      setPrivacyDashboard((current) => (current ? { ...current, wake_word: wakeWord } : current));
      setPrivacyActionMessage(
        wakeWord.enabled
          ? wakeWord.armed
            ? "Wake word is armed."
            : "Wake word is enabled but the detector is not running."
          : "Wake word is off."
      );
      if (wakeWord.last_error) {
        setOperationError("Failed to update wake word detector", wakeWord.last_error);
      }
    } catch (error) {
      setOperationError("Failed to update wake word detector", error);
      await refreshPrivacyCenter();
    }
  }

  async function handleToggleCrisisClass(className: string, enabled: boolean) {
    try {
      const dashboard = await setCrisisClassEnabled(className, enabled);
      setPrivacyDashboard(dashboard);
      setPrivacyActionMessage(`${enabled ? "Enabled" : "Disabled"} ${className.replace(/_/g, " ")}.`);
    } catch (error) {
      setOperationError("Failed to update override channel", error);
      await refreshPrivacyCenter();
    }
  }

  async function handleRevertActionReceipt(receiptId: number) {
    try {
      await revertActionReceipt(receiptId);
      await refreshPrivacyCenter();
      setPrivacyActionMessage(`Reverted action receipt #${receiptId}.`);
    } catch (error) {
      setOperationError("Failed to revert action receipt", error);
      await refreshPrivacyCenter();
    }
  }

  async function handleSetTrustLevel(actionClass: string, level: "L1" | "L2" | "L3") {
    try {
      const trustLadder = await setTrustLevel(actionClass, level);
      setPrivacyDashboard((current) => current ? { ...current, trust_ladder: trustLadder } : current);
      setPrivacyActionMessage(`Set ${actionClass} trust to ${level}.`);
      await refreshPrivacyCenter();
    } catch (error) {
      setOperationError("Failed to update trust level", error);
      await refreshPrivacyCenter();
    }
  }

  async function handleDemoteTrustClass(actionClass: string) {
    try {
      const trustLadder = await demoteTrustClass(actionClass);
      setPrivacyDashboard((current) => current ? { ...current, trust_ladder: trustLadder } : current);
      setPrivacyActionMessage(`Demoted ${actionClass} to L1.`);
      await refreshPrivacyCenter();
    } catch (error) {
      setOperationError("Failed to demote trust class", error);
      await refreshPrivacyCenter();
    }
  }

  async function handleSetTtsVoice(voice: string) {
    try {
      const dashboard = await setTtsVoice(voice);
      setPrivacyDashboard(dashboard);
    } catch (error) {
      setOperationError("Failed to update Jeff's voice", error);
    }
  }

  async function handleStartLocalRuntime() {
    setLocalModelBusy(true);
    try {
      const status = await startLocalRuntime();
      setPrivacyDashboard((current) => current ? { ...current, local_runtime: status } : current);
      setPrivacyActionMessage("Local runtime started.");
      await refreshPrivacyCenter();
    } catch (error) {
      setOperationError("Failed to start local runtime", error);
    } finally {
      setLocalModelBusy(false);
    }
  }

  async function handleStopLocalRuntime() {
    setLocalModelBusy(true);
    try {
      const status = await stopLocalRuntime();
      setPrivacyDashboard((current) => current ? { ...current, local_runtime: status } : current);
      setPrivacyActionMessage("Local runtime stopped.");
      await refreshPrivacyCenter();
    } catch (error) {
      setOperationError("Failed to stop local runtime", error);
    } finally {
      setLocalModelBusy(false);
    }
  }

  async function handleDeleteLocalModel(kind: "reasoning" | "embedding") {
    setLocalModelBusy(true);
    try {
      const status = await deleteLocalModel(kind);
      setPrivacyDashboard((current) => current ? { ...current, local_runtime: status } : current);
      setPrivacyActionMessage(`Deleted local ${kind} model.`);
      await refreshPrivacyCenter();
    } catch (error) {
      setOperationError("Failed to delete local model", error);
    } finally {
      setLocalModelBusy(false);
    }
  }

  async function handleDownloadLocalModel(kind: "reasoning" | "embedding") {
    const parsedExpectedBytes = localModelExpectedBytes.trim()
      ? Number(localModelExpectedBytes.trim())
      : null;
    const expectedBytes =
      parsedExpectedBytes !== null && Number.isFinite(parsedExpectedBytes)
        ? parsedExpectedBytes
        : null;
    setLocalModelBusy(true);
    try {
      const status = await downloadLocalModel(
        kind,
        localModelUrl.trim(),
        localModelSha256.trim(),
        expectedBytes
      );
      setPrivacyDashboard((current) => current ? { ...current, local_runtime: status } : current);
      setLocalModelUrl("");
      setLocalModelSha256("");
      setLocalModelExpectedBytes("");
      setPrivacyActionMessage(`Installed local ${kind} model.`);
      await refreshPrivacyCenter();
    } catch (error) {
      setOperationError("Failed to install local model", error);
    } finally {
      setLocalModelBusy(false);
    }
  }

  async function handleToggleDebrief(next: boolean) {
    try {
      await setDebriefEnabled(next);
      setDebriefEnabledState(next);
      setPrivacyActionMessage(next ? "End-of-day debrief enabled." : "End-of-day debrief disabled.");
    } catch (error) {
      setOperationError("Failed to update debrief setting", error);
    }
  }

  async function handleToggleVoice(next: boolean) {
    try {
      const config = await setVoiceConfig(next, "");
      setVoiceEnabledState(config.enabled);
      setPrivacyActionMessage(
        next ? "Realtime voice enabled." : "Realtime voice disabled (STT/TTS pipeline)."
      );
    } catch (error) {
      setOperationError("Failed to update voice setting", error);
    }
  }

  async function handleDownloadCuratedEmbeddingModel() {
    setLocalModelBusy(true);
    try {
      const status = await downloadCuratedEmbeddingModel();
      setPrivacyDashboard((current) => current ? { ...current, local_runtime: status } : current);
      setPrivacyActionMessage("Installed the semantic embedding model.");
      await refreshPrivacyCenter();
    } catch (error) {
      setOperationError("Failed to install semantic embedding model", error);
    } finally {
      setLocalModelBusy(false);
    }
  }

  async function handleSetLlmDailyBudget(budgetKey: string, rawValue: string) {
    const budgetUsd = Number(rawValue.trim());
    if (!Number.isFinite(budgetUsd) || budgetUsd < 0) {
      setPrivacyActionMessage("Daily budget must be a non-negative dollar amount.");
      return;
    }
    try {
      const status = await setLlmDailyBudget(budgetKey, budgetUsd);
      setPrivacyDashboard((current) => current ? { ...current, cost_governor: status } : current);
      setPrivacyActionMessage(`Updated ${budgetKey} daily budget.`);
    } catch (error) {
      setOperationError("Failed to update LLM budget", error);
    }
  }

  async function handleDismissSelectionCapture() {
    try {
      await dismissSelectionCapture();
      setSelectionCaptureIndicator(null);
    } catch (error) {
      setOperationError("Failed to dismiss captured selection", error);
    }
  }

  async function handleClearUserProfileMemory() {
    try {
      const dashboard = await clearUserProfileMemory();
      setPrivacyDashboard(dashboard);
      setUserProfileSignals([]);
      setRelationalProfile(await getRelationalProfile());
      setMemoryFacts([]);
      setMemoryEpisodes([]);
      setMemoryPromptPreview(null);
      setPrivacyActionMessage("User profile memory cleared.");
    } catch (error) {
      setOperationError("Failed to clear user profile memory", error);
    }
  }

  async function handleRunMemoryConsolidation() {
    if (memoryConsolidationBusy) {
      return;
    }
    setMemoryConsolidationBusy(true);
    try {
      const report: ConsolidationReportDto = await runMemoryConsolidation();
      setPrivacyActionMessage(
        `Consolidated ${report.processed_episode_count} episodes; ${report.upserted_fact_count} facts added, ${report.merged_fact_count} merged.`
      );
      await refreshPrivacyCenter();
    } catch (error) {
      setOperationError("Failed to consolidate memory", error);
    } finally {
      setMemoryConsolidationBusy(false);
    }
  }

  async function handleDeleteFact(id: number) {
    try {
      await deleteFact(id);
      await refreshPrivacyCenter();
    } catch (error) {
      setOperationError("Failed to delete memory fact", error);
    }
  }

  async function handleDeleteEpisode(id: number) {
    try {
      await deleteEpisode(id);
      await refreshPrivacyCenter();
    } catch (error) {
      setOperationError("Failed to delete memory episode", error);
    }
  }

  async function handleClearActiveTaskData() {
    try {
      const result: DataClearResultDto = await clearActiveTaskData();
      setPrivacyActionMessage(result.message);
      setSelectionCaptureIndicator(null);
      await refreshShellState();
      await refreshPrivacyCenter();
    } catch (error) {
      setOperationError("Failed to clear active task data", error);
    }
  }

  async function handleClearAllJeffData() {
    if (clearAllConfirmation.trim() !== "CLEAR JEFF") {
      setPrivacyActionMessage("Type CLEAR JEFF to confirm clearing all data.");
      return;
    }

    try {
      const result: DataClearResultDto = await clearAllJeffData();
      setPrivacyActionMessage(result.message);
      setClearAllConfirmation("");
      setPrivacyCenterOpen(false);
      setSelectionCaptureIndicator(null);
      await refreshShellState();
      await refreshPrivacyCenter();
    } catch (error) {
      setOperationError("Failed to clear all Jeff data", error);
    }
  }

  async function handleRefreshRecentlyLearned() {
    if (!activeTask) return;
    try {
      const items = await listRecentlyLearned(activeTask.id, 10);
      setRecentlyLearned(items);
    } catch {
      // non-fatal
    }
  }

  // phase 15: proactive initiation handlers

  function dismissDriftNotice() {
    setDriftNotice(null);
    if (activeTask) {
      void dismissProactiveTrigger(activeTask.id, "drift").catch(() => undefined);
    }
  }

  async function handleToggleQuietMode() {
    const next = !quietMode;
    try {
      await setQuietMode(next);
      setQuietModeState(next);
      await refreshPrivacyCenter();
    } catch {
      // non-fatal
    }
  }

  async function handleDismissSpeculativeSubtask() {
    if (activeTask && speculativeSubtask) {
      void dismissProactiveTrigger(activeTask.id, "stuck").catch(() => undefined);
    }
    setSpeculativeSubtask(null);
  }

  async function handleCancelSpeculativeSubtask() {
    if (speculativeSubtask) {
      try {
        await cancelSubtask(speculativeSubtask.subtask_id);
      } catch {
        // non-fatal
      }
    }
    setSpeculativeSubtask(null);
  }

  async function handleProposeRevision(instructionSource: "typed" | "voice", voiceInstruction?: string) {
    if (!activeTask || !selectedArtifactId) {
      setErrorMessage("Select an editable artifact before requesting revision.");
      return;
    }

    const instruction = (voiceInstruction ?? revisionInstruction).trim();
    if (!instruction) {
      return;
    }

    await interruptCurrentInteraction("user_barge_in");

    try {
      const result = await proposeArtifactRevision(
        activeTask.id,
        selectedArtifactId,
        {
          start_offset: artifactSelectionStart,
          end_offset: artifactSelectionEnd
        },
        instruction,
        instructionSource
      );

      setRetrievalDebugMeta({
        decisionEventType: "assistant_revision_proposal",
        decisionReason: `revision_${result.selection_source}`,
        confidence: result.confidence
      });
      setRetrievalDebugChunks(result.retrieved_chunks);

      setRevisionDebug({
        activeArtifactId: result.active_artifact_id,
        selectedStart: result.used_start_offset,
        selectedEnd: result.used_end_offset,
        selectionSource: result.selection_source,
        confidence: result.confidence,
        groundingNotes: result.grounding_notes,
        contextSource: result.context_source,
        instructionSource,
        retrievedChunks: result.retrieved_chunks
      });

      if (instructionSource === "typed") {
        setRevisionInstruction("");
      }

      await loadArtifactRevisionState(activeTask.id, selectedArtifactId);
      await refreshActionCenterState(activeTask.id);
      const messageList = await listMessages(activeTask.id);
      setMessages(messageList);
      await refreshCoworkingStatus();
    } catch (error) {
      setOperationError("Failed to create revision proposal", error);
      await refreshCoworkingStatus();
    }
  }

  async function handleApplyRevision(revisionId: number) {
    if (!activeTask || !selectedArtifactId) {
      return;
    }

    try {
      const result = await applyRevision(revisionId);
      setArtifactContent(result.artifact_content);
      await loadArtifactRevisionState(activeTask.id, selectedArtifactId);
      await refreshActionCenterState(activeTask.id);
      const messageList = await listMessages(activeTask.id);
      setMessages(messageList);

      setRevisionDebug((current) => ({
        ...current,
        activeArtifactId: selectedArtifactId,
        confidence: result.revision.retrieval_confidence,
        groundingNotes:
          result.revision.grounding_notes ?? "Applied revision with stored grounding details.",
        contextSource: "direct_instruction"
      }));
    } catch (error) {
      setOperationError("Failed to apply revision", error);
    }
  }

  async function handleRejectRevision(revisionId: number) {
    if (!activeTask || !selectedArtifactId) {
      return;
    }

    try {
      await rejectRevision(revisionId);
      await loadArtifactRevisionState(activeTask.id, selectedArtifactId);
      await refreshActionCenterState(activeTask.id);
      const messageList = await listMessages(activeTask.id);
      setMessages(messageList);
    } catch (error) {
      setOperationError("Failed to reject revision", error);
    }
  }

  async function handleRevertVersion(versionId: number) {
    if (!activeTask || !selectedArtifactId) {
      return;
    }

    try {
      const restored = await revertArtifactToVersion(versionId);
      setArtifactContent(restored);
      await loadArtifactRevisionState(activeTask.id, selectedArtifactId);
      await refreshActionCenterState(activeTask.id);
      const messageList = await listMessages(activeTask.id);
      setMessages(messageList);
    } catch (error) {
      setOperationError("Failed to revert version", error);
    }
  }

  async function handleCreateSubtask(
    instructionSource: "text" | "voice" | "system",
    voiceInstruction?: string,
    executionTypeOverride?: ExecutionType
  ) {
    if (!activeTask) {
      setErrorMessage("Select an active task before creating a subtask.");
      return;
    }

    const description = (voiceInstruction ?? subtaskInstruction).trim();
    if (!description) {
      return;
    }

    const executionType = executionTypeOverride ?? subtaskExecutionType;
    const title = deriveSubtaskTitle(description);

    try {
      const job = await createAgentJob({
        taskId: activeTask.id,
        goalContract: JSON.stringify({
          title,
          instruction: description,
          execution_type: executionType,
          source: instructionSource
        })
      });

      if (instructionSource === "text") {
        setSubtaskInstruction("");
      }

      setSubtaskSuggestion(null);
      setSelectedAgentJob(job);
      setSuggestionActionMessage("I started an agent job. Review the live plan, verification, and deliverable in the workload view.");
      await refreshActionCenterState(activeTask.id);
      const messageList = await listMessages(activeTask.id);
      setMessages(messageList);
      await refreshCoworkingStatus();
    } catch (error) {
      setOperationError("Failed to create subtask", error);
    }
  }

  async function handleSendJobSteering() {
    if (!activeTask || !selectedAgentJob) {
      return;
    }
    const message = jobSteeringInput.trim();
    if (!message) {
      return;
    }
    try {
      const detail = await sendJobSteering(selectedAgentJob.job.id, message);
      setSelectedAgentJob(detail);
      setJobSteeringInput("");
      await refreshActionCenterState(activeTask.id);
    } catch (error) {
      setOperationError("Failed to send job steering", error);
    }
  }

  async function handleCancelAgentJob() {
    if (!activeTask || !selectedAgentJob) {
      return;
    }
    try {
      const detail = await cancelAgentJob(selectedAgentJob.job.id);
      setSelectedAgentJob(detail);
      await refreshActionCenterState(activeTask.id);
    } catch (error) {
      setOperationError("Failed to cancel agent job", error);
    }
  }

  async function handleCreateStandingJobFromInstruction() {
    if (!activeTask) {
      setErrorMessage("Select an active task before creating a standing job.");
      return;
    }
    const description = subtaskInstruction.trim();
    if (!description) {
      return;
    }
    try {
      await createStandingJob({
        taskId: activeTask.id,
        goalContract: JSON.stringify({
          title: deriveSubtaskTitle(description),
          instruction: description,
          execution_type: subtaskExecutionType,
          source: "standing_job"
        }),
        scheduleSpec: inferStandingScheduleSpec(description),
        critical: /\b(guard|critical|watch|alert)\b/i.test(description)
      });
      setSubtaskInstruction("");
      setSuggestionActionMessage("I created a standing job. It is listed in the workload view with its next run.");
      await refreshActionCenterState(activeTask.id);
    } catch (error) {
      setOperationError("Failed to create standing job", error);
    }
  }

  async function handleRunDueStandingJobs() {
    if (!activeTask) {
      return;
    }
    try {
      const details = await runDueStandingJobs();
      if (details.length > 0) {
        setSelectedAgentJob(details[0]);
      }
      await refreshActionCenterState(activeTask.id);
    } catch (error) {
      setOperationError("Failed to run due standing jobs", error);
    }
  }

  async function handleToggleSpeculation(enabled: boolean) {
    try {
      const status = await setSpeculationEnabled(enabled);
      setPrivacyDashboard((current) => (current ? { ...current, speculation: status } : current));
    } catch (error) {
      setOperationError("Failed to update speculation", error);
    }
  }

  async function handleDiscardSpeculation(cacheId: number) {
    try {
      await discardSpeculationCacheEntry(cacheId);
      setSpeculationCache((current) => current.filter((entry) => entry.id !== cacheId));
    } catch (error) {
      setOperationError("Failed to discard speculation", error);
    }
  }

  async function handleRemoveRemoteDoc(id: number) {
    try {
      await removeRemoteDoc(id);
      setRemoteDocs((current) => current.filter((doc) => doc.id !== id));
    } catch (error) {
      setOperationError("Failed to remove remote doc", error);
    }
  }

  async function handleSaveWebUserGuard() {
    try {
      await setWebUserNameGuard(webUserGuard.trim());
    } catch (error) {
      setOperationError("Failed to save web guard", error);
    }
  }

  async function handleToggleToolConnection(connectionId: number, enabled: boolean) {
    try {
      const updated = await setToolConnectionEnabled(connectionId, enabled);
      setToolConnections((current) => current.map((c) => (c.id === updated.id ? updated : c)));
    } catch (error) {
      setOperationError("Failed to update connection", error);
    }
  }

  async function handleRemoveToolConnection(connectionId: number) {
    try {
      await removeToolConnection(connectionId);
      setToolConnections((current) => current.filter((c) => c.id !== connectionId));
    } catch (error) {
      setOperationError("Failed to disconnect", error);
    }
  }

  async function handleProposeCustomTool(gapId: number) {
    try {
      const tool = await proposeCustomTool(gapId);
      setCustomTools((current) => [tool, ...current.filter((t) => t.id !== tool.id)]);
    } catch (error) {
      setOperationError("Failed to propose tool", error);
    }
  }

  async function handleApproveCustomTool(toolId: number) {
    try {
      const tool = await approveCustomTool(toolId);
      setCustomTools((current) => current.map((t) => (t.id === tool.id ? tool : t)));
    } catch (error) {
      setOperationError("Failed to approve tool", error);
    }
  }

  async function handleKillCustomTool(toolId: number) {
    try {
      const tool = await killCustomTool(toolId);
      setCustomTools((current) => current.map((t) => (t.id === tool.id ? tool : t)));
    } catch (error) {
      setOperationError("Failed to kill tool", error);
    }
  }

  async function handleToggleStandingJob(standingJobId: number, enabled: boolean) {
    try {
      await setStandingJobEnabled(standingJobId, enabled);
      if (activeTask) {
        await refreshActionCenterState(activeTask.id);
      }
    } catch (error) {
      setOperationError("Failed to update standing job", error);
    }
  }

  async function handleCancelSubtask(subtaskId: number) {
    if (!activeTask) {
      return;
    }

    try {
      const cancelled = await cancelSubtask(subtaskId);
      updateSubtaskDebugFromSubtask(cancelled);
      await refreshActionCenterState(activeTask.id);
      const messageList = await listMessages(activeTask.id);
      setMessages(messageList);
    } catch (error) {
      setOperationError("Failed to cancel subtask", error);
    }
  }

  async function handleAcceptSubtaskResult(subtaskId: number) {
    if (!activeTask) {
      return;
    }

    try {
      const updated = await acceptSubtaskResult(subtaskId);
      updateSubtaskDebugFromSubtask(updated);
      await refreshActionCenterState(activeTask.id);
    } catch (error) {
      setOperationError("Failed to accept subtask result", error);
    }
  }

  async function handleRejectSubtaskResult(subtaskId: number) {
    if (!activeTask) {
      return;
    }

    try {
      const updated = await rejectSubtaskResult(subtaskId);
      updateSubtaskDebugFromSubtask(updated);
      await refreshActionCenterState(activeTask.id);
    } catch (error) {
      setOperationError("Failed to reject subtask result", error);
    }
  }

  // phase 16: file write proposal handlers

  async function handleApproveFileWrite(proposalId: number) {
    if (!activeTask) return;
    try {
      const entry = await approveSubtaskFileWrite(activeTask.id, proposalId);
      setWriteAuditLog((prev) => [entry, ...prev]);
      const updated = await listFileWriteProposals(activeTask.id);
      setFileWriteProposals(updated);
      await refreshActionCenterState(activeTask.id);
    } catch (error) {
      setOperationError("Failed to approve file write", error);
    }
  }

  async function handleRejectFileWrite(proposalId: number) {
    if (!activeTask) return;
    try {
      const entry = await rejectSubtaskFileWrite(activeTask.id, proposalId);
      setWriteAuditLog((prev) => [entry, ...prev]);
      const updated = await listFileWriteProposals(activeTask.id);
      setFileWriteProposals(updated);
    } catch (error) {
      setOperationError("Failed to reject file write", error);
    }
  }

  async function handleSuggestSubtask() {
    if (!activeTask) {
      return;
    }

    try {
      const suggestion = await suggestSubtask(activeTask.id);
      setSubtaskSuggestion(suggestion);
      if (suggestion) {
        const parsed = parseSubtaskSnapshot(suggestion.parent_context_snapshot);
        setSubtaskDebug({
          selectedSubtaskId: null,
          executionType: suggestion.execution_type,
          instructionSource: suggestion.instruction_source,
          contextSnapshot: suggestion.parent_context_snapshot,
          retrievedChunks: parsed?.retrieved_chunks ?? suggestion.retrieved_chunks
        });
      }
    } catch (error) {
      setOperationError("Failed to load subtask suggestion", error);
    }
  }

  async function handleAcceptSuggestedSubtask() {
    if (!subtaskSuggestion) {
      return;
    }

    const suggestedType = normalizeExecutionType(subtaskSuggestion.execution_type);
    await handleCreateSubtask(
      "system",
      subtaskSuggestion.description,
      suggestedType
    );
  }

  async function handleRefineSubtask(subtaskId: number) {
    if (!activeTask) {
      return;
    }

    const instruction = (subtaskRefinementInputById[subtaskId] ?? "").trim();
    if (!instruction) {
      return;
    }

    try {
      const refined = await refineSubtask(subtaskId, instruction, "text");
      setSubtaskRefinementInputById((current) => ({ ...current, [subtaskId]: "" }));
      updateSubtaskDebugFromSubtask(refined);
      await refreshActionCenterState(activeTask.id);
      const messageList = await listMessages(activeTask.id);
      setMessages(messageList);
    } catch (error) {
      setOperationError("Failed to refine subtask", error);
    }
  }

  async function handleConvertSubtaskToRevision(subtaskId: number) {
    if (!activeTask || !selectedArtifactId) {
      setErrorMessage("Select an editable artifact before converting subtask output.");
      return;
    }

    try {
      const result = await convertSubtaskToRevision(
        activeTask.id,
        subtaskId,
        selectedArtifactId,
        {
          start_offset: artifactSelectionStart,
          end_offset: artifactSelectionEnd
        }
      );

      setRetrievalDebugMeta({
        decisionEventType: "assistant_revision_proposal",
        decisionReason: `subtask_to_revision_${result.selection_source}`,
        confidence: result.confidence
      });
      setRetrievalDebugChunks(result.retrieved_chunks);
      setRevisionDebug({
        activeArtifactId: result.active_artifact_id,
        selectedStart: result.used_start_offset,
        selectedEnd: result.used_end_offset,
        selectionSource: result.selection_source,
        confidence: result.confidence,
        groundingNotes: result.grounding_notes,
        contextSource: "subtask_result",
        instructionSource: "system",
        retrievedChunks: result.retrieved_chunks
      });

      await loadArtifactRevisionState(activeTask.id, selectedArtifactId);
      await refreshActionCenterState(activeTask.id);
      const messageList = await listMessages(activeTask.id);
      setMessages(messageList);
      await refreshCoworkingStatus();
    } catch (error) {
      setOperationError("Failed to convert subtask result into revision", error);
    }
  }

  function updateSubtaskDebugFromSubtask(subtask: SubTaskDto) {
    const parsed = parseSubtaskSnapshot(subtask.parent_context_snapshot);
    setSubtaskDebug({
      selectedSubtaskId: subtask.subtask_id,
      executionType: subtask.execution_type,
      instructionSource: subtask.instruction_source,
      contextSnapshot: subtask.parent_context_snapshot,
      retrievedChunks: parsed?.retrieved_chunks ?? []
    });
  }

  async function startRecording(purpose: RecordingPurpose) {
    if (recording || !activeTask) {
      return;
    }

    setErrorMessage(null);

    try {
      await interruptCurrentInteraction("user_barge_in");

      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      const recorder = new MediaRecorder(stream);

      mediaStreamRef.current = stream;
      mediaRecorderRef.current = recorder;
      audioChunksRef.current = [];
      recordingPurposeRef.current = purpose;
      setRecordingPurpose(purpose);

      recorder.ondataavailable = (event) => {
        if (event.data.size > 0) {
          audioChunksRef.current.push(event.data);
        }
      };

      recorder.onstop = () => {
        void finalizeVoiceInput();
      };

      recorder.start();
      setRecording(true);
      const status = await setUserSpeaking(true);
      setCoworkingStatus(status);
      updateTrayStatus("listening");

      // partial stt: best-effort early routing before whisper finalizes.
      // only for chat purpose; revision/subtask need full transcription.
      if (purpose === "chat" && activeTask) {
        tryStartPartialStt(activeTask.id);
      }
    } catch (error) {
      stopMicrophoneStream();
      setRecording(false);
      setOperationError("Failed to start microphone recording", error);
      await refreshCoworkingStatus();
    }
  }

  async function stopRecording() {
    const recorder = mediaRecorderRef.current;
    if (!recorder) {
      return;
    }

    if (recorder.state !== "inactive") {
      recorder.stop();
    }

    setRecording(false);
    updateTrayStatus("working");
    try {
      const status = await setUserSpeaking(false);
      setCoworkingStatus(status);
    } catch {
      // ignore status update failure
    }
  }

  async function finalizeVoiceInput() {
    stopPartialStt();

    const chunks = audioChunksRef.current;
    audioChunksRef.current = [];

    // if partial stt already routed the message, skip whisper entirely.
    if (partialSttSentRef.current) {
      partialSttSentRef.current = false;
      stopMicrophoneStream();
      mediaRecorderRef.current = null;
      recordingPurposeRef.current = "chat";
      setRecordingPurpose("chat");
      await refreshCoworkingStatus();
      return;
    }

    const active = activeTask;
    if (!active || chunks.length === 0) {
      stopMicrophoneStream();
      await refreshCoworkingStatus();
      return;
    }

    try {
      const mimeType = chunks[0].type || "audio/webm";
      const blob = new Blob(chunks, { type: mimeType });
      const audioBase64 = await blobToBase64(blob);

      const transcription = await transcribeAudio(audioBase64, mimeType);
      const transcriptText = transcription.text.trim();

      if (transcriptText.length > 0) {
        if (recordingPurposeRef.current === "revision") {
          await handleProposeRevision("voice", transcriptText);
        } else if (recordingPurposeRef.current === "subtask") {
          await handleCreateSubtask("voice", transcriptText);
        } else if (recordingPurposeRef.current === "cancel_subtask") {
          const target = activeSubtasks[0];
          if (target) {
            await handleCancelSubtask(target.subtask_id);
          } else {
            setErrorMessage("No running subtask to cancel.");
          }
        } else {
          await submitRoutedMessage(active.id, transcriptText, "voice");
        }
      }
    } catch (error) {
      setOperationError("Voice transcription failed", error);
    } finally {
      stopMicrophoneStream();
      mediaRecorderRef.current = null;
      recordingPurposeRef.current = "chat";
      setRecordingPurpose("chat");
      await refreshCoworkingStatus();
    }
  }

  async function interruptCurrentInteraction(
    reason: "user_barge_in" | "jeff_barge_in" | "explicit" | "error" = "explicit"
  ) {
    stopSpeechPlayback();
    stopStreamingTtsPlayback();
    stopPartialStt();

    // cancel any in-flight streaming llm turn on the backend.
    const activeTurnId = streamingTurnIdRef.current;
    if (activeTurnId) {
      void cancelStreamingTurn(activeTurnId, reason).catch(() => undefined);
      // streamingTurnId is cleared by the incoming EVENT_TURN_CANCELLED;
      // no need to reset it here to avoid a race with the next turn.
    }

    await cancelInteraction().catch(() => 0);
    await refreshCoworkingStatus();
  }

  async function startSpeechPlayback(text: string, requestId: number) {
    try {
      const shouldPlay = await waitForSpeechPlaybackSlot(requestId);
      if (!shouldPlay) {
        updateTrayStatus("idle");
        return;
      }

      const speech = await synthesizeSpeech(text);
      if (requestId !== sendRequestIdRef.current) {
        return;
      }

      stopSpeechPlayback();

      const blob = base64ToBlob(speech.audio_base64, speech.mime_type);
      const objectUrl = URL.createObjectURL(blob);
      ttsObjectUrlRef.current = objectUrl;

      const audio = new Audio(objectUrl);
      ttsAudioRef.current = audio;

      const speakingStatus = await setAssistantSpeaking(true);
      setCoworkingStatus(speakingStatus);
      updateTrayStatus("working");

      audio.onended = () => {
        void setAssistantSpeaking(false)
          .then((status) => setCoworkingStatus(status))
          .catch(() => undefined);
        updateTrayStatus("idle");
      };

      audio.onerror = () => {
        void setAssistantSpeaking(false)
          .then((status) => setCoworkingStatus(status))
          .catch(() => undefined);
        setErrorMessage("Failed to play synthesized speech.");
        updateTrayStatus("idle");
      };

      await audio.play();
    } catch (error) {
      setOperationError("Text-to-speech failed (response text still available in chat)", error);
      await refreshCoworkingStatus();
      updateTrayStatus("idle");
    }
  }

  function waitForSpeechPlaybackSlot(requestId: number): Promise<boolean> {
    if (!userIsTypingRef.current) {
      return Promise.resolve(true);
    }

    if (speechDelayTimerRef.current !== null) {
      window.clearTimeout(speechDelayTimerRef.current);
      speechDelayTimerRef.current = null;
    }

    const startedAt = Date.now();
    return new Promise((resolve) => {
      const check = () => {
        if (requestId !== sendRequestIdRef.current) {
          speechDelayTimerRef.current = null;
          resolve(false);
          return;
        }
        if (!userIsTypingRef.current) {
          speechDelayTimerRef.current = null;
          resolve(true);
          return;
        }
        if (Date.now() - startedAt >= 3000) {
          speechDelayTimerRef.current = null;
          resolve(false);
          return;
        }
        speechDelayTimerRef.current = window.setTimeout(check, 100);
      };
      speechDelayTimerRef.current = window.setTimeout(check, 100);
    });
  }

  function stopSpeechPlayback() {
    if (speechDelayTimerRef.current !== null) {
      window.clearTimeout(speechDelayTimerRef.current);
      speechDelayTimerRef.current = null;
    }

    if (ttsAudioRef.current) {
      const isJsDomRuntime =
        typeof navigator !== "undefined" &&
        navigator.userAgent.toLowerCase().includes("jsdom");

      if (!isJsDomRuntime && typeof ttsAudioRef.current.pause === "function") {
        try {
          ttsAudioRef.current.pause();
        } catch {
          // jsdom does not implement media pause fully
        }
      }
      ttsAudioRef.current.currentTime = 0;
      ttsAudioRef.current = null;
    }

    if (ttsObjectUrlRef.current) {
      if (typeof URL.revokeObjectURL === "function") {
        URL.revokeObjectURL(ttsObjectUrlRef.current);
      }
      ttsObjectUrlRef.current = null;
    }

    void setAssistantSpeaking(false)
      .then((status) => setCoworkingStatus(status))
      .catch(() => undefined);
    updateTrayStatus("idle");
  }

  // attempt to play the next queued streaming tts phrase in phrase_id order.
  // called each time a new chunk arrives and each time a phrase finishes.
  function scheduleStreamTtsPlayback() {
    if (userIsTypingRef.current) {
      if (streamTtsDelayTimerRef.current === null) {
        streamTtsDelayTimerRef.current = window.setTimeout(() => {
          streamTtsDelayTimerRef.current = null;
          if (userIsTypingRef.current) {
            discardStreamingTtsForTextOnly();
          } else {
            scheduleStreamTtsPlayback();
          }
        }, 3000);
      }
      return;
    }

    if (streamTtsDelayTimerRef.current !== null) {
      window.clearTimeout(streamTtsDelayTimerRef.current);
      streamTtsDelayTimerRef.current = null;
    }

    if (streamTtsCurrentRef.current !== null) {
      // a phrase is already playing; it will call this on end.
      return;
    }
    const next = streamTtsQueueRef.current.get(streamTtsNextPhraseRef.current);
    if (!next) {
      // next phrase not yet in queue; will be called again when it arrives.
      return;
    }
    streamTtsQueueRef.current.delete(streamTtsNextPhraseRef.current);
    streamTtsNextPhraseRef.current += 1;
    const { audio, url } = next;
    streamTtsCurrentRef.current = audio;

    audio.onended = () => {
      URL.revokeObjectURL(url);
      streamTtsCurrentRef.current = null;
      // clear ttsActiveTurnIdRef only after all queued phrases have played.
      if (streamTtsQueueRef.current.size === 0) {
        ttsActiveTurnIdRef.current = null;
        void setAssistantSpeaking(false)
          .then((status) => setCoworkingStatus(status))
          .catch(() => undefined);
        updateTrayStatus("idle");
      }
      scheduleStreamTtsPlayback();
    };
    audio.onerror = () => {
      URL.revokeObjectURL(url);
      streamTtsCurrentRef.current = null;
      if (streamTtsQueueRef.current.size === 0) {
        ttsActiveTurnIdRef.current = null;
        void setAssistantSpeaking(false)
          .then((status) => setCoworkingStatus(status))
          .catch(() => undefined);
        updateTrayStatus("idle");
      }
      scheduleStreamTtsPlayback();
    };

    void setAssistantSpeaking(true)
      .then((status) => setCoworkingStatus(status))
      .catch(() => undefined);
    updateTrayStatus("working");

    void audio.play().catch(() => {
      streamTtsCurrentRef.current = null;
      if (streamTtsQueueRef.current.size === 0) {
        ttsActiveTurnIdRef.current = null;
        void setAssistantSpeaking(false)
          .then((status) => setCoworkingStatus(status))
          .catch(() => undefined);
        updateTrayStatus("idle");
      }
    });
  }

  function discardStreamingTtsForTextOnly() {
    for (const { audio, url } of streamTtsQueueRef.current.values()) {
      try {
        audio.pause();
      } catch {
        // ignore
      }
      if (typeof URL.revokeObjectURL === "function") {
        URL.revokeObjectURL(url);
      }
    }
    streamTtsQueueRef.current.clear();
    streamTtsNextPhraseRef.current = 1;
    ttsActiveTurnIdRef.current = null;
    void setAssistantSpeaking(false)
      .then((status) => setCoworkingStatus(status))
      .catch(() => undefined);
    updateTrayStatus("idle");
  }

  // immediately stop all streaming tts playback and drain the queue.
  // called on barge-in and before any new streaming turn starts.
  function stopStreamingTtsPlayback() {
    if (streamTtsDelayTimerRef.current !== null) {
      window.clearTimeout(streamTtsDelayTimerRef.current);
      streamTtsDelayTimerRef.current = null;
    }

    if (streamTtsCurrentRef.current) {
      try {
        streamTtsCurrentRef.current.pause();
      } catch {
        // ignore
      }
      streamTtsCurrentRef.current = null;
    }
    for (const { audio, url } of streamTtsQueueRef.current.values()) {
      try {
        audio.pause();
      } catch {
        // ignore
      }
      if (typeof URL.revokeObjectURL === "function") {
        URL.revokeObjectURL(url);
      }
    }
    streamTtsQueueRef.current.clear();
    streamTtsNextPhraseRef.current = 1;
    ttsActiveTurnIdRef.current = null;

    void setAssistantSpeaking(false)
      .then((status) => setCoworkingStatus(status))
      .catch(() => undefined);
    updateTrayStatus("idle");
  }

  // try to start web speech api recognition for partial transcripts.
  // on interim result with confidence >= 0.7, submits early and stops recording.
  // falls back gracefully if the api is not available.
  function tryStartPartialStt(taskId: number) {
    const win = window as unknown as Record<string, unknown>;
    const SpeechRecognitionCtor = (win["SpeechRecognition"] ?? win["webkitSpeechRecognition"]) as
      | (new () => PartialSpeechRecognition)
      | undefined;

    if (!SpeechRecognitionCtor) return;

    partialSttSentRef.current = false;
    let recognition: PartialSpeechRecognition;
    try {
      recognition = new SpeechRecognitionCtor();
    } catch {
      return;
    }

    recognition.interimResults = true;
    recognition.continuous = false;
    recognition.lang = "en-US";

    recognition.onresult = (event: SpeechRecognitionEvent) => {
      if (partialSttSentRef.current) return;
      for (let i = event.resultIndex; i < event.results.length; i++) {
        const result = event.results[i];
        if (
          !result.isFinal &&
          result[0] &&
          result[0].confidence >= 0.7 &&
          result[0].transcript.trim().length >= 5
        ) {
          partialSttSentRef.current = true;
          const text = result[0].transcript.trim();
          // stop the recorder — finalizeVoiceInput checks partialSttSentRef
          // and skips whisper since we already routed.
          void stopRecording();
          void submitRoutedMessage(taskId, text, "voice");
          break;
        }
      }
    };

    recognition.onerror = () => {
      speechRecognitionRef.current = null;
    };
    recognition.onend = () => {
      speechRecognitionRef.current = null;
    };

    try {
      recognition.start();
      speechRecognitionRef.current = recognition;
    } catch {
      speechRecognitionRef.current = null;
    }
  }

  function stopPartialStt() {
    if (speechRecognitionRef.current) {
      try {
        speechRecognitionRef.current.stop();
      } catch {
        // ignore
      }
      speechRecognitionRef.current = null;
    }
  }

  function stopMicrophoneStream() {
    if (mediaStreamRef.current) {
      for (const track of mediaStreamRef.current.getTracks()) {
        track.stop();
      }
      mediaStreamRef.current = null;
    }
  }

  function clearTypingSignal() {
    if (typingTimeoutRef.current !== null) {
      window.clearTimeout(typingTimeoutRef.current);
      typingTimeoutRef.current = null;
    }

    setTypingActivity(false);
  }

  function setTypingActivity(isTyping: boolean) {
    const allowed = privacyDashboard?.typing_activity_enabled !== false;
    const next = isTyping && allowed;
    userIsTypingRef.current = next;
    if (!next) {
      typingRateTimestampsRef.current = [];
    }

    void setUserTyping(next)
      .then((status) => setCoworkingStatus(status))
      .catch(() => undefined);

    if (!next) {
      scheduleStreamTtsPlayback();
    }
  }

  function noteRateOnlyKeydown() {
    if (!activeTask || privacyDashboard?.typing_activity_enabled === false) {
      return;
    }

    const now = Date.now();
    typingRateTimestampsRef.current = [...typingRateTimestampsRef.current, now].filter(
      (timestamp) => now - timestamp <= 2000
    );

    if (typingRateTimestampsRef.current.length > 1) {
      setTypingActivity(true);
    }

    if (typingTimeoutRef.current !== null) {
      window.clearTimeout(typingTimeoutRef.current);
    }
    typingTimeoutRef.current = window.setTimeout(() => {
      typingTimeoutRef.current = null;
      setTypingActivity(false);
    }, 2000);
  }

  function handleChatInputChange(value: string) {
    setChatInput(value);

    if (!activeTask) {
      return;
    }

    if (typingTimeoutRef.current !== null) {
      window.clearTimeout(typingTimeoutRef.current);
      typingTimeoutRef.current = null;
    }

    setTypingActivity(true);

    typingTimeoutRef.current = window.setTimeout(() => {
      typingTimeoutRef.current = null;
      setTypingActivity(false);
    }, 1200);
  }

  function handleArtifactSelection() {
    const textarea = artifactTextareaRef.current;
    if (!textarea) {
      return;
    }

    setArtifactSelectionStart(textarea.selectionStart);
    setArtifactSelectionEnd(textarea.selectionEnd);
  }

  async function handleProactiveToggle(enabled: boolean) {
    try {
      const status = await setProactiveMode(enabled);
      setCoworkingStatus(status);
    } catch (error) {
      setOperationError("Failed to toggle proactive mode", error);
    }
  }

  const statusLabel = formatStatusLabel(coworkingStatus?.state ?? "idle");
  const parsedSubtaskSnapshot = parseSubtaskSnapshot(subtaskDebug.contextSnapshot);

  return (
    <main className="shell">
      <header className="header-row">
        <div>
          <h1>jeff</h1>
        </div>
        <div className="row-actions">
          {onCloseWorkspace ? (
            <button
              type="button"
              className="workspace-back-btn"
              onClick={onCloseWorkspace}
              data-testid="close-workspace"
            >
              back to companion
            </button>
          ) : viewMode === "workspace" ? (
            <button type="button" onClick={() => setViewMode("home")}>
              back to home
            </button>
          ) : null}
          <button
            type="button"
            onClick={() => void handleOpenPrivacyCenter()}
            data-testid="privacy-center-open-home"
          >
            What Jeff knows
          </button>
        </div>
      </header>

      <section className="panel status-panel" data-testid="status-indicator">
        <h2>Status</h2>
        <p>
          <strong>{statusLabel}</strong>
          {coworkingStatus?.state === "speaking" ? " • Jeff is speaking" : ""}
        </p>
      </section>

      {selectionCaptureIndicator ? (
        <section
          className={`panel selection-capture-panel ${
            selectionCaptureIndicator.status === "failed" ? "selection-capture-panel-error" : ""
          }`}
          data-testid="selection-capture-indicator"
        >
          <div className="row-actions">
            <p>
              <strong>
                {selectionCaptureIndicator.status === "captured"
                  ? `Captured ${selectionCaptureIndicator.word_count} words from ${selectionCaptureIndicator.app_name}`
                  : selectionCaptureIndicator.message}
              </strong>
              {selectionCaptureIndicator.status === "captured" &&
              selectionCaptureIndicator.document_title ? (
                <span> • {selectionCaptureIndicator.document_title}</span>
              ) : null}
            </p>
            <button
              type="button"
              onClick={() => void handleDismissSelectionCapture()}
              data-testid="selection-capture-dismiss"
            >
              Dismiss
            </button>
          </div>
        </section>
      ) : null}

      {viewMode === "home" ? (
        <section className="panel" data-testid="home-resume-screen">
          <h2>Resume</h2>
          {loading ? <p>Loading task state...</p> : null}
          {!loading ? <p data-testid="resume-active-task">Last active task: {activeTaskLabel}</p> : null}

          <div className="row-actions">
            <button onClick={() => void handleContinueTask()} disabled={!activeTask} data-testid="continue-task-button">
              Continue Task
            </button>
          </div>

          {!loading && !activeTask ? (
            <section className="companion-card" data-testid="home-no-active-task-prompt">
              <p>Tell me what you're working on.</p>
              <form onSubmit={handleStartTaskFromPrompt} className="task-form">
                <input
                  aria-label="What are you working on?"
                  placeholder="Draft my StoryMap thesis intro and tighten evidence links"
                  value={chatInput}
                  onChange={(event) => setChatInput(event.target.value)}
                />
                <button type="submit" disabled={chatInput.trim().length === 0}>
                  Start
                </button>
              </form>
            </section>
          ) : null}

          <h3>Create Task</h3>
          <form onSubmit={handleCreateTask} className="task-form">
            <input
              aria-label="Task title"
              placeholder="history storymap"
              value={newTaskTitle}
              onChange={(event) => setNewTaskTitle(event.target.value)}
            />
            <button type="submit" disabled={newTaskTitle.trim().length === 0}>
              Create Task
            </button>
          </form>

          <h3>Tasks</h3>
          {tasks.length === 0 ? <p>No tasks yet.</p> : null}
          <ul className="task-list">
            {tasks.map((task) => (
              <li key={task.id} className="task-row">
                <div>
                  <strong>{task.title}</strong>
                  <p className="task-meta">slug: {task.slug}</p>
                </div>
                <div className="task-actions">
                  {task.is_active ? <span className="active-pill">Active</span> : null}
                  <button onClick={() => void handleSetActiveTask(task.id)} disabled={task.is_active}>
                    Set Active
                  </button>
                </div>
              </li>
            ))}
          </ul>
        </section>
      ) : null}

      {viewMode === "workspace" && activeTask ? (
        <section
          className={fullWorkspaceVisible ? "workspace-grid" : "workspace-grid companion-grid"}
          data-testid="workspace-screen"
        >
          {fullWorkspaceVisible ? (
            <div className="panel">
            {!onCloseWorkspace ? (
              <div className="row-actions">
                <button
                  onClick={() => setFullWorkspaceVisible(false)}
                  data-testid="toggle-full-workspace"
                >
                  companion view
                </button>
              </div>
            ) : null}
            <h2>Task Workspace</h2>
            <p data-testid="workspace-task-title">Task: {activeTask.title}</p>
            <p data-testid="active-task-summary">{taskSummary ? taskSummary.summary_text : "Summary unavailable."}</p>
            <p data-testid="active-task-workspace">
              {workspaceInfo ? workspaceInfo.workspace_path : "Workspace unavailable."}
            </p>

            <h3>Artifacts</h3>
            <form onSubmit={handleImportArtifact} className="task-form">
              <input
                aria-label="Artifact file path"
                placeholder="/absolute/path/to/notes.md"
                value={artifactPathInput}
                onChange={(event) => setArtifactPathInput(event.target.value)}
              />
              <button type="submit" disabled={artifactPathInput.trim().length === 0}>
                Import File
              </button>
            </form>

            {artifacts.length === 0 ? <p data-testid="artifacts-empty">No artifacts imported yet.</p> : null}
            {artifacts.length > 0 ? (
              <ul data-testid="artifacts-list" className="artifact-list">
                {artifacts.map((artifact) => {
                  const selected = artifact.id === selectedArtifactId;
                  const editable = isEditableArtifact(artifact);

                  return (
                    <li key={artifact.id} className={selected ? "artifact-item selected" : "artifact-item"}>
                      <button type="button" onClick={() => void handleSelectArtifact(artifact.id)}>
                        {artifact.file_name} ({artifact.chunk_count} chunks)
                        {editable ? "" : " [read-only]"}
                      </button>
                    </li>
                  );
                })}
              </ul>
            ) : null}

            <h3>SubTasks</h3>
            <form
              onSubmit={(event) => {
                event.preventDefault();
                void handleCreateSubtask("text");
              }}
              className="task-form"
              data-testid="subtask-create-form"
            >
              <input
                aria-label="Subtask instruction"
                placeholder="draft a better intro"
                value={subtaskInstruction}
                onChange={(event) => setSubtaskInstruction(event.target.value)}
                data-testid="subtask-instruction-input"
              />
              <select
                value={subtaskExecutionType}
                onChange={(event) => setSubtaskExecutionType(event.target.value as ExecutionType)}
                data-testid="subtask-execution-type"
              >
                <option value="draft_generation">Draft generation</option>
                <option value="expansion">Expansion</option>
                <option value="synthesis">Synthesis</option>
                <option value="targeted_research_synthesis">Targeted research synthesis</option>
              </select>
              <button type="submit" disabled={subtaskInstruction.trim().length === 0}>
                Create SubTask
              </button>
              <button
                type="button"
                onClick={() => void handleCreateStandingJobFromInstruction()}
                disabled={subtaskInstruction.trim().length === 0}
                data-testid="standing-job-create"
              >
                Make Standing Job
              </button>
              <button
                type="button"
                onClick={() => void startRecording("subtask")}
                disabled={recording}
                data-testid="voice-subtask-button"
              >
                Voice SubTask
              </button>
              <button
                type="button"
                onClick={() => void startRecording("cancel_subtask")}
                disabled={recording || activeSubtasks.length === 0}
                data-testid="voice-cancel-subtask-button"
              >
                Voice Cancel Running
              </button>
            </form>

            <div className="row-actions">
              <button type="button" onClick={() => void handleSuggestSubtask()} data-testid="subtask-suggest-button">
                Get Jeff Suggestion
              </button>
            </div>

            {subtaskSuggestion ? (
              <div className="subtask-suggestion" data-testid="subtask-suggestion-card">
                <p>
                  <strong>{subtaskSuggestion.title}</strong> ({subtaskSuggestion.execution_type})
                </p>
                <p>{subtaskSuggestion.description}</p>
                <p>{subtaskSuggestion.reason}</p>
                <div className="row-actions">
                  <button type="button" onClick={() => void handleAcceptSuggestedSubtask()}>
                    Accept Suggestion
                  </button>
                  <button type="button" onClick={() => setSubtaskSuggestion(null)}>
                    Ignore
                  </button>
                </div>
              </div>
            ) : null}

            <h3>Active SubTasks</h3>
            {activeSubtasks.length === 0 ? (
              <p data-testid="active-subtasks-empty">No active subtasks.</p>
            ) : (
              <ul data-testid="active-subtasks-list" className="subtask-list">
                {activeSubtasks.map((subtask) => {
                  const steps = subtaskStepsById[subtask.subtask_id] ?? [];
                  return (
                    <li key={subtask.subtask_id} className="subtask-item">
                      <p>
                        <strong>#{subtask.subtask_id}</strong> {subtask.title}
                      </p>
                      <p>
                        {subtask.status} • {subtask.execution_type} • source {subtask.instruction_source}
                      </p>
                      {steps.length > 0 ? (
                        <ul className="subtask-step-list" data-testid={`subtask-steps-${subtask.subtask_id}`}>
                          {steps.map((step) => (
                            <li key={step.id} className={`subtask-step subtask-step--${step.status}`} data-testid="subtask-step-item">
                              <span>{step.step_index + 1}. [{step.step_type}] {step.description.slice(0, 60)}</span>
                              <span> — {step.status}</span>
                              {step.error_message ? <span> — {step.error_message}</span> : null}
                            </li>
                          ))}
                        </ul>
                      ) : null}
                      <div className="row-actions">
                        <button type="button" onClick={() => void handleCancelSubtask(subtask.subtask_id)}>
                          Cancel
                        </button>
                        <button type="button" onClick={() => updateSubtaskDebugFromSubtask(subtask)}>
                          Inspect
                        </button>
                      </div>
                    </li>
                  );
                })}
              </ul>
            )}

            <h3>Completed SubTasks</h3>
            {completedSubtasks.length === 0 ? (
              <p data-testid="completed-subtasks-empty">No completed subtasks yet.</p>
            ) : (
              <ul data-testid="completed-subtasks-list" className="subtask-list">
                {completedSubtasks.map((subtask) => (
                  <li key={subtask.subtask_id} className="subtask-item">
                    <p>
                      <strong>#{subtask.subtask_id}</strong> {subtask.title}
                    </p>
                    <p>
                      {subtask.status} • review {subtask.result_review_status} • {subtask.execution_type}
                    </p>
                    {subtask.result_summary ? <p>{subtask.result_summary}</p> : null}
                    {subtask.result_payload ? <pre>{subtask.result_payload}</pre> : null}
                    {subtask.error_message ? <p>Error: {subtask.error_message}</p> : null}
                    <div className="row-actions">
                      <button type="button" onClick={() => void handleAcceptSubtaskResult(subtask.subtask_id)} disabled={subtask.status !== "completed"}>
                        Accept Result
                      </button>
                      <button type="button" onClick={() => void handleRejectSubtaskResult(subtask.subtask_id)} disabled={subtask.status !== "completed"}>
                        Reject Result
                      </button>
                      <button
                        type="button"
                        onClick={() => void handleConvertSubtaskToRevision(subtask.subtask_id)}
                        disabled={subtask.status !== "completed" || !artifactContent?.is_editable}
                      >
                        Convert to Revision
                      </button>
                      <button type="button" onClick={() => updateSubtaskDebugFromSubtask(subtask)}>
                        Inspect
                      </button>
                    </div>
                    {subtask.status === "completed" ? (
                      <form
                        onSubmit={(event) => {
                          event.preventDefault();
                          void handleRefineSubtask(subtask.subtask_id);
                        }}
                        className="task-form"
                      >
                        <input
                          aria-label={`Refine subtask ${subtask.subtask_id}`}
                          value={subtaskRefinementInputById[subtask.subtask_id] ?? ""}
                          onChange={(event) =>
                            setSubtaskRefinementInputById((current) => ({
                              ...current,
                              [subtask.subtask_id]: event.target.value
                            }))
                          }
                          placeholder="refine this output"
                          data-testid={`refine-subtask-input-${subtask.subtask_id}`}
                        />
                        <button
                          type="submit"
                          disabled={(subtaskRefinementInputById[subtask.subtask_id] ?? "").trim().length === 0}
                        >
                          Refine
                        </button>
                      </form>
                    ) : null}
                  </li>
                ))}
              </ul>
            )}

            {fileWriteProposals.length > 0 ? (
              <>
                <h3>Pending File Writes</h3>
                <ul data-testid="file-write-proposals-list" className="subtask-list">
                  {fileWriteProposals.map((proposal) => (
                    <li key={proposal.id} className="subtask-item" data-testid="file-write-proposal-item">
                      <p>
                        <strong>{proposal.proposed_path}</strong> — subtask #{proposal.subtask_id}
                      </p>
                      <pre>{proposal.proposed_content.slice(0, 400)}{proposal.proposed_content.length > 400 ? "\n..." : ""}</pre>
                      <div className="row-actions">
                        <button type="button" onClick={() => void handleApproveFileWrite(proposal.id)} data-testid="workspace-file-write-approve">
                          Approve &amp; write
                        </button>
                        <button type="button" onClick={() => void handleRejectFileWrite(proposal.id)} data-testid="workspace-file-write-reject">
                          Reject
                        </button>
                      </div>
                    </li>
                  ))}
                </ul>
              </>
            ) : null}

            {writeAuditLog.length > 0 ? (
              <>
                <h3>File Write Audit Log</h3>
                <ul data-testid="write-audit-log-list" className="subtask-list">
                  {writeAuditLog.map((entry) => (
                    <li key={entry.id} data-testid="write-audit-log-item">
                      [{entry.action}] {entry.proposed_path} — subtask #{entry.subtask_id} at {entry.resolved_at}
                    </li>
                  ))}
                </ul>
              </>
            ) : null}

            <h3>Artifact Editor</h3>
            {!artifactContent ? <p data-testid="artifact-editor-empty">Select an editable artifact.</p> : null}
            {artifactContent && !artifactContent.is_editable ? (
              <p data-testid="artifact-editor-readonly">This artifact is not editable in Phase 6 (.md/.txt only).</p>
            ) : null}
            {artifactContent && artifactContent.is_editable ? (
              <div className="editor-panel" data-testid="artifact-editor-panel">
                <p data-testid="active-artifact-id">Active artifact id: {artifactContent.artifact_id}</p>
                <textarea
                  ref={artifactTextareaRef}
                  className="artifact-editor"
                  value={artifactContent.content}
                  readOnly
                  onSelect={handleArtifactSelection}
                />
                <p data-testid="artifact-selection-range">
                  Selection: {artifactSelectionStart}..{artifactSelectionEnd}
                </p>
              </div>
            ) : null}

            <h3>Revision Request</h3>
            <form
                onSubmit={(event) => {
                  event.preventDefault();
                  void handleProposeRevision("typed");
                }}
                className="task-form"
              >
              <input
                aria-label="Revision instruction"
                placeholder="make this more analytical"
                value={revisionInstruction}
                onChange={(event) => setRevisionInstruction(event.target.value)}
                data-testid="revision-instruction-input"
              />
              <button type="submit" disabled={revisionInstruction.trim().length === 0 || !artifactContent?.is_editable}>
                Propose Revision
              </button>
              <button
                type="button"
                onClick={() => void startRecording("revision")}
                disabled={!artifactContent?.is_editable || recording}
                data-testid="voice-revision-button"
              >
                Voice Revision
              </button>
            </form>

            <h3>Revision Review</h3>
            {pendingRevisions.length === 0 ? (
              <p data-testid="pending-revisions-empty">No pending revisions.</p>
            ) : (
              <ul data-testid="pending-revisions-list" className="revision-list">
                {pendingRevisions.map((revision) => (
                  <li key={revision.revision_id} className="revision-item">
                    <p>
                      <strong>Revision #{revision.revision_id}</strong> • {revision.status}
                    </p>
                    <p>Instruction: {revision.instruction_text}</p>
                    <p>Target: {revision.target_description}</p>
                    <p>Original:</p>
                    <pre>{revision.original_text}</pre>
                    <p>Proposed:</p>
                    <pre>{revision.proposed_text}</pre>
                    <div className="row-actions">
                      <button type="button" onClick={() => void handleApplyRevision(revision.revision_id)}>
                        Accept
                      </button>
                      <button type="button" onClick={() => void handleRejectRevision(revision.revision_id)}>
                        Reject
                      </button>
                    </div>
                  </li>
                ))}
              </ul>
            )}

            <h3>Version History</h3>
            {artifactVersions.length === 0 ? (
              <p data-testid="artifact-versions-empty">No saved versions yet.</p>
            ) : (
              <ul data-testid="artifact-versions-list" className="version-list">
                {artifactVersions.map((version) => (
                  <li key={version.version_id} className="version-item">
                    <p>
                      <strong>Version #{version.version_id}</strong> • {version.version_reason}
                    </p>
                    <p>{version.content_preview}</p>
                    <button type="button" onClick={() => void handleRevertVersion(version.version_id)}>
                      Revert
                    </button>
                  </li>
                ))}
              </ul>
            )}

            <h3>Open Resources</h3>
            {openResources.length === 0 ? (
              <p data-testid="open-resources-empty">No open resources.</p>
            ) : (
              <ul>
                {openResources.map((resource) => (
                  <li key={resource.id}>{resource.label}</li>
                ))}
              </ul>
            )}
            </div>
          ) : null}

          <aside className="panel side-panel" data-testid="companion-view">
            <h2>Chat</h2>

            {!fullWorkspaceVisible ? (
              <button
                className="workspace-back-btn"
                onClick={() => setFullWorkspaceVisible(true)}
                data-testid="toggle-full-workspace"
              >
                workspace tools
              </button>
            ) : null}

            {!fullWorkspaceVisible ? (
              <section className="settings-panel companion-header" data-testid="companion-context-header">
                <div className="row-actions">
                  <p>
                    <strong>Task:</strong> {activeTask.title}
                  </p>
                  <button
                    type="button"
                    onClick={() => void handleToggleQuietMode()}
                    data-testid="quiet-mode-toggle"
                    title={quietMode ? "Quiet mode on — click to disable" : "Quiet mode off — click to enable"}
                  >
                    {quietMode ? "[Q]" : "[q]"}
                  </button>
                  <button
                    type="button"
                    onClick={() => void handleOpenPrivacyCenter()}
                    data-testid="privacy-center-open"
                  >
                    What Jeff knows
                  </button>
                </div>
                <p>{buildCompanionGreeting(activeTask, sessionModeState, artifacts)}</p>
                <p data-testid="companion-route-hint">Last routed intent: {lastRoutedIntent}</p>
                {privacyDashboard?.active_window_context_enabled !== false &&
                activeContext &&
                activeContext.document_title ? (
                  <p className="companion-context-line" data-testid="companion-active-context">
                    {activeContext.app_name} &mdash; {activeContext.document_title}
                  </p>
                ) : null}
                {privacyDashboard?.calendar_context_enabled && calendarEvent && calendarEvent.minutes_until <= 480 ? (
                  <p className="companion-context-line" data-testid="companion-calendar-event">
                    Meeting in {calendarEvent.minutes_until} min &mdash; {calendarEvent.title}
                  </p>
                ) : null}
              </section>
            ) : null}

            {!fullWorkspaceVisible &&
            onboardingStatus &&
            !onboardingStatus.preferred_workspace_folder &&
            !workspacePromptDismissed ? (
              <div className="companion-card" data-testid="companion-workspace-soft-prompt">
                <p>
                  Set a workspace folder to let Jeff learn from files automatically.
                  Jeff still works fully without one.
                </p>
                <div className="row-actions">
                  <button type="button" onClick={() => void handleOpenOnboarding()}>
                    Choose folder
                  </button>
                  <button type="button" onClick={() => void handleDismissWorkspacePrompt()}>
                    Skip for now
                  </button>
                </div>
              </div>
            ) : null}

            {!fullWorkspaceVisible &&
            onboardingStatus?.onboarding_complete &&
            privacyDashboard?.active_window_context_enabled !== false &&
            accessibilityPermissionGranted === false &&
            !accessibilityPromptDismissed &&
            !activeContext ? (
              <div className="companion-card" data-testid="accessibility-context-prompt">
                <p>Jeff needs accessibility permission to know which document you have open.</p>
                <div className="row-actions">
                  <button
                    type="button"
                    onClick={() => void handleRequestAccessibilityPermission()}
                    data-testid="request-accessibility-permission"
                  >
                    Enable
                  </button>
                  <button type="button" onClick={() => setAccessibilityPromptDismissed(true)}>
                    Not now
                  </button>
                </div>
              </div>
            ) : null}

            {driftNotice ? (
              <div className="companion-card" data-testid="drift-flag-notice">
                <p>Heads up: {driftNotice}</p>
                <button type="button" onClick={dismissDriftNotice} data-testid="drift-notice-dismiss">
                  Got it
                </button>
              </div>
            ) : null}

            {docSwitchBanner ? (
              <div className="companion-card" data-testid="doc-switch-banner">
                <p>
                  You switched to {docSwitchBanner.document_title}. Want to start or switch tasks?
                </p>
                <div className="row-actions">
                  <button
                    type="button"
                    onClick={() => void handleStartTaskFromDocumentTitle(docSwitchBanner.document_title)}
                    data-testid="doc-switch-start-task"
                  >
                    Start task
                  </button>
                  {docSwitchTaskCandidates.map((task) => (
                    <button
                      type="button"
                      key={task.id}
                      onClick={() => {
                        setDocSwitchBanner(null);
                        void handleSetActiveTask(task.id);
                      }}
                    >
                      Switch to {task.title}
                    </button>
                  ))}
                  <button
                    type="button"
                    onClick={() => setDocSwitchBanner(null)}
                    data-testid="doc-switch-dismiss"
                  >
                    Dismiss
                  </button>
                </div>
              </div>
            ) : null}

            {privacyCenterOpen ? (
              <section className="settings-panel privacy-center-panel" data-testid="privacy-center-panel">
                <div className="row-actions">
                  <h3>What Jeff knows</h3>
                  <button type="button" onClick={() => void refreshPrivacyCenter()} data-testid="privacy-center-refresh">
                    Refresh
                  </button>
                  <button type="button" onClick={() => setPrivacyCenterOpen(false)} data-testid="privacy-center-close">
                    Close
                  </button>
                </div>

                {privacyActionMessage ? (
                  <p data-testid="privacy-action-message">{privacyActionMessage}</p>
                ) : null}

                {privacyDashboard ? (
                  <>
                    <ul className="compact-list privacy-surface-list" data-testid="privacy-surface-list">
                      <li data-testid="privacy-surface-workspace">
                        <label className="toggle-row">
                          <input
                            type="checkbox"
                            checked={privacyDashboard.workspace_watcher_enabled}
                            onChange={(event) =>
                              void handleTogglePrivacySurface("workspace_watcher", event.target.checked)
                            }
                            data-testid="privacy-toggle-workspace-watcher"
                          />
                          Workspace watcher
                        </label>
                        <p className="task-meta">
                          {privacyDashboard.workspace_folder_path ?? "No folder set"};{" "}
                          {privacyDashboard.workspace_watched_file_count} files known;{" "}
                          {privacyDashboard.workspace_watcher_running ? "running" : "stopped"}
                        </p>
                      </li>

                      <li data-testid="privacy-surface-clipboard">
                        <label className="toggle-row">
                          <input
                            type="checkbox"
                            checked={privacyDashboard.clipboard_capture_enabled}
                            onChange={(event) =>
                              void handleTogglePrivacySurface("clipboard_capture", event.target.checked)
                            }
                            data-testid="privacy-toggle-clipboard-capture"
                          />
                          Clipboard capture
                        </label>
                        <p className="task-meta">{privacyDashboard.clipboard_capture_reminder}</p>
                      </li>

                      <li data-testid="privacy-surface-active-window">
                        <label className="toggle-row">
                          <input
                            type="checkbox"
                            checked={privacyDashboard.active_window_context_enabled}
                            onChange={(event) =>
                              void handleTogglePrivacySurface("active_window_context", event.target.checked)
                            }
                            data-testid="privacy-toggle-active-window-context"
                          />
                          Active window context
                        </label>
                        <p className="task-meta">
                          Accessibility permission: {privacyDashboard.accessibility_permission_status}
                        </p>
                      </li>

                      <li data-testid="privacy-surface-selection-capture">
                        <label className="toggle-row">
                          <input
                            type="checkbox"
                            checked={privacyDashboard.selection_capture_enabled}
                            onChange={(event) =>
                              void handleTogglePrivacySurface("selection_capture", event.target.checked)
                            }
                            data-testid="privacy-toggle-selection-capture"
                          />
                          Selection capture
                        </label>
                        <p className="task-meta">Only captures selected text after the capture hotkey or bridge action.</p>
                        {selectionBridgeStatus ? (
                          <p className="task-meta" data-testid="selection-bridge-status">
                            Browser bridge: 127.0.0.1:{selectionBridgeStatus.port}; token{" "}
                            <code>{selectionBridgeStatus.token}</code>
                          </p>
                        ) : null}
                        <p className="task-meta" data-testid="extension-install-instructions">
                          To enable text capture in Chrome or Google Docs: download the Jeff browser
                          extension, open <code>chrome://extensions</code>, enable Developer Mode,
                          click Load unpacked, and select the unzipped extension folder.
                        </p>
                      </li>

                      <li data-testid="privacy-surface-typing-activity">
                        <label className="toggle-row">
                          <input
                            type="checkbox"
                            checked={privacyDashboard.typing_activity_enabled}
                            onChange={(event) =>
                              void handleTogglePrivacySurface("typing_activity", event.target.checked)
                            }
                            data-testid="privacy-toggle-typing-activity"
                          />
                          Typing activity
                        </label>
                        <p className="task-meta">Rate-only; Jeff stores no key values or typed text.</p>
                      </li>

                      <li data-testid="privacy-surface-wake-word">
                        <label className="toggle-row">
                          <input
                            type="checkbox"
                            checked={privacyDashboard.wake_word?.enabled ?? false}
                            onChange={(event) => void handleToggleWakeWord(event.target.checked)}
                            data-testid="privacy-toggle-wake-word"
                          />
                          Wake word
                        </label>
                        <p className="task-meta" data-testid="wake-word-status">
                          "{privacyDashboard.wake_word?.phrase ?? "hey jeff"}";{" "}
                          {privacyDashboard.wake_word?.configured ? "detector configured" : "detector not configured"};{" "}
                          {privacyDashboard.wake_word?.running ? "running" : "stopped"};{" "}
                          {privacyDashboard.wake_word?.armed ? "armed" : "not armed"}
                          {privacyDashboard.wake_word?.sidecar_pid
                            ? `; pid ${privacyDashboard.wake_word.sidecar_pid}`
                            : ""}
                        </p>
                        <p className="task-meta" data-testid="wake-word-privacy-guarantee">
                          Pre-wake microphone audio stays inside the detector process; Jeff only receives a wake token.
                          Raw audio IPC: {privacyDashboard.wake_word?.no_raw_audio_ipc ?? true ? "disabled" : "enabled"}.
                        </p>
                        {privacyDashboard.wake_word?.last_error ? (
                          <p className="task-meta" data-testid="wake-word-error">
                            {privacyDashboard.wake_word.last_error}
                          </p>
                        ) : null}
                      </li>

                      <li data-testid="privacy-surface-proactive">
                        <label className="toggle-row">
                          <input
                            type="checkbox"
                            checked={privacyDashboard.proactive_triggers_enabled}
                            onChange={(event) =>
                              void handleTogglePrivacySurface("proactive_triggers", event.target.checked)
                            }
                            data-testid="privacy-toggle-proactive-triggers"
                          />
                          Proactive triggers
                        </label>
                        <p className="task-meta">Equivalent to quiet mode for proactive surfaces.</p>
                      </li>

                      <li data-testid="privacy-surface-crisis">
                        <label className="toggle-row">
                          <span>Override channel</span>
                        </label>
                        <p className="task-meta">
                          Deterministic emergency classes bypass ordinary interruption judgment.
                          Quiet mode downgrades delivery to a persistent card.
                        </p>
                        <div className="crisis-control-list" data-testid="crisis-control-list">
                          {(privacyDashboard.crisis_controls ?? []).map((control) => (
                            <label className="toggle-row" key={control.class} data-testid="crisis-class-control">
                              <input
                                type="checkbox"
                                checked={control.enabled}
                                onChange={(event) =>
                                  void handleToggleCrisisClass(control.class, event.target.checked)
                                }
                                data-testid={`privacy-toggle-crisis-${control.class}`}
                              />
                              {control.label}
                            </label>
                          ))}
                        </div>
                      </li>

                      <li data-testid="privacy-surface-action-receipts">
                        <label className="toggle-row">
                          <span>Action receipts</span>
                        </label>
                        <p className="task-meta">
                          Every mutation Jeff performs is recorded here with its class, surface, trust level, and undo state.
                        </p>
                        <div className="action-receipt-list" data-testid="action-receipt-list">
                          {(privacyDashboard.action_receipts ?? []).slice(0, 6).map((receipt) => (
                            <div className="action-receipt-row" key={receipt.id}>
                              <div>
                                <strong>#{receipt.id} {receipt.class}</strong>
                                <p className="task-meta">
                                  {receipt.surface}; {receipt.level}; {receipt.status}; {receipt.description}
                                </p>
                              </div>
                              {receipt.undo_ref && receipt.status === "applied" ? (
                                <button
                                  type="button"
                                  onClick={() => void handleRevertActionReceipt(receipt.id)}
                                  data-testid="action-receipt-revert"
                                >
                                  revert
                                </button>
                              ) : null}
                            </div>
                          ))}
                        </div>
                      </li>

                      <li data-testid="privacy-surface-speculation">
                        <label className="toggle-row">
                          <span>Speculation</span>
                          <input
                            type="checkbox"
                            checked={privacyDashboard.speculation.enabled}
                            onChange={(event) =>
                              void handleToggleSpeculation(event.target.checked)
                            }
                            data-testid="privacy-toggle-speculation"
                          />
                        </label>
                        <p className="task-meta">
                          When you are working but not talking to Jeff, it precomputes the likely
                          next request as a read-only job. Speculative work can never make changes.
                        </p>
                        <p className="task-meta" data-testid="speculation-stats">
                          Spend today: ${privacyDashboard.speculation.spent_today_usd.toFixed(2)} of $
                          {privacyDashboard.speculation.daily_budget_usd.toFixed(2)}; hit rate{" "}
                          {(privacyDashboard.speculation.hit_rate * 100).toFixed(0)}% (
                          {privacyDashboard.speculation.hit_count}/
                          {privacyDashboard.speculation.hit_count +
                            privacyDashboard.speculation.miss_count}
                          ); {privacyDashboard.speculation.fresh_cached} cached
                        </p>
                        <div className="compact-list" data-testid="speculation-cache-list">
                          {speculationCache.map((entry) => (
                            <div className="action-receipt-row" key={entry.id}>
                              <div>
                                <strong>#{entry.id}</strong>
                                <p className="task-meta">
                                  {entry.status}; {entry.request_text}
                                </p>
                              </div>
                              <button
                                type="button"
                                onClick={() => void handleDiscardSpeculation(entry.id)}
                                data-testid="speculation-discard"
                              >
                                discard
                              </button>
                            </div>
                          ))}
                        </div>
                      </li>

                      <li data-testid="privacy-surface-tool-bus">
                        <label className="toggle-row">
                          <span>Connections</span>
                        </label>
                        <p className="task-meta">
                          External tools (email, calendar, web, drive) connect through one governed
                          layer. Connections receive only the specific query, never your context.
                          Disconnecting stops calls immediately.
                        </p>
                        <div className="compact-list" data-testid="tool-connection-list">
                          {toolConnections.slice(0, 6).map((connection) => (
                            <div className="action-receipt-row" key={connection.id}>
                              <div>
                                <strong>{connection.name}</strong>
                                <p className="task-meta">
                                  {connection.transport}; {connection.enabled ? "connected" : "disconnected"};
                                  scopes: {connection.scopes.join(", ") || "none"}
                                </p>
                              </div>
                              <div className="row-actions">
                                <button
                                  type="button"
                                  data-testid="tool-connection-toggle"
                                  onClick={() =>
                                    void handleToggleToolConnection(connection.id, !connection.enabled)
                                  }
                                >
                                  {connection.enabled ? "disconnect" : "reconnect"}
                                </button>
                                <button
                                  type="button"
                                  data-testid="tool-connection-remove"
                                  onClick={() => void handleRemoveToolConnection(connection.id)}
                                >
                                  remove
                                </button>
                              </div>
                            </div>
                          ))}
                        </div>
                        <p className="task-meta">Recent tool calls</p>
                        <div className="compact-list" data-testid="tool-call-log">
                          {toolCallLog.slice(0, 6).map((entry) => (
                            <div key={entry.id}>
                              <p className="task-meta">
                                {entry.connection_name}.{entry.tool_name} {entry.argument_summary} [{entry.status}]
                              </p>
                            </div>
                          ))}
                        </div>
                      </li>

                      <li data-testid="privacy-surface-remote-docs">
                        <label className="toggle-row">
                          <span>Remote documents</span>
                        </label>
                        <p className="task-meta">
                          Documents pulled in from Drive or Docs are ingested with provenance.
                          Removing one purges its content from retrieval.
                        </p>
                        <div className="compact-list" data-testid="remote-doc-list">
                          {remoteDocs.slice(0, 6).map((doc) => (
                            <div className="action-receipt-row" key={doc.id}>
                              <div>
                                <strong>{doc.title}</strong>
                                <p className="task-meta">
                                  {doc.provenance}; {doc.url}
                                </p>
                              </div>
                              <button
                                type="button"
                                data-testid="remote-doc-remove"
                                onClick={() => void handleRemoveRemoteDoc(doc.id)}
                              >
                                remove
                              </button>
                            </div>
                          ))}
                        </div>
                      </li>

                      <li data-testid="privacy-surface-email">
                        <label className="toggle-row">
                          <span>Email</span>
                        </label>
                        <p className="task-meta">
                          Jeff reads and drafts email but never sends. Drafts are propose-only
                          (email.draft at L1). Reply watches notify you when a specific reply lands.
                        </p>
                        <div className="compact-list" data-testid="email-reply-watch-list">
                          {emailReplyWatches.slice(0, 6).map((watch) => (
                            <p className="task-meta" key={watch.id}>
                              watching {watch.sender} {watch.thread_hint ? `(${watch.thread_hint})` : ""} [{watch.status}]
                            </p>
                          ))}
                        </div>
                      </li>

                      <li data-testid="privacy-surface-web-research">
                        <label className="toggle-row">
                          <span>Web research</span>
                        </label>
                        <p className="task-meta">
                          Web search is rate-limited and logged. The user-name guard blocks searches
                          that mention a name you set. Every source used in a job is cited.
                        </p>
                        <div className="row-actions" data-testid="web-user-guard">
                          <input
                            type="text"
                            value={webUserGuard}
                            onChange={(event) => setWebUserGuard(event.target.value)}
                            placeholder="Block web searches mentioning this name"
                            data-testid="web-user-guard-input"
                          />
                          <button
                            type="button"
                            data-testid="web-user-guard-save"
                            onClick={() => void handleSaveWebUserGuard()}
                          >
                            save
                          </button>
                        </div>
                        <div className="compact-list" data-testid="web-query-log">
                          {webQueryLog.slice(0, 6).map((entry) => (
                            <p className="task-meta" key={entry.id}>
                              {entry.tool}: {entry.query} [{entry.status}] ({entry.result_count})
                            </p>
                          ))}
                        </div>
                      </li>

                      <li data-testid="privacy-surface-self-extend">
                        <label className="toggle-row">
                          <span>Self-built tools</span>
                        </label>
                        <p className="task-meta">
                          When Jeff hits a capability wall repeatedly, it can propose a tool.
                          Self-built tools are propose-only (capped at L1) and individually killable.
                        </p>
                        <div className="compact-list" data-testid="capability-gap-list">
                          {capabilityGaps.slice(0, 6).map((gap) => (
                            <div className="action-receipt-row" key={gap.id}>
                              <div>
                                <strong>{gap.surface}</strong>
                                <p className="task-meta">
                                  {gap.description}; seen {gap.occurrence_count}x
                                </p>
                              </div>
                              {gap.occurrence_count >= 2 ? (
                                <button
                                  type="button"
                                  onClick={() => void handleProposeCustomTool(gap.id)}
                                  data-testid="capability-gap-propose"
                                >
                                  propose tool
                                </button>
                              ) : null}
                            </div>
                          ))}
                        </div>
                        <div className="compact-list" data-testid="custom-tool-list">
                          {customTools.slice(0, 6).map((tool) => (
                            <div className="action-receipt-row" key={tool.id}>
                              <div>
                                <strong>#{tool.id} {tool.name}</strong>
                                <p className="task-meta">
                                  {tool.kind}; L1; {tool.status}; {tool.purpose}
                                </p>
                              </div>
                              {tool.status === "staged" ? (
                                <button
                                  type="button"
                                  onClick={() => void handleApproveCustomTool(tool.id)}
                                  data-testid="custom-tool-approve"
                                >
                                  approve
                                </button>
                              ) : tool.status === "installed" ? (
                                <button
                                  type="button"
                                  onClick={() => void handleKillCustomTool(tool.id)}
                                  data-testid="custom-tool-kill"
                                >
                                  kill
                                </button>
                              ) : null}
                            </div>
                          ))}
                        </div>
                      </li>

                      <li data-testid="privacy-surface-trust-ladder">
                        <label className="toggle-row">
                          <span>Trust ladder</span>
                        </label>
                        <p className="task-meta">
                          Autonomy is per action class. Email send, file delete, and custom tools are capped at L1.
                        </p>
                        <div className="trust-ladder-list" data-testid="trust-ladder-list">
                          {(privacyDashboard.trust_ladder ?? []).map((entry) => (
                            <div className="trust-ladder-row" key={entry.class} data-testid="trust-ladder-row">
                              <div>
                                <strong>{entry.class}</strong>
                                <p className="task-meta">
                                  {entry.level} of {entry.max_level}; streak {entry.approval_streak};{" "}
                                  {entry.sticky_l1 ? "sticky L1" : "eligible"}
                                </p>
                                {entry.graduation_offer ? (
                                  <p className="task-meta" data-testid="trust-graduation-offer">
                                    {entry.graduation_offer}
                                  </p>
                                ) : null}
                                {entry.recent_history.length > 0 ? (
                                  <p className="task-meta" data-testid="trust-history">
                                    Last: {entry.recent_history[0].status} #{entry.recent_history[0].id}
                                  </p>
                                ) : null}
                              </div>
                              <div className="row-actions trust-ladder-actions">
                                {entry.max_level !== "L1" &&
                                entry.level === "L1" &&
                                entry.graduation_offer?.startsWith("Offer L2") ? (
                                  <button
                                    type="button"
                                    data-testid="trust-offer-l2"
                                    onClick={() => void handleSetTrustLevel(entry.class, "L2")}
                                  >
                                    L2
                                  </button>
                                ) : null}
                                {entry.max_level === "L3" &&
                                entry.level === "L2" &&
                                entry.graduation_offer?.includes("L3 is available") ? (
                                  <button
                                    type="button"
                                    data-testid="trust-explicit-l3"
                                    onClick={() => void handleSetTrustLevel(entry.class, "L3")}
                                  >
                                    L3
                                  </button>
                                ) : null}
                                {entry.level !== "L1" ? (
                                  <button
                                    type="button"
                                    data-testid="trust-demote"
                                    onClick={() => void handleDemoteTrustClass(entry.class)}
                                  >
                                    L1
                                  </button>
                                ) : null}
                              </div>
                            </div>
                          ))}
                        </div>
                      </li>

                      <li data-testid="privacy-surface-native-docs">
                        <label className="toggle-row">
                          <span>Native docs</span>
                          <span className="status-pill">
                            {privacyDashboard.native_docs.ax_buffer_writeback_enabled ? "AX fallback on" : "AX fallback off"}
                          </span>
                        </label>
                        <p className="task-meta" data-testid="native-docs-automation-explainer">
                          {privacyDashboard.native_docs.automation_permission_explainer}
                        </p>
                        <p className="task-meta">
                          Pages {privacyDashboard.native_docs.pages_supported ? "supported" : "unavailable"}; Word{" "}
                          {privacyDashboard.native_docs.word_supported ? "supported" : "unavailable"}; automation{" "}
                          {privacyDashboard.native_docs.automation_permission_status.replace(/_/g, " ")}.
                        </p>
                      </li>

                      <li data-testid="privacy-surface-profile">
                        <label className="toggle-row">
                          <input
                            type="checkbox"
                            checked={privacyDashboard.user_profile_memory_enabled}
                            onChange={(event) =>
                              void handleTogglePrivacySurface("user_profile_memory", event.target.checked)
                            }
                            data-testid="privacy-toggle-user-profile-memory"
                          />
                          User profile memory
                        </label>
                        <p className="task-meta">{privacyDashboard.user_profile_signal_count} signals stored.</p>
                        <button
                          type="button"
                          onClick={() => void handleClearUserProfileMemory()}
                          data-testid="privacy-clear-user-profile"
                        >
                          Clear memory
                        </button>
                      </li>

                      <li data-testid="privacy-surface-memory-panel">
                        <label className="toggle-row">
                          <span>Memory</span>
                        </label>
                        <p className="task-meta">
                          {memoryFacts.length} facts; {memoryEpisodes.length} recent episodes.
                        </p>
                        <div className="row-actions">
                          <button
                            type="button"
                            onClick={() => void handleRunMemoryConsolidation()}
                            disabled={memoryConsolidationBusy || !privacyDashboard.user_profile_memory_enabled}
                            data-testid="memory-consolidate-now"
                          >
                            Consolidate
                          </button>
                          <button
                            type="button"
                            onClick={() => void refreshPrivacyCenter()}
                            data-testid="memory-refresh"
                          >
                            Refresh
                          </button>
                        </div>

                        <p className="task-meta">Facts</p>
                        {memoryFacts.length === 0 ? (
                          <p data-testid="memory-facts-empty">No consolidated memory facts.</p>
                        ) : (
                          <ul className="compact-list" data-testid="memory-facts-list">
                            {memoryFacts.map((fact) => (
                              <li key={`memory-fact-${fact.id}`} data-testid="memory-fact">
                                <span>
                                  {formatMemoryKind(fact.kind)}: {fact.text}
                                  {` (${evidenceCount(fact.evidence_ids_json)} evidence)`}
                                </span>
                                <button
                                  type="button"
                                  onClick={() => void handleDeleteFact(fact.id)}
                                  data-testid="memory-delete-fact"
                                >
                                  Remove
                                </button>
                              </li>
                            ))}
                          </ul>
                        )}

                        <p className="task-meta">Episodes</p>
                        {memoryEpisodes.length === 0 ? (
                          <p data-testid="memory-episodes-empty">No recent memory episodes for this task.</p>
                        ) : (
                          <ul className="compact-list" data-testid="memory-episodes-list">
                            {memoryEpisodes.slice(0, 8).map((episode) => (
                              <li key={`memory-episode-${episode.id}`} data-testid="memory-episode">
                                <span>
                                  {formatMemoryKind(episode.kind)}: {episode.text}
                                </span>
                                <button
                                  type="button"
                                  onClick={() => void handleDeleteEpisode(episode.id)}
                                  data-testid="memory-delete-episode"
                                >
                                  Remove
                                </button>
                              </li>
                            ))}
                          </ul>
                        )}

                        {memoryPromptPreview ? (
                          <pre className="privacy-memory-preview" data-testid="memory-prompt-preview">
                            {memoryPromptPreview}
                          </pre>
                        ) : null}
                      </li>

                      <li data-testid="privacy-surface-local-runtime">
                        <label className="toggle-row">
                          <span>Local model runtime</span>
                        </label>
                        <p className="task-meta" data-testid="local-runtime-status">
                          {privacyDashboard.local_runtime.mode};{" "}
                          {privacyDashboard.local_runtime.healthy ? "healthy" : "not healthy"};{" "}
                          {privacyDashboard.local_runtime.running ? "running" : "stopped"};{" "}
                          {privacyDashboard.local_runtime.endpoint}
                        </p>
                        <p className="task-meta" data-testid="local-runtime-models">
                          Reflex:{" "}
                          {privacyDashboard.local_runtime.reasoning_model_present ? "installed" : "missing"};{" "}
                          embeddings:{" "}
                          {privacyDashboard.local_runtime.embedding_model_present ? "installed" : "hash fallback"};{" "}
                          {privacyDashboard.local_runtime.installed_model_bytes} bytes installed
                        </p>
                        <p className="task-meta" data-testid="local-runtime-model-dir">
                          <code>{privacyDashboard.local_runtime.model_dir}</code>
                        </p>
                        {privacyDashboard.local_runtime.last_error ? (
                          <p className="task-meta" data-testid="local-runtime-last-error">
                            {privacyDashboard.local_runtime.last_error}
                          </p>
                        ) : null}
                        <div className="row-actions">
                          <button
                            type="button"
                            onClick={() => void handleStartLocalRuntime()}
                            disabled={localModelBusy}
                            data-testid="local-runtime-start"
                          >
                            Start
                          </button>
                          <button
                            type="button"
                            onClick={() => void handleStopLocalRuntime()}
                            disabled={localModelBusy}
                            data-testid="local-runtime-stop"
                          >
                            Stop
                          </button>
                          <button
                            type="button"
                            onClick={() => void handleDeleteLocalModel("reasoning")}
                            disabled={localModelBusy || !privacyDashboard.local_runtime.reasoning_model_present}
                            data-testid="local-runtime-delete-reasoning"
                          >
                            Delete Reflex
                          </button>
                          <button
                            type="button"
                            onClick={() => void handleDeleteLocalModel("embedding")}
                            disabled={localModelBusy || !privacyDashboard.local_runtime.embedding_model_present}
                            data-testid="local-runtime-delete-embedding"
                          >
                            Delete embeddings
                          </button>
                        </div>
                        <input
                          className="inline-input"
                          value={localModelUrl}
                          onChange={(event) => setLocalModelUrl(event.target.value)}
                          placeholder="https://...gguf"
                          disabled={localModelBusy}
                          data-testid="local-model-url"
                        />
                        <input
                          className="inline-input"
                          value={localModelSha256}
                          onChange={(event) => setLocalModelSha256(event.target.value)}
                          placeholder="sha256"
                          disabled={localModelBusy}
                          data-testid="local-model-sha256"
                        />
                        <input
                          className="inline-input"
                          value={localModelExpectedBytes}
                          onChange={(event) => setLocalModelExpectedBytes(event.target.value)}
                          placeholder="expected bytes"
                          disabled={localModelBusy}
                          data-testid="local-model-expected-bytes"
                        />
                        <div className="row-actions">
                          <button
                            type="button"
                            onClick={() => void handleDownloadLocalModel("reasoning")}
                            disabled={localModelBusy || !localModelUrl.trim() || !localModelSha256.trim()}
                            data-testid="local-model-download-reasoning"
                          >
                            Install Reflex
                          </button>
                          <button
                            type="button"
                            onClick={() => void handleDownloadLocalModel("embedding")}
                            disabled={localModelBusy || !localModelUrl.trim() || !localModelSha256.trim()}
                            data-testid="local-model-download-embedding"
                          >
                            Install embeddings
                          </button>
                        </div>
                        <p className="task-meta" data-testid="embedding-mode-status">
                          Embeddings:{" "}
                          {privacyDashboard.local_runtime.semantic_embedding_available
                            ? "semantic (on-device bge-small)"
                            : "lexical fallback (keyword hashing)"}
                        </p>
                        <p className="task-meta">
                          Semantic recall needs a real on-device embedding model.
                          Install it in one click; it stays on your device.
                        </p>
                        <div className="row-actions">
                          <button
                            type="button"
                            onClick={() => void handleDownloadCuratedEmbeddingModel()}
                            disabled={localModelBusy || privacyDashboard.local_runtime.semantic_embedding_available}
                            data-testid="local-model-download-semantic-embedding"
                          >
                            Download semantic embedding model (~35 MB)
                          </button>
                        </div>
                      </li>

                      <li data-testid="privacy-surface-interruptions">
                        <label className="toggle-row">
                          <span>Interruptions</span>
                        </label>
                        <p className="task-meta" data-testid="interruption-self-audit">
                          {interruptionAudit
                            ? `Jeff spoke ${interruptionAudit.delivered} ${
                                interruptionAudit.delivered === 1 ? "time" : "times"
                              } in the last ${interruptionAudit.days} days; you engaged with ${
                                interruptionAudit.engaged
                              }.`
                            : "No interjections recorded yet."}
                        </p>
                        <label className="toggle-row">
                          <span>End-of-day debrief</span>
                          <input
                            type="checkbox"
                            data-testid="privacy-toggle-debrief"
                            checked={debriefEnabled}
                            onChange={(event) => void handleToggleDebrief(event.target.checked)}
                          />
                        </label>
                        <p className="task-meta">
                          When on, Jeff closes the day with a short debrief. The morning briefing
                          rides your existing proactive setting.
                        </p>
                        <label className="toggle-row">
                          <span>Realtime voice</span>
                          <input
                            type="checkbox"
                            data-testid="privacy-toggle-voice"
                            checked={voiceEnabled}
                            onChange={(event) => void handleToggleVoice(event.target.checked)}
                          />
                        </label>
                        <p className="task-meta">
                          When on, talking to Jeff uses a full-duplex realtime voice session; off
                          uses the standard speech pipeline.
                        </p>
                      </li>

                      <li data-testid="privacy-surface-spend">
                        <label className="toggle-row">
                          <span>Spend</span>
                        </label>
                        <p className="task-meta" data-testid="cost-governor-today">
                          Today: {formatSpendUsd(privacyDashboard.cost_governor.today_total_usd)}
                        </p>
                        {privacyDashboard.cost_governor.last_notice ? (
                          <p className="task-meta" data-testid="cost-governor-notice">
                            {privacyDashboard.cost_governor.last_notice}
                          </p>
                        ) : null}
                        <div className="cost-tier-list" data-testid="cost-tier-list">
                          {privacyDashboard.cost_governor.tiers.map((tier) => (
                            <div
                              className={tier.over_budget ? "cost-tier-row cost-tier-row-over" : "cost-tier-row"}
                              data-testid={`cost-tier-${tier.tier}`}
                              key={tier.budget_key}
                            >
                              <div className="cost-tier-header">
                                <span>{tier.tier}</span>
                                <span>
                                  {formatSpendUsd(tier.spent_usd)} / {formatSpendUsd(tier.budget_usd)}
                                </span>
                              </div>
                              <progress
                                aria-label={`${tier.tier} spend`}
                                max={1}
                                value={budgetProgressValue(tier.spent_usd, tier.budget_usd)}
                              />
                              <div className="row-actions">
                                <input
                                  className="inline-input cost-budget-input"
                                  type="number"
                                  min="0"
                                  step="0.01"
                                  defaultValue={tier.budget_usd.toFixed(2)}
                                  onBlur={(event) =>
                                    void handleSetLlmDailyBudget(tier.budget_key, event.currentTarget.value)
                                  }
                                  onKeyDown={(event) => {
                                    if (event.key === "Enter") {
                                      event.currentTarget.blur();
                                    }
                                  }}
                                  data-testid={`cost-budget-${tier.tier}`}
                                />
                                {tier.degrade_to ? (
                                  <span className="task-meta">Degrades to {tier.degrade_to}</span>
                                ) : null}
                              </div>
                            </div>
                          ))}
                        </div>
                        <div className="cost-history-list" data-testid="cost-history-list">
                          {privacyDashboard.cost_governor.history.length > 0 ? (
                            privacyDashboard.cost_governor.history.map((entry) => (
                              <span key={entry.date}>
                                {entry.date}: {formatSpendUsd(entry.total_usd)}
                              </span>
                            ))
                          ) : (
                            <span>No spend in the last 7 days.</span>
                          )}
                        </div>
                      </li>

                      <li data-testid="privacy-surface-calendar">
                        <label className="toggle-row">
                          <input
                            type="checkbox"
                            checked={privacyDashboard.calendar_context_enabled}
                            onChange={(event) =>
                              void handleTogglePrivacySurface("calendar_context", event.target.checked)
                            }
                            data-testid="privacy-toggle-calendar-context"
                          />
                          Calendar context
                        </label>
                        <p className="task-meta">Permission: {privacyDashboard.calendar_permission_status}</p>
                        {privacyDashboard.calendar_context_enabled &&
                        privacyDashboard.calendar_permission_status !== "granted" ? (
                          <button
                            type="button"
                            onClick={() => void handleRequestCalendarPermission()}
                            data-testid="request-calendar-permission"
                          >
                            Enable calendar permission
                          </button>
                        ) : null}
                      </li>

                      {activeTask ? (
                        <li data-testid="privacy-surface-content-observation">
                          <label className="toggle-row">
                            <input
                              type="checkbox"
                              checked={privacyDashboard.content_observation_enabled}
                              onChange={(event) =>
                                void (async () => {
                                  const updated = await setContentObservationEnabled(
                                    activeTask.id,
                                    event.target.checked
                                  );
                                  setPrivacyDashboard(updated);
                                })()
                              }
                              data-testid="privacy-toggle-content-observation"
                            />
                            Active document reading
                          </label>
                          <p className="task-meta">
                            Jeff will periodically read the text in your active document window or
                            an enabled Google Docs tab to give you better feedback. This text never leaves your device.
                            Google Docs also requires the extension's per-site toggle. Comprehension
                            passes are recorded as memory episodes in this audit.
                          </p>
                          {privacyDashboard.content_observation_enabled ? (
                            <p className="task-meta" data-testid="content-observation-status">
                              {privacyDashboard.content_observation_capture_failed
                                ? `Could not read text — this app may restrict accessibility access.`
                                : privacyDashboard.content_observation_last_captured_at
                                ? `Last read: ${new Date(Number(privacyDashboard.content_observation_last_captured_at) * 1000).toLocaleTimeString()}${
                                    privacyDashboard.content_observation_source_origin
                                      ? ` from ${
                                          privacyDashboard.content_observation_document_title ||
                                          privacyDashboard.content_observation_source_origin
                                        }`
                                      : ""
                                  }`
                                : "Not yet captured"}
                            </p>
                          ) : null}
                          {privacyDashboard.content_observation_enabled ? (
                            <button
                              type="button"
                              onClick={() => void clearContentObservation()}
                              data-testid="content-observation-clear"
                            >
                              Clear
                            </button>
                          ) : null}
                        </li>
                      ) : null}

                      <li data-testid="privacy-surface-voice">
                        <label className="toggle-row">
                          <span>Jeff voice</span>
                          <select
                            value={privacyDashboard.tts_voice}
                            onChange={(event) => void handleSetTtsVoice(event.target.value)}
                            data-testid="tts-voice-select"
                          >
                            {privacyDashboard.available_tts_voices.map((voice) => (
                              <option key={voice} value={voice}>
                                {voice}
                              </option>
                            ))}
                          </select>
                        </label>
                        <p className="task-meta">Takes effect on the next spoken response.</p>
                      </li>
                    </ul>

                    <h4 className="compact-heading">Audit</h4>
                    {activeTask ? (
                      <>
                        <p className="task-meta">Write decisions</p>
                        {writeAuditLog.length === 0 ? (
                          <p data-testid="privacy-write-audit-empty">No write decisions for this task.</p>
                        ) : (
                          <ul className="compact-list" data-testid="privacy-write-audit-list">
                            {writeAuditLog.map((entry) => (
                              <li key={`privacy-write-${entry.id}`}>
                                {entry.action} {entry.proposed_path} at {entry.resolved_at}
                              </li>
                            ))}
                          </ul>
                        )}

                        <p className="task-meta">Proactive triggers</p>
                        {proactiveAuditLog.length === 0 ? (
                          <p data-testid="privacy-proactive-audit-empty">No proactive triggers for this task.</p>
                        ) : (
                          <ul className="compact-list" data-testid="privacy-proactive-audit-list">
                            {proactiveAuditLog.map((entry) => (
                              <li key={`privacy-trigger-${entry.id}`}>
                                {entry.trigger_type} at {entry.fired_at}{" "}
                                {entry.suppressed ? "(suppressed)" : "(surfaced)"}
                              </li>
                            ))}
                          </ul>
                        )}

                        <p className="task-meta">Synthesis decisions</p>
                        {synthesisLog.length === 0 ? (
                          <p data-testid="privacy-synthesis-audit-empty">No synthesis decisions for this task.</p>
                        ) : (
                          <ul className="compact-list" data-testid="privacy-synthesis-audit-list">
                            {synthesisLog.map((entry) => (
                              <li key={`privacy-synthesis-${entry.id}`}>
                                {entry.reason_type} at {entry.created_at}{" "}
                                {entry.delivered ? "(delivered)" : "(suppressed)"}
                                {entry.reason_detail ? ` - ${entry.reason_detail}` : ""}
                              </li>
                            ))}
                          </ul>
                        )}
                      </>
                    ) : (
                      <p data-testid="privacy-audit-no-task">No active task, so there is no task audit yet.</p>
                    )}

                    <h4 className="compact-heading">Data controls</h4>
                    <div className="row-actions">
                      <button
                        type="button"
                        onClick={() => void handleClearActiveTaskData()}
                        disabled={!activeTask}
                        data-testid="privacy-clear-active-task-data"
                      >
                        Clear active task data
                      </button>
                    </div>

                    <label className="toggle-row">
                      <span>Type CLEAR JEFF to clear all Jeff data</span>
                      <input
                        type="text"
                        value={clearAllConfirmation}
                        onChange={(event) => setClearAllConfirmation(event.target.value)}
                        data-testid="privacy-clear-all-confirmation"
                      />
                    </label>
                    <button
                      type="button"
                      onClick={() => void handleClearAllJeffData()}
                      disabled={clearAllConfirmation.trim() !== "CLEAR JEFF"}
                      data-testid="privacy-clear-all-data"
                    >
                      Clear all Jeff data
                    </button>
                  </>
                ) : (
                  <p data-testid="privacy-center-loading">Loading privacy state.</p>
                )}
              </section>
            ) : null}

            {/* phase 23: cross-task collision notice */}
            {collisionNotice ? (
              <div className="companion-card" data-testid="collision-notice">
                <p>{collisionNotice}</p>
                <button type="button" onClick={() => setCollisionNotice(null)}>Dismiss</button>
              </div>
            ) : null}

            {/* phase 23: pending live edit preview cards */}
            {pendingLiveEdits.map((edit) => (
              <div
                key={edit.receipt_id}
                className="companion-card"
                data-testid="live-edit-preview-card"
              >
                <p>
                  <strong>
                    {edit.status === "pending_approval" ? "Apply edit" : "Manual apply needed"} in{" "}
                    {edit.editor_surface}
                  </strong>{" "}
                  &mdash; {edit.document_title}
                </p>
                {edit.editor_surface.toLowerCase().includes("google") ? (
                  <p className="task-meta" data-testid="google-docs-tracked-change-note">
                    Google Docs edits use the anchored browser adapter. If suggesting mode is active, the change appears as a native suggestion; anchor drift falls back to guided apply.
                  </p>
                ) : null}
                <div className="live-edit-diff" data-testid="live-edit-diff">
                  <div className="live-edit-before">
                    <p className="task-meta">Before</p>
                    <pre>{edit.before_text}</pre>
                  </div>
                  <div className="live-edit-after">
                    <p className="task-meta">After</p>
                    <pre>{edit.after_text}</pre>
                  </div>
                </div>
                {edit.status === "pending_approval" ? (
                  <div className="row-actions">
                    <button
                      type="button"
                      data-testid="live-edit-approve"
                      onClick={() => {
                        void approveLiveEdit(edit.receipt_id).then(() =>
                          getPendingLiveEdits().then(setPendingLiveEdits)
                        ).catch(() => undefined);
                      }}
                    >
                      Apply
                    </button>
                    <button
                      type="button"
                      data-testid="live-edit-reject"
                      onClick={() => {
                        void rejectLiveEdit(edit.receipt_id).then(() =>
                          getPendingLiveEdits().then(setPendingLiveEdits)
                        ).catch(() => undefined);
                      }}
                    >
                      Reject
                    </button>
                  </div>
                ) : (
                  <>
                    <p className="task-meta" data-testid="guided-apply-fallback">
                      The document changed or the anchor moved. Paste this manually where it belongs.
                    </p>
                    <div className="row-actions">
                      <button
                        type="button"
                        data-testid="live-edit-copy"
                        onClick={() => {
                          if (navigator.clipboard) {
                            void navigator.clipboard.writeText(edit.after_text).catch(() => undefined);
                          }
                        }}
                      >
                        Copy replacement
                      </button>
                      <button
                        type="button"
                        data-testid="live-edit-dismiss-fallback"
                        onClick={() => {
                          void rejectLiveEdit(edit.receipt_id).then(() =>
                            getPendingLiveEdits().then(setPendingLiveEdits)
                          ).catch(() => undefined);
                        }}
                      >
                        Dismiss
                      </button>
                    </div>
                  </>
                )}
              </div>
            ))}

            {/* phase 23: "Jeff remembers" panel */}
            {privacyDashboard?.user_profile_memory_enabled !== false ? (
              <section
                className="settings-panel jeff-remembers-panel"
                data-testid="jeff-remembers-panel"
              >
                <div className="row-actions">
                  <button
                    type="button"
                    onClick={() => setJeffRemembersOpen((o) => !o)}
                    data-testid="jeff-remembers-toggle"
                  >
                    {jeffRemembersOpen ? "Hide" : "Show"} what Jeff remembers
                    {` (${rememberedSignalCount})`}
                  </button>
                  {jeffRemembersOpen && rememberedSignalCount > 0 ? (
                    <button
                      type="button"
                      data-testid="jeff-remembers-clear-all"
                      onClick={() => {
                        void clearUserProfileMemory()
                          .then(() => refreshPrivacyCenter())
                          .then(() => setJeffRemembersOpen(false))
                          .catch(() => undefined);
                      }}
                    >
                      Clear all
                    </button>
                  ) : null}
                </div>
                {jeffRemembersOpen ? (
                  <div data-testid="jeff-remembers-list">
                    {activeStatedGoals.length > 0 ? (
                      <section className="memory-section" data-testid="jeff-remembers-goals">
                        <h4>Goals</h4>
                        <ul>
                          {activeStatedGoals.map((goal) => (
                            <li key={goal.id} data-testid="jeff-remembers-goal">
                              <span>{goal.goal_text}</span>
                              <button
                                type="button"
                                data-testid="jeff-remembers-delete-goal"
                                onClick={() => {
                                  void deleteStatedGoal(goal.id)
                                    .then(setRelationalProfile)
                                    .catch(() => undefined);
                                }}
                              >
                                Remove
                              </button>
                            </li>
                          ))}
                        </ul>
                      </section>
                    ) : null}
                    {strugglePatterns.length > 0 ? (
                      <section className="memory-section" data-testid="jeff-remembers-patterns">
                        <h4>Patterns</h4>
                        <ul>
                          {strugglePatterns.map((pattern) => (
                            <li key={pattern.id} data-testid="jeff-remembers-pattern">
                              <span>{pattern.pattern_text}</span>
                              <button
                                type="button"
                                data-testid="jeff-remembers-delete-pattern"
                                onClick={() => {
                                  void deleteStrugglePattern(pattern.id)
                                    .then(setRelationalProfile)
                                    .catch(() => undefined);
                                }}
                              >
                                Remove
                              </button>
                            </li>
                          ))}
                        </ul>
                      </section>
                    ) : null}
                    {userProfileSignals.length > 0 ? (
                      <section className="memory-section" data-testid="jeff-remembers-communication">
                        <h4>Communication style</h4>
                        <ul>
                          {userProfileSignals.slice(0, 3).map((signal) => (
                            <li key={signal.key} data-testid="jeff-remembers-signal">
                              <span>{signal.label}</span>
                              <button
                                type="button"
                                data-testid="jeff-remembers-delete-signal"
                                onClick={() => {
                                  void deleteUserProfileSignal(signal.key)
                                    .then(setUserProfileSignals)
                                    .catch(() => undefined);
                                }}
                              >
                                Remove
                              </button>
                            </li>
                          ))}
                        </ul>
                      </section>
                    ) : null}
                  </div>
                ) : null}
                {jeffRemembersOpen ? (
                  <div className="row-actions">
                    <input
                      type="text"
                      value={rubricInput}
                      onChange={(e) => setRubricInput(e.target.value)}
                      placeholder='Quality note, e.g. "Always cite sources."'
                      data-testid="rubric-input"
                    />
                    <button
                      type="button"
                      data-testid="rubric-add"
                      onClick={() => {
                        if (!rubricInput.trim()) return;
                        void addQualityRubric(rubricInput.trim())
                          .then(setUserProfileSignals)
                          .then(() => setRubricInput(""))
                          .catch(() => undefined);
                      }}
                    >
                      Save
                    </button>
                  </div>
                ) : null}
              </section>
            ) : null}

            {/* phase 23: "Your workload" section */}
            {workloadSummary ? (
              <section
                className="settings-panel workload-panel"
                data-testid="workload-panel"
              >
                <div className="row-actions">
                  <button
                    type="button"
                    onClick={() => setWorkloadOpen((o) => !o)}
                    data-testid="workload-toggle"
                  >
                    {workloadOpen ? "Hide" : "Show"} your workload
                    {workloadSummary.active_tasks.length > 0
                      ? ` (${workloadSummary.active_tasks.length} active)`
                      : ""}
                  </button>
                  <button
                    type="button"
                    data-testid="workload-refresh"
                    onClick={() => {
                      void Promise.all([
                        getWorkloadSummary().then(setWorkloadSummary),
                        activeTask ? listAgentJobs(activeTask.id, 50).then(setAgentJobs) : Promise.resolve(),
                        activeTask ? listStandingJobs(activeTask.id).then(setStandingJobs) : Promise.resolve()
                      ]).catch(() => undefined);
                    }}
                  >
                    Refresh
                  </button>
                </div>
                {workloadOpen ? (
                  <>
                    {workloadSummary.active_tasks.length > 0 ? (
                      <div data-testid="workload-active-tasks">
                        <p className="task-meta">Active (last 14 days)</p>
                        <ul>
                          {workloadSummary.active_tasks.map((task) => (
                            <li key={task.id} data-testid="workload-task-item">
                              <button
                                type="button"
                                data-testid="workload-task-switch"
                                onClick={() => {
                                  void switchActiveTaskFromCompanion(task.id)
                                    .then((t) => setActiveTaskState(t))
                                    .then(() => getWorkloadSummary().then(setWorkloadSummary))
                                    .catch(() => undefined);
                                }}
                              >
                                {task.title}
                              </button>
                              {task.pending_item_count > 0 ? (
                                <span className="task-meta"> — {task.pending_item_count} pending</span>
                              ) : null}
                              {task.days_since_focus !== null ? (
                                <span className="task-meta">
                                  {" "}&mdash; {task.days_since_focus === 0 ? "today" : `${task.days_since_focus}d ago`}
                                </span>
                              ) : null}
                            </li>
                          ))}
                        </ul>
                      </div>
                    ) : null}
                    {agentJobs.length > 0 ? (
                      <div data-testid="agent-jobs-panel">
                        <p className="task-meta">Agent jobs</p>
                        <ul className="subtask-list" data-testid="agent-jobs-list">
                          {agentJobs.slice(0, 8).map((job) => (
                            <li key={job.id} className="subtask-item" data-testid="agent-job-item">
                              <button
                                type="button"
                                data-testid="agent-job-open"
                                onClick={() => {
                                  void getAgentJobDetail(job.id).then(setSelectedAgentJob).catch(() => undefined);
                                }}
                              >
                                #{job.id} {job.status}
                              </button>
                              <p className="task-meta">{job.goal_contract}</p>
                            </li>
                          ))}
                        </ul>
                      </div>
                    ) : null}
                    {standingJobs.length > 0 ? (
                      <div data-testid="standing-jobs-panel">
                        <div className="row-actions">
                          <p className="task-meta">Standing jobs</p>
                          <button
                            type="button"
                            data-testid="standing-jobs-run-due"
                            onClick={() => void handleRunDueStandingJobs()}
                          >
                            Run due
                          </button>
                        </div>
                        <ul className="subtask-list" data-testid="standing-jobs-list">
                          {standingJobs.slice(0, 8).map((job) => (
                            <li key={job.id} className="subtask-item" data-testid="standing-job-item">
                              <p>
                                <strong>#{job.id}</strong> {job.enabled ? "enabled" : "paused"}
                                {job.critical ? " critical" : ""}
                              </p>
                              <p className="task-meta">{job.schedule_spec}</p>
                              <p className="task-meta">Next run: {job.next_run_at}</p>
                              <p className="task-meta">{job.goal_contract}</p>
                              <div className="row-actions">
                                <button
                                  type="button"
                                  data-testid="standing-job-toggle"
                                  onClick={() => void handleToggleStandingJob(job.id, !job.enabled)}
                                >
                                  {job.enabled ? "Disable" : "Enable"}
                                </button>
                              </div>
                            </li>
                          ))}
                        </ul>
                      </div>
                    ) : null}
                    {selectedAgentJob ? (
                      <div className="subtask-item" data-testid="agent-job-detail">
                        <p>
                          <strong>Job #{selectedAgentJob.job.id}</strong> {selectedAgentJob.job.status}
                        </p>
                        <div className="row-actions" data-testid="agent-job-steering-controls">
                          <input
                            type="text"
                            value={jobSteeringInput}
                            onChange={(event) => setJobSteeringInput(event.target.value)}
                            placeholder="Steer this job"
                            data-testid="job-steering-input"
                          />
                          <button
                            type="button"
                            onClick={() => void handleSendJobSteering()}
                            disabled={jobSteeringInput.trim().length === 0}
                            data-testid="job-steering-send"
                          >
                            Send
                          </button>
                          <button
                            type="button"
                            onClick={() => void handleCancelAgentJob()}
                            disabled={[
                              "completed",
                              "blocked",
                              "budget_exhausted",
                              "cancelled_partial"
                            ].includes(selectedAgentJob.job.status)}
                            data-testid="agent-job-cancel"
                          >
                            Cancel
                          </button>
                        </div>
                        <p className="task-meta" data-testid="agent-job-verification">
                          {selectedAgentJob.job.verification_transcript ?? "verification pending"}
                        </p>
                        {selectedAgentJob.job.capability_request_json ? (
                          <pre data-testid="agent-job-capability-request">
                            {selectedAgentJob.job.capability_request_json}
                          </pre>
                        ) : null}
                        {selectedAgentJob.job.deliverable_json ? (
                          <pre data-testid="agent-job-deliverable">
                            {selectedAgentJob.job.deliverable_json}
                          </pre>
                        ) : null}
                        <ul className="subtask-step-list" data-testid="agent-job-steps">
                          {selectedAgentJob.steps.map((step) => (
                            <li key={step.id} className={`subtask-step subtask-step--${step.status}`}>
                              {step.step_index + 1}. {step.phase}: {step.status}
                            </li>
                          ))}
                        </ul>
                        <ul className="compact-list" data-testid="agent-job-checkpoints">
                          {selectedAgentJob.checkpoints.map((checkpoint) => (
                            <li key={checkpoint.id}>
                              checkpoint {checkpoint.step_index + 1}: {checkpoint.phase}
                            </li>
                          ))}
                        </ul>
                        <ul className="compact-list" data-testid="agent-job-steering">
                          {selectedAgentJob.steering.map((steering) => (
                            <li key={steering.id}>
                              {steering.status}: {steering.message}
                            </li>
                          ))}
                        </ul>
                        <ul className="compact-list" data-testid="agent-job-events">
                          {selectedAgentJob.events.slice(-6).map((event) => (
                            <li key={event.id}>{event.event_type}</li>
                          ))}
                        </ul>
                      </div>
                    ) : null}
                    {workloadSummary.stale_tasks.length > 0 ? (
                      <div data-testid="workload-stale-tasks">
                        <p className="task-meta">Not worked on recently</p>
                        <ul>
                          {workloadSummary.stale_tasks.map((task) => (
                            <li key={task.id} data-testid="workload-stale-task-item">
                              <span>{task.title}</span>
                              {task.days_since_focus !== null ? (
                                <span className="task-meta"> &mdash; last worked on {task.days_since_focus} days ago</span>
                              ) : null}
                            </li>
                          ))}
                        </ul>
                      </div>
                    ) : null}
                  </>
                ) : null}
              </section>
            ) : null}

            <section
              className="settings-panel recently-learned-panel"
              data-testid="recently-learned-panel"
            >
              <div className="row-actions">
                <button
                  type="button"
                  onClick={() => setRecentlyLearnedOpen((open) => !open)}
                  data-testid="recently-learned-toggle"
                >
                  {recentlyLearnedOpen ? "Hide" : "Show"} recently learned
                  {recentlyLearned.length > 0 ? ` (${recentlyLearned.length})` : ""}
                </button>
                <button
                  type="button"
                  onClick={() => void handleRefreshRecentlyLearned()}
                  data-testid="recently-learned-refresh"
                >
                  Refresh
                </button>
              </div>

              {recentlyLearnedOpen ? (
                <>
                  <div className="row-actions">
                    <p className="task-meta">
                      Watcher:{" "}
                      {watcherStatus?.is_watching
                        ? `watching ${watcherStatus.watched_path}`
                        : "not watching"}
                    </p>
                    {watcherStatus?.is_watching ? (
                      <button
                        type="button"
                        onClick={() => void handleStopWatcher()}
                        data-testid="stop-watcher-button"
                      >
                        Stop watcher
                      </button>
                    ) : (
                      <button
                        type="button"
                        onClick={() =>
                          workspaceInfo
                            ? void handleStartWatcher(workspaceInfo.workspace_path)
                            : undefined
                        }
                        data-testid="start-watcher-button"
                        disabled={!workspaceInfo}
                      >
                        Start watcher
                      </button>
                    )}
                  </div>

                  <div className="row-actions">
                    <label data-testid="clipboard-capture-label">
                      <input
                        type="checkbox"
                        checked={clipboardCaptureEnabled}
                        onChange={() => void handleToggleClipboardCapture()}
                        data-testid="clipboard-capture-toggle"
                      />
                      Capture clipboard (off by default)
                    </label>
                  </div>

                  {recentlyLearned.length === 0 ? (
                    <p data-testid="recently-learned-empty">Nothing learned yet.</p>
                  ) : (
                    <ul
                      className="compact-list"
                      data-testid="recently-learned-list"
                    >
                      {recentlyLearned.map((item) => (
                        <li key={item.id}>
                          <span className="task-meta">{item.source}</span>{" "}
                          <strong>{item.display_label}</strong>
                          {item.preview_text ? (
                            <span className="task-meta"> — {item.preview_text.slice(0, 80)}</span>
                          ) : null}
                          <span className="task-meta"> {item.ingested_at}</span>
                        </li>
                      ))}
                    </ul>
                  )}
                </>
              ) : null}
            </section>

            {fullWorkspaceVisible && showDebugPanels ? (
              <section className="settings-panel" data-testid="action-center-panel">
              <h3>Action Center</h3>
              <p data-testid="action-center-summary">
                Pending revisions {taskPendingRevisions.length} • Active subtasks {activeSubtasks.length} • Review subtasks{" "}
                {completedSubtasksAwaitingReview.length} • Suggestions {pendingSuggestions.length}
              </p>

              <div className="action-center-grid">
                <div>
                  <p className="task-meta">Pending revisions</p>
                  {taskPendingRevisions.length === 0 ? (
                    <p data-testid="action-center-revisions-empty">none</p>
                  ) : (
                    <ul data-testid="action-center-revisions-list" className="compact-list">
                      {taskPendingRevisions.slice(0, 5).map((revision) => (
                        <li key={`action-revision-${revision.revision_id}`}>
                          #{revision.revision_id} • artifact {revision.artifact_id}
                        </li>
                      ))}
                    </ul>
                  )}
                </div>

                <div>
                  <p className="task-meta">Active subtasks</p>
                  {activeSubtasks.length === 0 ? (
                    <p data-testid="action-center-active-subtasks-empty">none</p>
                  ) : (
                    <ul data-testid="action-center-active-subtasks-list" className="compact-list">
                      {activeSubtasks.slice(0, 5).map((subtask) => (
                        <li key={`action-active-subtask-${subtask.subtask_id}`}>
                          #{subtask.subtask_id} • {subtask.status}
                        </li>
                      ))}
                    </ul>
                  )}
                </div>

                <div>
                  <p className="task-meta">Results awaiting review</p>
                  {completedSubtasksAwaitingReview.length === 0 ? (
                    <p data-testid="action-center-review-subtasks-empty">none</p>
                  ) : (
                    <ul data-testid="action-center-review-subtasks-list" className="compact-list">
                      {completedSubtasksAwaitingReview.slice(0, 5).map((subtask) => (
                        <li key={`action-review-subtask-${subtask.subtask_id}`}>
                          #{subtask.subtask_id} • {subtask.title}
                        </li>
                      ))}
                    </ul>
                  )}
                </div>

                <div>
                  <p className="task-meta">Pending suggestions</p>
                  {pendingSuggestions.length === 0 ? (
                    <p data-testid="action-center-suggestions-empty">none</p>
                  ) : (
                    <ul data-testid="action-center-suggestions-list" className="compact-list">
                      {pendingSuggestions.slice(0, 5).map((suggestion) => (
                        <li key={`action-suggestion-${suggestion.suggestion_id}`}>
                          {suggestion.title}
                        </li>
                      ))}
                    </ul>
                  )}
                </div>
              </div>
              </section>
            ) : null}

            {!fullWorkspaceVisible ? (
              <section className="settings-panel" data-testid="companion-inline-actions">
                <h3>Jeff Actions</h3>

                {topPendingSuggestion ? (
                  <div className="companion-card" data-testid="companion-suggestion-card">
                    <p>
                      You might want to: <strong>{topPendingSuggestion.title}</strong>
                    </p>
                    <p>{topPendingSuggestion.description}</p>
                    <div className="row-actions">
                      <button
                        type="button"
                        onClick={() => void handleAcceptSuggestion(topPendingSuggestion)}
                        data-testid="companion-suggestion-accept"
                      >
                        Yes
                      </button>
                      <button
                        type="button"
                        onClick={() => void handleDismissSuggestion(topPendingSuggestion.suggestion_id)}
                        data-testid="companion-suggestion-dismiss"
                      >
                        Not now
                      </button>
                      <button
                        type="button"
                        onClick={() => void handleExplainSuggestion(topPendingSuggestion.suggestion_id)}
                      >
                        Tell me more
                      </button>
                    </div>
                  </div>
                ) : (
                  <p data-testid="companion-suggestion-empty">No pending suggestion right now.</p>
                )}

                {pendingRevisions.slice(0, 1).map((revision) => (
                  <div key={`companion-revision-${revision.revision_id}`} className="companion-card" data-testid="companion-revision-card">
                    <p>I tightened your section. Want to apply revision #{revision.revision_id}?</p>
                    <div className="row-actions">
                      <button type="button" onClick={() => void handleApplyRevision(revision.revision_id)} data-testid="companion-revision-apply">
                        Apply
                      </button>
                      <button
                        type="button"
                        onClick={() =>
                          setExpandedCompanionRevisionId((current) =>
                            current === revision.revision_id ? null : revision.revision_id
                          )
                        }
                        data-testid="companion-revision-diff"
                      >
                        See diff
                      </button>
                      <button type="button" onClick={() => void handleRejectRevision(revision.revision_id)} data-testid="companion-revision-ignore">
                        Ignore
                      </button>
                    </div>
                    {expandedCompanionRevisionId === revision.revision_id ? (
                      <pre data-testid="companion-revision-diff-content">
                        Original: {revision.original_text}

                        Proposed: {revision.proposed_text}
                      </pre>
                    ) : null}
                  </div>
                ))}

                {completedSubtasksAwaitingReview.slice(0, 1).map((subtask) => (
                  <div key={`companion-subtask-${subtask.subtask_id}`} className="companion-card" data-testid="companion-subtask-card">
                    <p>
                      I drafted "{subtask.title}". Want to review result #{subtask.subtask_id}?
                    </p>
                    <div className="row-actions">
                      <button
                        type="button"
                        onClick={() =>
                          setExpandedCompanionSubtaskId((current) =>
                            current === subtask.subtask_id ? null : subtask.subtask_id
                          )
                        }
                        data-testid="companion-subtask-view"
                      >
                        View result
                      </button>
                      <button
                        type="button"
                        onClick={() => void handleConvertSubtaskToRevision(subtask.subtask_id)}
                        disabled={!artifactContent?.is_editable}
                        data-testid="companion-subtask-convert"
                      >
                        Convert to edit
                      </button>
                      <button
                        type="button"
                        onClick={() => void handleRejectSubtaskResult(subtask.subtask_id)}
                        data-testid="companion-subtask-ignore"
                      >
                        Ignore
                      </button>
                    </div>
                    {expandedCompanionSubtaskId === subtask.subtask_id ? (
                      <pre data-testid="companion-subtask-result">{subtask.result_payload ?? subtask.result_summary ?? "No result yet."}</pre>
                    ) : null}
                  </div>
                ))}

                {speculativeSubtask ? (
                  <div className="companion-card" data-testid="speculative-subtask-card">
                    <p>I started a background subtask: <strong>{speculativeSubtask.title}</strong></p>
                    <p>{speculativeSubtask.description}</p>
                    <div className="row-actions">
                      <button
                        type="button"
                        onClick={() =>
                          setExpandedCompanionSubtaskId((current) =>
                            current === speculativeSubtask.subtask_id ? null : speculativeSubtask.subtask_id
                          )
                        }
                        data-testid="speculative-subtask-view"
                      >
                        View
                      </button>
                      <button
                        type="button"
                        onClick={() => void handleCancelSpeculativeSubtask()}
                        data-testid="speculative-subtask-cancel"
                      >
                        Cancel
                      </button>
                      <button
                        type="button"
                        onClick={() => void handleDismissSpeculativeSubtask()}
                        data-testid="speculative-subtask-dismiss"
                      >
                        Dismiss
                      </button>
                    </div>
                  </div>
                ) : null}

                {fileWriteProposals.map((proposal) => (
                  <div key={proposal.id} className="companion-card" data-testid="file-write-proposal-card">
                    <p>
                      A chain subtask wants to write a file:{" "}
                      <strong>{proposal.proposed_path}</strong>
                    </p>
                    <pre data-testid="file-write-proposal-preview">
                      {proposal.proposed_content.slice(0, 300)}
                      {proposal.proposed_content.length > 300 ? "\n..." : ""}
                    </pre>
                    <div className="row-actions">
                      <button
                        type="button"
                        onClick={() => void handleApproveFileWrite(proposal.id)}
                        data-testid="file-write-approve-button"
                      >
                        Approve &amp; write
                      </button>
                      <button
                        type="button"
                        onClick={() => void handleRejectFileWrite(proposal.id)}
                        data-testid="file-write-reject-button"
                      >
                        Reject
                      </button>
                    </div>
                  </div>
                ))}
              </section>
            ) : null}

            {fullWorkspaceVisible ? (
              <section className="settings-panel" data-testid="next-suggestions-panel">
              <div className="row-actions">
                <h3>Next Suggestions</h3>
                <button type="button" onClick={() => void handleRefreshSuggestions()} data-testid="suggestions-refresh-button">
                  Refresh
                </button>
              </div>
              {showDebugPanels ? (
                <>
                  <p data-testid="session-mode-label">
                    Mode: {formatModeLabel(sessionModeState?.current_mode ?? "quiet_observing")}
                  </p>
                  <p data-testid="session-mode-reason">
                    {sessionModeState?.mode_reason ?? "Mode reason unavailable."}
                  </p>
                </>
              ) : null}
              {suggestionActionMessage ? (
                <p data-testid="suggestion-action-message">{suggestionActionMessage}</p>
              ) : null}
              {suggestions.length === 0 ? (
                <p data-testid="suggestions-empty">No suggestions right now.</p>
              ) : (
                <ul data-testid="suggestions-list" className="revision-list">
                  {suggestions.map((suggestion) => (
                    <li key={suggestion.suggestion_id} className="revision-item">
                      <p>
                        <strong>{suggestion.title}</strong> ({suggestion.suggestion_type})
                      </p>
                      <p>{suggestion.description}</p>
                      <p className="task-meta">Reason: {suggestion.source_reason}</p>
                      <div className="row-actions">
                        <button
                          type="button"
                          onClick={() => void handleAcceptSuggestion(suggestion)}
                          disabled={isActionPending(`suggestion-accept-${suggestion.suggestion_id}`)}
                          data-testid={`suggestion-accept-${suggestion.suggestion_id}`}
                        >
                          Accept
                        </button>
                        <button
                          type="button"
                          onClick={() => void handleDismissSuggestion(suggestion.suggestion_id)}
                          disabled={isActionPending(`suggestion-dismiss-${suggestion.suggestion_id}`)}
                          data-testid={`suggestion-dismiss-${suggestion.suggestion_id}`}
                        >
                          Dismiss
                        </button>
                        <button
                          type="button"
                          onClick={() => void handleExplainSuggestion(suggestion.suggestion_id)}
                          data-testid={`suggestion-explain-${suggestion.suggestion_id}`}
                        >
                          Tell me more
                        </button>
                      </div>
                      {suggestionExplainById[suggestion.suggestion_id] ? (
                        <pre data-testid={`suggestion-explain-text-${suggestion.suggestion_id}`}>
                          {suggestionExplainById[suggestion.suggestion_id]}
                        </pre>
                      ) : null}
                    </li>
                  ))}
                </ul>
              )}
              </section>
            ) : null}

            <section className="settings-panel" data-testid="proactive-settings">
              <label className="toggle-row">
                <input
                  type="checkbox"
                  checked={coworkingStatus?.proactive_mode ?? false}
                  onChange={(event) => void handleProactiveToggle(event.target.checked)}
                  data-testid="proactive-toggle"
                />
                Proactive mode
              </label>
              <p>Pause threshold: {coworkingStatus?.pause_threshold_seconds ?? 0}s</p>
              <p>Cooldown remaining: {coworkingStatus?.cooldown_remaining_seconds ?? 0}s</p>
              {showDebugPanels ? (
                <p data-testid="coworking-decision-reason">Decision: {retrievalDebugMeta.decisionReason}</p>
              ) : null}
            </section>

            {fullWorkspaceVisible && showDebugPanels ? (
              <section className="settings-panel" data-testid="runtime-inspector-panel">
              <h3>Runtime Inspector</h3>
              <p>Active task: {activeTask.id} ({activeTask.title})</p>
              <p>Current mode: {formatModeLabel(sessionModeState?.current_mode ?? "quiet_observing")}</p>
              <p>Coworking state: {coworkingStatus?.state ?? "idle"}</p>
              <p>Proactive mode: {coworkingStatus?.proactive_mode ? "on" : "off"}</p>
              <p>Listening: {coworkingStatus?.user_speaking ? "yes" : "no"}</p>
              <p>Typing: {coworkingStatus?.user_typing ? "yes" : "no"}</p>
              <p>Speaking: {coworkingStatus?.state === "speaking" ? "yes" : "no"}</p>
              <p>Active artifact: {selectedArtifactId ?? sessionModeState?.active_artifact_id ?? "none"}</p>
              <p>Last nudge/decision reason: {retrievalDebugMeta.decisionReason}</p>
              <p>Pending revisions: {taskPendingRevisions.length}</p>
              <p>Pending suggestions: {pendingSuggestions.length}</p>
              <p>Active subtasks: {activeSubtasks.length}</p>
              <p>Last engine decision: {sessionModeState?.last_engine_decision ?? "n/a"}</p>

              <h4 className="compact-heading">Recent Events</h4>
              {recentEvents.length === 0 ? (
                <p data-testid="runtime-events-empty">No recent events.</p>
              ) : (
                <ul data-testid="runtime-events-list" className="compact-list">
                  {recentEvents.map((event) => (
                    <li key={`event-${event.id}`}>
                      <span className="event-type">{event.event_type}</span>
                      <span className="event-summary">{summarizeEventPayload(event)}</span>
                    </li>
                  ))}
                </ul>
              )}
              </section>
            ) : null}

            <div className="chat-controls">
              <button
                type="button"
                onClick={() => {
                  if (recording) {
                    void stopRecording();
                  } else {
                    void startRecording("chat");
                  }
                }}
                data-testid="record-toggle"
              >
                {recording ? "Stop Recording" : "Start Recording"}
              </button>
              <p data-testid="recording-indicator">{recording ? `Recording (${recordingPurpose})` : "Not recording"}</p>
            </div>

            <div className="chat-history" data-testid="chat-history">
              {messages.length === 0 && !streamingTurnId ? <p>No messages yet.</p> : null}
              {messages.map((message) => (
                <div key={message.id} className={`chat-bubble chat-${message.role} kind-${message.message_kind}`}>
                  <p className="chat-role">{message.role}</p>
                  <p className="chat-source">source: {message.message_source}</p>
                  {message.message_kind === "assistant_nudge" ? <p className="nudge-label">Nudge</p> : null}
                  {message.message_kind === "assistant_answer" ? <p className="answer-label">Answer</p> : null}
                  {message.message_kind === "assistant_revision_proposal" ? (
                    <p className="revision-label">Revision Proposal</p>
                  ) : null}
                  {message.message_kind === "assistant_revision_status" ? (
                    <p className="revision-status-label">Revision Status</p>
                  ) : null}
                  {message.message_kind === "assistant_interrupted" ? (
                    <p className="interrupted-label">interrupted</p>
                  ) : null}
                  <p>{message.content}</p>
                </div>
              ))}
              {streamingTurnId && (streamingText[streamingTurnId] ?? "").length > 0 ? (
                <div
                  className="chat-bubble chat-assistant kind-assistant_partial"
                  data-testid="streaming-message"
                >
                  <p className="chat-role">assistant</p>
                  <p className="streaming-indicator">streaming</p>
                  <p>{streamingText[streamingTurnId]}</p>
                </div>
              ) : streamingTurnId ? (
                <div className="chat-bubble chat-assistant kind-assistant_partial" data-testid="streaming-message">
                  <p className="chat-role">assistant</p>
                  <p className="streaming-indicator">thinking...</p>
                </div>
              ) : null}
            </div>

            <form onSubmit={handleSendTextMessage} className="task-form">
              <input
                aria-label="Chat input"
                placeholder="what are the requirements"
                value={chatInput}
                onChange={(event) => handleChatInputChange(event.target.value)}
                onBlur={() => clearTypingSignal()}
              />
              <button type="submit" disabled={chatInput.trim().length === 0}>
                Send
              </button>
            </form>

            {fullWorkspaceVisible && showDebugPanels ? (
              <>
                <h3>Retrieval Debug</h3>
                <p>
                  Event: {retrievalDebugMeta.decisionEventType} | Reason: {retrievalDebugMeta.decisionReason}
                  {retrievalDebugMeta.confidence !== null ? ` | Confidence: ${retrievalDebugMeta.confidence.toFixed(3)}` : ""}
                </p>
                {retrievalDebugChunks.length === 0 ? (
                  <p data-testid="retrieval-debug-empty">No retrieval debug data yet.</p>
                ) : (
                  <ul data-testid="retrieval-debug-list" className="retrieval-list">
                    {retrievalDebugChunks.map((chunk) => (
                      <li key={chunk.chunk_id}>
                        <p className="chunk-source">
                          {chunk.artifact_file_name} • score {chunk.similarity_score.toFixed(3)} • chunk #{chunk.position_index}
                        </p>
                        <p>{chunk.chunk_text.slice(0, 320)}</p>
                      </li>
                    ))}
                  </ul>
                )}

                <h3>Revision Debug</h3>
                <div className="revision-debug" data-testid="revision-debug-panel">
              <p>Active artifact: {revisionDebug.activeArtifactId ?? "none"}</p>
              <p>
                Selection used: {revisionDebug.selectedStart ?? "-"}..{revisionDebug.selectedEnd ?? "-"} ({revisionDebug.selectionSource})
              </p>
              <p>Instruction source: {revisionDebug.instructionSource}</p>
              <p>Context source: {revisionDebug.contextSource}</p>
              <p>
                Confidence:{" "}
                {revisionDebug.confidence !== null ? revisionDebug.confidence.toFixed(3) : "n/a"}
              </p>
              <p>Grounding notes: {revisionDebug.groundingNotes}</p>
              {revisionDebug.retrievedChunks.length > 0 ? (
                <ul data-testid="revision-debug-chunks" className="retrieval-list">
                  {revisionDebug.retrievedChunks.map((chunk) => (
                    <li key={`revision-debug-${chunk.chunk_id}`}>
                      <p className="chunk-source">
                        {chunk.artifact_file_name} • score {chunk.similarity_score.toFixed(3)} • chunk #{chunk.position_index}
                      </p>
                      <p>{chunk.chunk_text.slice(0, 220)}</p>
                    </li>
                  ))}
                </ul>
              ) : (
                <p>No revision grounding chunks yet.</p>
              )}
                </div>

                <h3>SubTask Debug</h3>
                <div className="revision-debug" data-testid="subtask-debug-panel">
              <p>Selected subtask: {subtaskDebug.selectedSubtaskId ?? "none"}</p>
              <p>Execution type: {subtaskDebug.executionType}</p>
              <p>Instruction source: {subtaskDebug.instructionSource}</p>
              <p>
                Snapshot instruction:{" "}
                {parsedSubtaskSnapshot?.instruction ? parsedSubtaskSnapshot.instruction : "n/a"}
              </p>
              <p>
                Snapshot summary:{" "}
                {parsedSubtaskSnapshot?.task_summary ? parsedSubtaskSnapshot.task_summary : "n/a"}
              </p>
              {(subtaskDebug.retrievedChunks.length > 0
                ? subtaskDebug.retrievedChunks
                : parsedSubtaskSnapshot?.retrieved_chunks ?? []
              ).length > 0 ? (
                <ul data-testid="subtask-debug-chunks" className="retrieval-list">
                  {(subtaskDebug.retrievedChunks.length > 0
                    ? subtaskDebug.retrievedChunks
                    : parsedSubtaskSnapshot?.retrieved_chunks ?? []
                  ).map((chunk) => (
                    <li key={`subtask-debug-${chunk.chunk_id}`}>
                      <p className="chunk-source">
                        {chunk.artifact_file_name} • score {chunk.similarity_score.toFixed(3)} • chunk #
                        {chunk.position_index}
                      </p>
                      <p>{chunk.chunk_text.slice(0, 220)}</p>
                    </li>
                  ))}
                </ul>
              ) : (
                <p>No subtask grounding chunks yet.</p>
              )}
                </div>

                <h3>Flow Debug</h3>
                <div className="revision-debug" data-testid="flow-debug-panel">
              <p data-testid="flow-mode">Current mode: {formatModeLabel(sessionModeState?.current_mode ?? "quiet_observing")}</p>
              <p>Mode reason: {flowDebug.modeReason}</p>
              <p>Engine decision: {flowDebug.decisionReason}</p>
              <p>Suppression: {flowDebug.suppressionState}</p>
              <p>No-op decision: {flowDebug.noOp ? "yes" : "no"}</p>
              <p>
                Evidence score: {flowDebug.evidenceScore !== null ? flowDebug.evidenceScore.toFixed(3) : "n/a"}
              </p>
              <p>Active artifact: {sessionModeState?.active_artifact_id ?? selectedArtifactId ?? "none"}</p>
              <p>Waiting on user decision: {sessionModeState?.waiting_on_user_decision ? "yes" : "no"}</p>
              <p>Top suggestion: {pendingSuggestions[0]?.title ?? "none"}</p>
              {flowDebug.retrievedChunks.length > 0 ? (
                <ul data-testid="flow-debug-chunks" className="retrieval-list">
                  {flowDebug.retrievedChunks.map((chunk) => (
                    <li key={`flow-debug-${chunk.chunk_id}`}>
                      <p className="chunk-source">
                        {chunk.artifact_file_name} • score {chunk.similarity_score.toFixed(3)} • chunk #{chunk.position_index}
                      </p>
                      <p>{chunk.chunk_text.slice(0, 220)}</p>
                    </li>
                  ))}
                </ul>
              ) : (
                <p>No flow retrieval support captured yet.</p>
              )}
                </div>
              </>
            ) : null}

            {fullWorkspaceVisible ? (
              <button
                type="button"
                className="debug-panels-toggle"
                onClick={handleToggleDebugPanels}
                data-testid="debug-panels-toggle"
              >
                {showDebugPanels ? "hide debug panels" : "show debug panels"}
              </button>
            ) : null}
          </aside>
        </section>
      ) : null}

      {errorMessage ? (
        <div role="alert" className="error" data-testid="jeff-error-banner">
          <p>{errorMessage}</p>
          {isApiKeyErrorMessage(errorMessage) ? (
            <button
              type="button"
              onClick={() => void handleOpenOnboarding(2)}
              data-testid="jeff-error-fix-api-key"
            >
              Update API key
            </button>
          ) : null}
        </div>
      ) : null}
    </main>
  );
}

async function classifyMessageIntentWithFallback(
  taskId: number,
  message: string
): Promise<{ intent: RoutedIntent; slots: IntentSlotsDto | null }> {
  const INTENT_CLASSIFIER_TIMEOUT_MS = 300;
  try {
    const result = await Promise.race([
      classifyMessageIntent(taskId, message),
      new Promise<never>((_, reject) =>
        setTimeout(
          () => reject(new Error(`intent_classifier_timeout_${INTENT_CLASSIFIER_TIMEOUT_MS}ms`)),
          INTENT_CLASSIFIER_TIMEOUT_MS
        )
      ),
    ]);
    return { intent: result.intent as RoutedIntent, slots: result.slots };
  } catch (err) {
    const reason = err instanceof Error ? err.message : String(err);
    console.warn(`[jeff] intent_classifier_fallback: ${reason}`);
    return { intent: inferMessageIntentKeyword(message), slots: null };
  }
}

function inferMessageIntentKeyword(message: string): RoutedIntent {
  const lower = message.toLowerCase();
  const trimmed = lower.trim();

  if (
    trimmed === "ok" ||
    trimmed === "okay" ||
    trimmed === "hmm" ||
    trimmed === "yes please" ||
    trimmed === "..." ||
    trimmed === "no that's not right"
  ) {
    return "unknown";
  }

  if (
    containsIntentPhrase(lower, [
      "fix ",
      "revise",
      "tighten",
      "rewrite",
      "improve this",
      "edit this",
      "fix this intro",
      "make this more analytical"
    ])
  ) {
    return "revision";
  }

  if (
    containsIntentPhrase(lower, [
      "draft ",
      "write ",
      "expand this",
      "synthesize",
      "build me a paragraph",
      "draft better intro"
    ])
  ) {
    return "subtask";
  }

  if (
    containsIntentPhrase(lower, [
      "what should i do next",
      "what next",
      "next step",
      "i'm stuck",
      "suggest next",
      "where should i focus"
    ])
  ) {
    return "suggestion";
  }

  return "answer";
}

function inferSubtaskExecutionTypeFromDraftType(draftType: string): ExecutionType {
  const lower = draftType.toLowerCase();
  if (lower.includes("expand") || lower.includes("expansion")) {
    return "expansion";
  }
  if (lower.includes("synth") || lower.includes("summary")) {
    return "synthesis";
  }
  if (lower.includes("research")) {
    return "targeted_research_synthesis";
  }
  return "draft_generation";
}

function inferSubtaskExecutionType(message: string): ExecutionType {
  const lower = message.toLowerCase();
  if (lower.includes("expand")) {
    return "expansion";
  }
  if (lower.includes("synth")) {
    return "synthesis";
  }
  if (lower.includes("research")) {
    return "targeted_research_synthesis";
  }
  return "draft_generation";
}

function inferStandingScheduleSpec(message: string): string {
  const lower = message.toLowerCase();
  const explicitDaily = lower.match(/\bdaily\s+([01]\d|2[0-3]):([0-5]\d)\b/);
  if (explicitDaily) {
    return `daily ${explicitDaily[1]}:${explicitDaily[2]}`;
  }
  const onEvent = lower.match(/\bon-event\s+([a-z0-9_-]+)\b/);
  if (onEvent) {
    return `on-event ${onEvent[1]}`;
  }
  if (lower.includes("every morning")) {
    return "daily 08:00";
  }
  if (lower.includes("every evening") || lower.includes("tonight") || lower.includes("citations")) {
    return "daily 18:00";
  }
  return "daily 18:00";
}

function normalizeRevisionInstruction(message: string): string {
  const trimmed = message.trim();
  if (trimmed.length === 0) {
    return "tighten this section and connect it to evidence requirements";
  }
  if (trimmed.length < 18) {
    return `${trimmed}. Tighten argument and connect to rubric evidence requirements.`;
  }
  return trimmed;
}

function buildRevisionInstruction(message: string, slots?: IntentSlotsDto | null): string {
  const instruction = slots?.instruction?.trim() || message.trim();
  const targetDescription = slots?.target_description?.trim();
  if (!targetDescription) {
    return instruction;
  }

  const lowerInstruction = instruction.toLowerCase();
  if (lowerInstruction.includes(targetDescription.toLowerCase())) {
    return instruction;
  }
  return `${targetDescription}: ${instruction}`;
}

function deriveRevisionTargetFromDescription(
  content: string | null,
  targetDescription: string | null,
  fallback: RevisionTargetDto | null
): RevisionTargetDto | null {
  if (!content || !targetDescription) {
    return fallback;
  }

  const cleanTarget = targetDescription.trim();
  if (!cleanTarget) {
    return fallback;
  }

  const paragraphIndex = parseParagraphOrdinal(cleanTarget);
  if (paragraphIndex !== null) {
    const paragraphRange = rangeForParagraph(content, paragraphIndex);
    if (paragraphRange) {
      return paragraphRange;
    }
  }

  const lowerContent = content.toLowerCase();
  const lowerTarget = cleanTarget.toLowerCase();
  const foundAt = lowerContent.indexOf(lowerTarget);
  if (foundAt < 0) {
    return fallback;
  }

  return expandRangeToParagraph(content, foundAt, foundAt + cleanTarget.length);
}

function parseParagraphOrdinal(targetDescription: string): number | null {
  const normalized = targetDescription.toLowerCase();
  const numeric = normalized.match(/(\d+)(?:st|nd|rd|th)?\s+paragraph/);
  if (numeric) {
    const parsed = Number.parseInt(numeric[1], 10);
    if (Number.isFinite(parsed) && parsed > 0) {
      return parsed - 1;
    }
  }

  const words = [
    "first",
    "second",
    "third",
    "fourth",
    "fifth",
    "sixth",
    "seventh",
    "eighth",
    "ninth",
    "tenth"
  ];
  for (let index = 0; index < words.length; index += 1) {
    if (normalized.includes(`${words[index]} paragraph`)) {
      return index;
    }
  }
  return null;
}

function rangeForParagraph(content: string, paragraphIndex: number): RevisionTargetDto | null {
  if (paragraphIndex < 0) {
    return null;
  }

  const paragraphRegex = /\S[\s\S]*?(?=\n\s*\n|$)/g;
  const ranges: Array<{ start: number; end: number }> = [];
  let match = paragraphRegex.exec(content);
  while (match) {
    ranges.push({ start: match.index, end: match.index + match[0].length });
    match = paragraphRegex.exec(content);
  }

  const range = ranges[paragraphIndex];
  if (!range || range.end <= range.start) {
    return null;
  }

  return {
    start_offset: range.start,
    end_offset: range.end
  };
}

function expandRangeToParagraph(content: string, start: number, end: number): RevisionTargetDto {
  const normalizedStart = Math.max(0, Math.min(start, content.length));
  const normalizedEnd = Math.max(normalizedStart, Math.min(end, content.length));

  const before = content.lastIndexOf("\n\n", normalizedStart);
  const after = content.indexOf("\n\n", normalizedEnd);
  const paragraphStart = before === -1 ? 0 : before + 2;
  const paragraphEnd = after === -1 ? content.length : after;

  if (paragraphEnd <= paragraphStart) {
    return {
      start_offset: normalizedStart,
      end_offset: Math.max(normalizedStart + 1, normalizedEnd)
    };
  }

  return {
    start_offset: paragraphStart,
    end_offset: paragraphEnd
  };
}

function normalizeFsPath(path: string | null | undefined): string {
  return (path ?? "")
    .trim()
    .replace(/\\/g, "/")
    .replace(/\/+$/, "");
}

function containsIntentPhrase(input: string, phrases: string[]): boolean {
  return phrases.some((phrase) => input.includes(phrase));
}

function isEditableArtifact(artifact: ArtifactDto): boolean {
  const extension = artifact.file_extension.toLowerCase();
  return extension === "md" || extension === "txt";
}

function deriveSubtaskTitle(instruction: string): string {
  const cleaned = instruction.trim().replace(/\s+/g, " ");
  if (!cleaned) {
    return "Subtask";
  }

  if (cleaned.length <= 64) {
    return cleaned;
  }

  return `${cleaned.slice(0, 61)}...`;
}

function normalizeExecutionType(value: string): ExecutionType {
  switch (value) {
    case "draft_generation":
    case "expansion":
    case "synthesis":
    case "targeted_research_synthesis":
      return value;
    default:
      return "draft_generation";
  }
}

function parseSubtaskSnapshot(snapshot: string): {
  task_summary: string;
  instruction: string;
  execution_type: string;
  recent_messages: string[];
  retrieved_chunks: RetrievedChunkDto[];
} | null {
  try {
    const parsed = JSON.parse(snapshot) as {
      task_summary?: string;
      instruction?: string;
      execution_type?: string;
      recent_messages?: string[];
      retrieved_chunks?: RetrievedChunkDto[];
    };

    return {
      task_summary: parsed.task_summary ?? "",
      instruction: parsed.instruction ?? "",
      execution_type: parsed.execution_type ?? "",
      recent_messages: Array.isArray(parsed.recent_messages) ? parsed.recent_messages : [],
      retrieved_chunks: Array.isArray(parsed.retrieved_chunks) ? parsed.retrieved_chunks : []
    };
  } catch {
    return null;
  }
}

function pickNextEditableArtifactId(artifacts: ArtifactDto[], currentArtifactId: number | null): number | null {
  if (currentArtifactId !== null && artifacts.some((artifact) => artifact.id === currentArtifactId && isEditableArtifact(artifact))) {
    return currentArtifactId;
  }

  const firstEditable = artifacts.find((artifact) => isEditableArtifact(artifact));
  return firstEditable ? firstEditable.id : null;
}

async function blobToBase64(blob: Blob): Promise<string> {
  const buffer = await blob.arrayBuffer();
  const bytes = new Uint8Array(buffer);
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary);
}

function base64ToBlob(base64: string, mimeType: string): Blob {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);

  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }

  return new Blob([bytes], { type: mimeType });
}

function formatStatusLabel(state: string): string {
  if (state === "silent_observing" || state === "awaiting_user") {
    return "OBSERVING";
  }

  return state.replace(/_/g, " ").toUpperCase();
}

function formatModeLabel(mode: string): string {
  return mode.replace(/_/g, " ").toUpperCase();
}

function formatSuggestionActionMessage(result: SuggestionAcceptanceDto): string {
  if (result.action_type === "revision_proposal_created" || result.action_type === "routed_to_revision_proposal") {
    return `Accepted suggestion: ${result.suggestion.title}. Revision proposal created for review.`;
  }

  if (result.action_type === "subtask_started") {
    return `Accepted suggestion: ${result.suggestion.title}. Bounded subtask started in parallel.`;
  }

  if (result.action_type === "followup_asked") {
    return `Accepted suggestion: ${result.suggestion.title}. Jeff asked a focused follow-up.`;
  }

  if (result.action_type === "routed_to_focused_answer") {
    return `Accepted suggestion: ${result.suggestion.title}. Returned a focused grounded answer.`;
  }

  return `Accepted suggestion: ${result.suggestion.title}.`;
}

function summarizeEventPayload(event: EventLogEntryDto): string {
  const raw = event.payload_json?.trim();
  if (!raw) {
    return event.created_at;
  }

  try {
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    const preferredKeys = [
      "decision_reason",
      "message_kind",
      "suggestion_id",
      "subtask_id",
      "revision_id",
      "status",
      "reason",
      "artifact_id"
    ];

    for (const key of preferredKeys) {
      const value = parsed[key];
      if (typeof value === "string" && value.trim().length > 0) {
        return `${key}=${value}`;
      }
      if (typeof value === "number") {
        return `${key}=${value}`;
      }
    }
  } catch {
    // keep raw fallback below
  }

  return raw.length > 72 ? `${raw.slice(0, 69)}...` : raw;
}

function buildCompanionGreeting(
  task: TaskDto,
  modeState: SessionModeStateDto | null,
  artifacts: ArtifactDto[]
): string {
  const selectedArtifact = modeState?.active_artifact_id
    ? artifacts.find((artifact) => artifact.id === modeState.active_artifact_id)
    : undefined;
  const artifactHint = selectedArtifact ? ` on ${selectedArtifact.file_name}` : "";
  const modeHint = modeState ? formatModeLabel(modeState.current_mode).toLowerCase() : "writing";
  return `You were ${modeHint}${artifactHint}. Want to continue or shift focus?`;
}

function deriveTaskTitleFromPrompt(prompt: string): string {
  const cleaned = prompt
    .replace(/\s+/g, " ")
    .replace(/[^a-zA-Z0-9\\-_' ]/g, "")
    .trim();
  if (!cleaned) {
    return "New task";
  }
  return cleaned.slice(0, 64);
}

function isApiKeyErrorMessage(message: string | null): boolean {
  if (!message) {
    return false;
  }
  const lower = message.toLowerCase();
  return (
    lower.includes("api key") ||
    lower.includes("openai_api_key is not configured") ||
    lower.includes("status 401") ||
    lower.includes("unauthorized") ||
    lower.includes("invalid_api_key")
  );
}

function formatError(error: unknown): string {
  if (typeof error === "string") {
    return mapJeffErrorMessage(error);
  }

  if (error instanceof Error) {
    return mapJeffErrorMessage(error.message);
  }

  return "Unexpected error";
}

function extractStreamCancelError(reason: string): string | null {
  if (!reason || reason === "user_barge_in" || reason === "jeff_barge_in" || reason === "explicit") {
    return null;
  }
  const colonIdx = reason.indexOf(": ");
  const msg = colonIdx >= 0 ? reason.slice(colonIdx + 2).trim() : reason.trim();
  return msg || null;
}

function mapJeffErrorMessage(raw: string): string {
  const API_TIMEOUT_MESSAGE = "Jeff couldn't reach OpenAI — check your network connection.";
  const API_KEY_MESSAGE = "Your API key isn't working. Open settings to update it.";
  const DB_LOCK_MESSAGE = "Jeff ran into a save conflict. Try again in a moment.";

  const lower = raw.toLowerCase();

  if (lower.includes("jeff couldn't reach openai")) {
    return API_TIMEOUT_MESSAGE;
  }

  if (
    lower.includes("openai_api_key is not configured") ||
    lower.includes("status 401") ||
    lower.includes("unauthorized") ||
    lower.includes("invalid_api_key") ||
    lower.includes("your api key isn't working")
  ) {
    return API_KEY_MESSAGE;
  }

  if (
    lower.includes("database is locked") ||
    lower.includes("sqlite_busy") ||
    lower.includes("save conflict")
  ) {
    return DB_LOCK_MESSAGE;
  }

  if (lower.includes("jeff needs ") && lower.includes("system settings")) {
    return raw;
  }

  if (
    lower.includes("permission") ||
    lower.includes("axisprocesstrusted") ||
    lower.includes("axuielement") ||
    lower.includes("not allowed") ||
    lower.includes("not authorized") ||
    lower.includes("denied")
  ) {
    const permission = inferPermissionLabel(lower);
    return `Jeff needs ${permission} to do this — open System Settings.`;
  }

  return raw;
}

function inferPermissionLabel(lowerMessage: string): string {
  if (
    lowerMessage.includes("accessibility") ||
    lowerMessage.includes("axisprocesstrusted") ||
    lowerMessage.includes("axuielement")
  ) {
    return "Accessibility permission";
  }

  if (lowerMessage.includes("notification")) {
    return "notification permission";
  }

  if (lowerMessage.includes("microphone") || lowerMessage.includes("record")) {
    return "microphone permission";
  }

  if (lowerMessage.includes("calendar")) {
    return "calendar permission";
  }

  return "required permission";
}

export default App;
