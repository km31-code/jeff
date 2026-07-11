import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import {
  EVENT_LLM_COMPLETE,
  EVENT_LLM_TOKEN,
  EVENT_TTS_CHUNK,
  EVENT_TURN_CANCELLED,
  EVENT_TURN_COMPLETE,
  LlmCompletePayload,
  LlmTokenPayload,
  TtsChunkPayload,
  TurnCancelledPayload,
  TurnCompletePayload,
  isStreamingEnabled
} from "./streamClient";

import {
  AmbientStateDto,
  OverlayMode,
  TrayStatus,
  getAmbientState,
  hideOverlay,
  markNotificationPermission,
  setOverlayMode,
  setTrayStatus,
  setQuietMode,
} from "./ambientClient";
import {
  ActiveWindowContextDto,
  ApiKeyValidationDto,
  ChatMessageDto,
  CrisisCardDto,
  FileWriteProposalDto,
  IntentSlotsDto,
  OnboardingStatusDto,
  RevisionProposalDto,
  SelectionCaptureIndicatorDto,
  TaskDto,
  WatcherStatusDto,
  approveSubtaskFileWrite,
  acceptSubtaskResult,
  applyRevision,
  cancelSubtask,
  cancelStreamingTurn,
  classifyMessageIntent,
  clearPreferredWorkspaceFolder,
  completeOnboarding,
  createTask,
  dismissSelectionCapture,
  generateRevisionAlternative,
  getActiveTask,
  getActiveWindowContext,
  getAccessibilityPermissionStatus,
  getOnboardingStatus,
  getSelectionCaptureIndicator,
  getWatcherStatus,
  listFileWriteProposals,
  listRevisionAlternatives,
  listSubtasks,
  listTasks,
  listMessages,
  listTaskPendingRevisions,
  recordTaskFocus,
  recordCrisisFeedback,
  rejectRevision,
  rejectSubtaskResult,
  rejectSubtaskFileWrite,
  requestAccessibilityPermission,
  sendMessage,
  sendMessageStreaming,
  setActiveTask,
  setPreferredWorkspaceFolder,
  startSubtaskChain,
  storeAnthropicApiKey,
  storeOpenAiApiKey,
  transcribeAudio,
  validateOpenAiApiKey,
  startVoiceSession,
  persistVoiceTranscript,
  handleVoiceToolCall,
  setInferenceMode
} from "./tauriClient";
import { connectRealtimeVoice, type RealtimeConnection } from "./voiceRealtime";

function extractStreamErrorMessage(reason: string): string | null {
  if (!reason || reason === "user_barge_in" || reason === "jeff_barge_in" || reason === "explicit") {
    return null;
  }
  const colonIdx = reason.indexOf(": ");
  const msg = colonIdx >= 0 ? reason.slice(colonIdx + 2).trim() : reason.trim();
  return msg || null;
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

// phase 11 overlay: ambient presence window. collapsed is a compact status
// bar, expanded shows the last few messages and a send box. this is not the
// full workspace — it is the always-there companion surface.

type OnboardingStep = 1 | 2 | 3 | 4 | 5;
type OverlayShownPayload = { interactive?: boolean };
type ActiveSubtaskState = { id: number; taskId: number; title: string };
type CompanionStartedPayload = { subtask_id: number; task_id: number; title: string };
type CompanionCompletePayload = { subtask_id: number; task_id: number; final_status: string };
type SpeculativeSubtaskState = { subtask_id: number; title: string; description?: string | null; result_summary?: string | null };
type WriteConfirmation = { id: number; fileName: string };

const WRITE_CONFIRMATION_MS = 3500;

function normalizePath(path: string): string {
  return path.replace(/\\/g, "/");
}

function fileNameFromPath(path: string): string {
  const normalized = normalizePath(path);
  return normalized.split("/").filter(Boolean).pop() ?? normalized;
}

function displayProposalPath(path: string): string {
  const normalized = normalizePath(path);
  const parts = normalized.split("/").filter(Boolean);
  if (parts.length <= 2) {
    return normalized;
  }
  return parts.slice(-2).join("/");
}

function proposalExcerpt(content: string): string {
  const compact = content.replace(/\s+/g, " ").trim();
  if (!compact) {
    return "(empty file)";
  }
  return compact.length > 80 ? `${compact.slice(0, 77)}...` : compact;
}

interface SpeechRecognitionEvent extends Event {
  resultIndex: number;
  results: SpeechRecognitionResultList;
}
interface OverlaySpeechRecognition {
  interimResults: boolean;
  continuous: boolean;
  lang: string;
  onresult: ((event: SpeechRecognitionEvent) => void) | null;
  onerror: (() => void) | null;
  onend: (() => void) | null;
  start(): void;
  stop(): void;
}

function describeStatus(status: TrayStatus): string {
  switch (status) {
    case "listening":
      return "listening";
    case "working":
      return "working";
    case "idle":
    default:
      return "idle";
  }
}

function formatHotkey(hotkey: string): string {
  // best-effort prettifier for display only.
  return hotkey
    .replace(/CmdOrCtrl/gi, navigatorIsApple() ? "\u2318" : "Ctrl")
    .replace(/Cmd/gi, "\u2318")
    .replace(/Shift/gi, "\u21E7")
    .replace(/Alt/gi, "\u2325")
    .replace(/Option/gi, "\u2325")
    .replace(/\+/g, " ");
}

function navigatorIsApple(): boolean {
  if (typeof navigator === "undefined") return false;
  return /Mac|iPhone|iPad/i.test(navigator.platform || "");
}

function deriveTaskTitleFromPrompt(prompt: string): string {
  const cleaned = prompt
    .replace(/\s+/g, " ")
    .replace(/[^a-zA-Z0-9\-_' ]/g, "")
    .trim();
  if (!cleaned) {
    return "New task";
  }
  return cleaned.slice(0, 64);
}

function isApiKeyIssue(message: string | null): boolean {
  if (!message) {
    return false;
  }
  const lower = message.toLowerCase();
  return (
    lower.includes("openai_api_key is not configured") ||
    lower.includes("api key") ||
    lower.includes("status 401") ||
    lower.includes("invalid_api_key") ||
    lower.includes("unauthorized")
  );
}

// intent routing helpers — mirror the logic in App.tsx so the overlay
// behaves identically to the full workspace for parallel-work and revision
// requests. all functions are pure and live outside the component.

type OverlayRoutedIntent = "answer" | "revision" | "subtask" | "suggestion" | "unknown";

function containsIntentPhrase(lower: string, phrases: string[]): boolean {
  return phrases.some((phrase) => lower.includes(phrase));
}

function inferOverlayMessageIntentKeyword(message: string): OverlayRoutedIntent {
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
      "make this more analytical",
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
      "draft better intro",
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
      "where should i focus",
    ])
  ) {
    return "suggestion";
  }

  return "answer";
}

function inferOverlaySubtaskExecutionType(message: string): string {
  const lower = message.toLowerCase();
  if (lower.includes("expand")) return "expansion";
  if (lower.includes("synth")) return "synthesis";
  if (lower.includes("research")) return "targeted_research_synthesis";
  return "draft_generation";
}

function inferOverlaySubtaskExecutionTypeFromDraftType(draftType: string): string {
  const lower = draftType.toLowerCase();
  if (lower.includes("expand") || lower.includes("expansion")) return "expansion";
  if (lower.includes("synth") || lower.includes("summary")) return "synthesis";
  if (lower.includes("research")) return "targeted_research_synthesis";
  return "draft_generation";
}

function deriveOverlaySubtaskTitle(description: string): string {
  return description.replace(/\s+/g, " ").trim().slice(0, 64) || "background task";
}

function playWakeWordAckCue(): void {
  const win = window as unknown as { webkitAudioContext?: typeof AudioContext };
  const AudioContextCtor = window.AudioContext ?? win.webkitAudioContext;
  if (!AudioContextCtor) return;

  try {
    const context = new AudioContextCtor();
    const oscillator = context.createOscillator();
    const gain = context.createGain();
    oscillator.type = "sine";
    oscillator.frequency.value = 880;
    gain.gain.value = 0.04;
    oscillator.connect(gain);
    gain.connect(context.destination);
    oscillator.start();
    oscillator.stop(context.currentTime + 0.08);
    oscillator.onended = () => {
      void context.close().catch(() => undefined);
    };
  } catch {
    // best-effort only; the wake flow still opens voice without the cue.
  }
}

async function classifyOverlayMessageIntentWithFallback(
  taskId: number,
  message: string
): Promise<{ intent: OverlayRoutedIntent; slots: IntentSlotsDto | null }> {
  const TIMEOUT_MS = 300;
  try {
    const result = await Promise.race([
      classifyMessageIntent(taskId, message),
      new Promise<never>((_, reject) =>
        setTimeout(
          () => reject(new Error(`intent_classifier_timeout_${TIMEOUT_MS}ms`)),
          TIMEOUT_MS
        )
      ),
    ]);
    return { intent: result.intent as OverlayRoutedIntent, slots: result.slots };
  } catch (err) {
    const reason = err instanceof Error ? err.message : String(err);
    console.warn(`[jeff] overlay_intent_fallback: ${reason}`);
    return { intent: inferOverlayMessageIntentKeyword(message), slots: null };
  }
}

interface OverlayProps {
  onOpenWorkspace: () => void;
}

export default function Overlay({ onOpenWorkspace }: OverlayProps): JSX.Element {
  // Root still wires this prop for the tray-driven workspace path, but the
  // companion no longer exposes workspace navigation as a primary action.
  void onOpenWorkspace;
  const [ambient, setAmbient] = useState<AmbientStateDto | null>(null);
  const [mode, setMode] = useState<OverlayMode>("collapsed");
  const [activeTask, setActiveTaskState] = useState<TaskDto | null>(null);
  const [messages, setMessages] = useState<ChatMessageDto[]>([]);
  const [input, setInput] = useState("");
  const [sending, setSending] = useState(false);
  const [recording, setRecording] = useState(false);
  // apex c4: realtime voice session state (idle/connecting/live/fallback/closed).
  const [voiceState, setVoiceState] = useState<"idle" | "connecting" | "live" | "fallback" | "closed">("idle");
  const [voiceMuted, setVoiceMuted] = useState(false);
  const voiceConnRef = useRef<RealtimeConnection | null>(null);
  const voiceStartPendingRef = useRef(false);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [infoNotice, setInfoNotice] = useState<string | null>(null);
  const [crisisCard, setCrisisCard] = useState<CrisisCardDto | null>(null);

  const mediaRecorderRef = useRef<MediaRecorder | null>(null);
  const mediaStreamRef = useRef<MediaStream | null>(null);
  const audioChunksRef = useRef<Blob[]>([]);

  // streaming tts queue: phrase-ordered audio so chunks play in arrival order.
  const ttsActiveTurnIdRef = useRef<string | null>(null);
  const streamTtsQueueRef = useRef<Map<number, { audio: HTMLAudioElement; url: string }>>(new Map());
  const streamTtsNextPhraseRef = useRef<number>(1);
  const streamTtsCurrentRef = useRef<HTMLAudioElement | null>(null);

  // partial stt via web speech api.
  const speechRecognitionRef = useRef<OverlaySpeechRecognition | null>(null);
  const partialSttSentRef = useRef<boolean>(false);
  const [hotkeyConflict, setHotkeyConflict] = useState<string | null>(null);
  const [streamingTurnId, setStreamingTurnId] = useState<string | null>(null);
  const [streamingText, setStreamingText] = useState("");

  // phase 20: active window context driven by backend context://context-updated events.
  const [activeContext, setActiveContext] = useState<ActiveWindowContextDto | null>(null);
  const [docSwitchBanner, setDocSwitchBanner] = useState<{ app_name: string; document_title: string } | null>(null);
  const [tasks, setTasks] = useState<TaskDto[]>([]);
  const [taskSwitcherOpen, setTaskSwitcherOpen] = useState(false);
  const [accessibilityPermissionGranted, setAccessibilityPermissionGranted] = useState<boolean | null>(null);
  const [accessibilityPromptDismissed, setAccessibilityPromptDismissed] = useState(false);
  const docSwitchTimerRef = useRef<number | null>(null);

  // phase 22: selected-text capture indicator. shown between messages and the
  // input box so the user sees what context is loaded before sending a message.
  const [selectionCaptureIndicator, setSelectionCaptureIndicator] =
    useState<SelectionCaptureIndicatorDto | null>(null);

  // phase 13: watcher status shown in the task row.
  const [watcherStatus, setWatcherStatus] = useState<WatcherStatusDto | null>(null);
  // brief "just indexed" confirmation shown when the watcher ingests a new file.
  const [fileIndexedNotice, setFileIndexedNotice] = useState<string | null>(null);
  const fileIndexedTimerRef = useRef<number | null>(null);
  // g3: one-time per-session soft prompt to connect a folder after the first
  // successful message exchange when no folder is connected.
  const [showFolderConnectPrompt, setShowFolderConnectPrompt] = useState(false);
  const folderPromptShownRef = useRef(false);
  const [activeSubtask, setActiveSubtask] = useState<ActiveSubtaskState | null>(null);
  const [pendingWriteProposals, setPendingWriteProposals] = useState<FileWriteProposalDto[]>([]);
  const [writeConfirmations, setWriteConfirmations] = useState<WriteConfirmation[]>([]);
  const writeConfirmationTimersRef = useRef<Map<number, number>>(new Map());

  // phase 29: pending revision cards with assessment-first rendering
  const [pendingRevisions, setPendingRevisions] = useState<RevisionProposalDto[]>([]);
  // map from original revision_id → loaded alternative proposal
  const [alternativeRevisions, setAlternativeRevisions] = useState<Record<number, RevisionProposalDto>>({});
  const [loadingAlternativeFor, setLoadingAlternativeFor] = useState<number | null>(null);

  // phase 28: proactive messages are delivered as chat bubbles; no banner state needed.
  const [speculativeSubtask, setSpeculativeSubtask] = useState<SpeculativeSubtaskState | null>(null);
  // d1: track whether tts audio is currently playing for the barge-in hint
  const [ttsActivePlaying, setTtsActivePlaying] = useState(false);
  const [ttsBargeInHintDismissed, setTtsBargeInHintDismissed] = useState(false);

  const [onboardingStatus, setOnboardingStatus] =
    useState<OnboardingStatusDto | null>(null);
  const [onboardingVisible, setOnboardingVisible] = useState(false);
  const [onboardingStep, setOnboardingStep] = useState<OnboardingStep>(1);
  const [onboardingBusy, setOnboardingBusy] = useState(false);
  const [apiKeyInput, setApiKeyInput] = useState("");
  const [anthropicKeyInput, setAnthropicKeyInput] = useState("");
  const [apiKeyValidation, setApiKeyValidation] =
    useState<ApiKeyValidationDto | null>(null);
  const [workspaceFolder, setWorkspaceFolder] = useState<string | null>(null);

  const activeTaskRef = useRef<TaskDto | null>(null);
  const docSwitchTaskCandidates = useMemo(
    () => tasks.filter((task) => !activeTask || task.id !== activeTask.id).slice(0, 3),
    [activeTask, tasks]
  );
  const taskSwitcherTasks = useMemo(() => tasks.slice(0, 5), [tasks]);
  const streamingTurnIdRef = useRef<string | null>(null);
  const onboardingSnoozedRef = useRef(false);
  const messageInputRef = useRef<HTMLInputElement | null>(null);
  const messagesEndRef = useRef<HTMLDivElement | null>(null);
  const apiKeyInputRef = useRef<HTMLInputElement | null>(null);
  const onboardingPrimaryActionRef = useRef<HTMLButtonElement | null>(null);
  const pendingInteractiveFocusRef = useRef(false);
  const modeRef = useRef<OverlayMode>(mode);
  const ambientRef = useRef<AmbientStateDto | null>(null);
  const onboardingVisibleRef = useRef(onboardingVisible);
  const onboardingStepRef = useRef<OnboardingStep>(onboardingStep);
  const hasStoredApiKeyRef = useRef(false);

  useEffect(() => {
    modeRef.current = mode;
    ambientRef.current = ambient;
    onboardingVisibleRef.current = onboardingVisible;
    onboardingStepRef.current = onboardingStep;
    hasStoredApiKeyRef.current = Boolean(onboardingStatus?.has_stored_api_key);
  }, [ambient, mode, onboardingStatus?.has_stored_api_key, onboardingStep, onboardingVisible]);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView?.({ behavior: "smooth" });
  }, [
    messages,
    pendingWriteProposals.length,
    speculativeSubtask,
    streamingTurnId,
    writeConfirmations.length
  ]);

  // g3: show the folder-connect soft prompt once per session after the first
  // successful message exchange when onboarding is complete and no folder is
  // connected. does not fire during onboarding or on startup.
  useEffect(() => {
    if (folderPromptShownRef.current) return;
    if (!onboardingStatus?.onboarding_complete) return;
    if (watcherStatus?.is_watching) return;
    if (messages.length === 0) return;
    const hasAssistantResponse = messages.some((m) => m.role === "assistant");
    if (!hasAssistantResponse) return;
    folderPromptShownRef.current = true;
    setShowFolderConnectPrompt(true);
  }, [messages, onboardingStatus?.onboarding_complete, watcherStatus?.is_watching]);

  useEffect(() => {
    return () => {
      writeConfirmationTimersRef.current.forEach((timerId) => window.clearTimeout(timerId));
      writeConfirmationTimersRef.current.clear();
    };
  }, []);

  const refreshAmbient = useCallback(async () => {
    try {
      const snapshot = await getAmbientState();
      setAmbient(snapshot);
      setMode(snapshot.overlay_mode);
    } catch (error) {
      setErrorMessage(String(error));
    }
  }, []);

  const refreshMessages = useCallback(async (taskId: number) => {
    try {
      const [list, status] = await Promise.all([
        listMessages(taskId),
        getWatcherStatus(taskId).catch(() => null),
      ]);
      // overlay shows the recent tail of the conversation.
      setMessages(list.slice(-20));
      if (status) setWatcherStatus(status);
    } catch (error) {
      setErrorMessage(String(error));
    }
  }, []);

  const refreshCompanionWork = useCallback(async (taskId: number) => {
    const [subtaskList, proposals, revisions] = await Promise.all([
      listSubtasks(taskId).catch(() => []),
      listFileWriteProposals(taskId).catch(() => []),
      listTaskPendingRevisions(taskId).catch(() => []),
    ]);
    const running = subtaskList.find((subtask) => subtask.status === "running") ?? null;
    setActiveSubtask(
      running
        ? { id: running.subtask_id, taskId: running.task_id, title: running.title }
        : null
    );
    setPendingWriteProposals(
      proposals.filter((proposal) => proposal.status === "pending_approval")
    );
    // only surface revisions that are originals (not alternatives) in the overlay card list.
    // alternatives are loaded inline when "see alternative" is clicked.
    setPendingRevisions(
      revisions.filter((r) => r.parent_revision_id === null)
    );
  }, []);

  const refreshActiveTask = useCallback(async () => {
    try {
      const task = await getActiveTask();
      setActiveTaskState(task);
      setTasks(await listTasks().catch(() => []));
      if (task) {
        await refreshMessages(task.id);
        await refreshCompanionWork(task.id);
      } else {
        setMessages([]);
        setActiveSubtask(null);
        setPendingWriteProposals([]);
        setPendingRevisions([]);
        setAlternativeRevisions({});
        setWriteConfirmations([]);
      }
    } catch (error) {
      setErrorMessage(String(error));
    }
  }, [refreshCompanionWork, refreshMessages]);

  const openOnboardingWizard = useCallback(
    async (step: OnboardingStep) => {
      setOnboardingVisible(true);
      setOnboardingStep(step);
      setMode("expanded");
      try {
        await setOverlayMode("expanded");
      } catch {
        // no-op: local mode state already forces expanded rendering
      }
    },
    []
  );

  const refreshOnboarding = useCallback(
    async (openIfIncomplete: boolean) => {
      try {
        const status = await getOnboardingStatus();
        setOnboardingStatus(status);
        setWorkspaceFolder(status.preferred_workspace_folder ?? null);

        if (
          openIfIncomplete &&
          !status.onboarding_complete &&
          !onboardingSnoozedRef.current
        ) {
          await openOnboardingWizard(1);
        }
      } catch (error) {
        setErrorMessage(String(error));
      }
    },
    [openOnboardingWizard]
  );

  const focusPrimaryInteractionTarget = useCallback(() => {
    if (typeof window === "undefined") return;

    window.requestAnimationFrame(() => {
      if (onboardingVisibleRef.current) {
        if (onboardingStepRef.current === 2 && !hasStoredApiKeyRef.current) {
          apiKeyInputRef.current?.focus();
          return;
        }

        onboardingPrimaryActionRef.current?.focus();
        return;
      }

      if (modeRef.current === "expanded") {
        messageInputRef.current?.focus();
      }
    });
  }, []);

  const schedulePrimaryInteractionFocus = useCallback(() => {
    if (typeof window === "undefined") return;

    window.setTimeout(() => {
      pendingInteractiveFocusRef.current = false;
      focusPrimaryInteractionTarget();
    }, 50);
  }, [focusPrimaryInteractionTarget]);

  // initial load + notification permission probing.
  useEffect(() => {
    refreshAmbient();
    refreshActiveTask();
    void refreshOnboarding(true);
    probeNotificationPermission();
  }, [refreshActiveTask, refreshAmbient, refreshOnboarding]);

  useEffect(() => {
    activeTaskRef.current = activeTask;
  }, [activeTask]);

  // subscribe to ambient events from the rust side.
  useEffect(() => {
    const unsubscribers: Promise<UnlistenFn>[] = [];

    unsubscribers.push(
      listen<AmbientStateDto>("ambient://state-changed", (event) => {
        setAmbient(event.payload);
        setMode(event.payload.overlay_mode);
      })
    );

    unsubscribers.push(
      listen<{ hotkey: string; error: string }>(
        "ambient://hotkey-conflict",
        (event) => {
          setHotkeyConflict(
            `Hotkey ${event.payload.hotkey} is taken (${event.payload.error}). Use the tray icon to summon Jeff.`
          );
        }
      )
    );

    unsubscribers.push(
      listen<{ context_kind: string | null; context_id: number | null }>(
        "ambient://notification-click",
        (event) => {
          const kind = event.payload.context_kind ?? null;
          const id = event.payload.context_id ?? null;
          setMode("expanded");
          void setOverlayMode("expanded").catch(() => undefined);

          // phase 28: synthesis proactive notifications — message is already in DB,
          // just refresh the thread so the user sees it when the overlay opens.
          if (kind?.startsWith("proactive_") && id !== null) {
            void refreshMessages(id).catch(() => undefined);
            return;
          }

          const active = activeTaskRef.current;
          if (active) {
            void refreshMessages(active.id).catch(() => undefined);
          }
        }
      )
    );

    unsubscribers.push(
      listen<OverlayShownPayload>("ambient://overlay-shown", (event) => {
        onboardingSnoozedRef.current = false;
        if (event.payload?.interactive) {
          pendingInteractiveFocusRef.current = true;
        }
        void refreshActiveTask();
        void refreshOnboarding(true).finally(() => {
          if (event.payload?.interactive) {
            schedulePrimaryInteractionFocus();
          }
        });

        // c3: reorientation is now driven by the background monitor.
        // on interactive summon, only record the focus timestamp so the
        // monitor's cooldown logic stays correct.
        if (event.payload?.interactive) {
          const task = activeTaskRef.current;
          if (task) {
            void recordTaskFocus(task.id).catch(() => undefined);
          }
        }
      })
    );

    unsubscribers.push(
      listen<{ step?: number }>("ambient://open-onboarding", (event) => {
        onboardingSnoozedRef.current = false;
        const step = (event.payload?.step as OnboardingStep) || 1;
        void openOnboardingWizard(step);
      })
    );

    return () => {
      unsubscribers.forEach((p) =>
        p.then((unlisten) => unlisten()).catch(() => undefined)
      );
    };
  }, [openOnboardingWizard, refreshActiveTask, refreshMessages, refreshOnboarding, schedulePrimaryInteractionFocus]);

  // c3: proactive events from the background monitor.
  // d2: hotkey-pressed when overlay is already visible.
  // d3: mic shortcut.
  useEffect(() => {
    const unsubscribers: Promise<UnlistenFn>[] = [];

    // phase 28: proactive message stored in DB — refresh the message list so it
    // appears in the conversation thread.
    unsubscribers.push(
      listen<{ task_id: number; message_id: number; kind: string; message_kind: string }>(
        "proactive://message_inserted",
        (event) => {
          const active = activeTaskRef.current;
          if (active && active.id !== event.payload.task_id) return;
          void refreshMessages(event.payload.task_id).catch(() => undefined);
        }
      )
    );

    // c3: speculative subtask started by background monitor.
    unsubscribers.push(
      listen<{ subtask_id: number; task_id: number; title: string; description?: string | null }>(
        "proactive://speculative_subtask",
        (event) => {
          const active = activeTaskRef.current;
          if (active && active.id !== event.payload.task_id) return;
          setSpeculativeSubtask({
            subtask_id: event.payload.subtask_id,
            title: event.payload.title,
            description: event.payload.description ?? null
          });
        }
      )
    );

    unsubscribers.push(
      listen<CompanionStartedPayload>("subtask://companion-started", (event) => {
        const active = activeTaskRef.current;
        if (active && active.id !== event.payload.task_id) return;
        setActiveSubtask({
          id: event.payload.subtask_id,
          taskId: event.payload.task_id,
          title: event.payload.title || "background task"
        });
      })
    );

    unsubscribers.push(
      listen<CrisisCardDto>("crisis://fired", (event) => {
        const active = activeTaskRef.current;
        if (active && active.id !== event.payload.task_id) return;
        setCrisisCard(event.payload);
        setMode("expanded");
        void setOverlayMode("expanded").catch(() => undefined);
      })
    );

    unsubscribers.push(
      listen<CompanionCompletePayload>("subtask://companion-complete", (event) => {
        const active = activeTaskRef.current;
        if (active && active.id !== event.payload.task_id) return;
        setActiveSubtask((current) =>
          current?.id === event.payload.subtask_id ? null : current
        );
        // if this is the speculative subtask, load result_summary to surface the assessment
        void listSubtasks(event.payload.task_id).then((subtasks) => {
          const completed = subtasks.find((s) => s.subtask_id === event.payload.subtask_id);
          if (completed?.result_summary) {
            setSpeculativeSubtask((current) =>
              current?.subtask_id === event.payload.subtask_id
                ? { ...current, result_summary: completed.result_summary }
                : current
            );
          }
        }).catch(() => undefined);
        void refreshCompanionWork(event.payload.task_id).catch(() => undefined);
      })
    );

    unsubscribers.push(
      listen<FileWriteProposalDto>("subtask://companion-write-proposal", (event) => {
        const active = activeTaskRef.current;
        const proposal = event.payload;
        if (active && active.id !== proposal.task_id) return;
        if (proposal.status !== "pending_approval") return;
        setPendingWriteProposals((current) => {
          const exists = current.some((item) => item.id === proposal.id);
          if (exists) {
            return current.map((item) => (item.id === proposal.id ? proposal : item));
          }
          return [...current, proposal];
        });
        setMode("expanded");
        void setOverlayMode("expanded").catch(() => undefined);
      })
    );

    // d2: hotkey pressed while overlay is visible — barge-in or hide.
    unsubscribers.push(
      listen<{ overlay_visible: boolean }>("ambient://hotkey-pressed", () => {
        if (ttsActiveTurnIdRef.current !== null || streamingTurnIdRef.current !== null) {
          stopStreamingTtsPlayback();
          setTtsBargeInHintDismissed(true);
          setTtsActivePlaying(false);
          const activeTurnId = streamingTurnIdRef.current;
          if (activeTurnId) {
            streamingTurnIdRef.current = null;
            setStreamingTurnId(null);
            setStreamingText("");
            setSending(false);
            cancelStreamingTurn(activeTurnId, "user_barge_in").catch(() => undefined);
            setTrayStatus("idle").catch(() => undefined);
          }
          messageInputRef.current?.focus();
        } else {
          void hideOverlay();
        }
      })
    );

    return () => {
      unsubscribers.forEach((p) => p.then((fn) => fn()).catch(() => undefined));
    };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // d1: global keydown listener — stop tts on first printable keystroke.
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (ttsActiveTurnIdRef.current === null && streamingTurnIdRef.current === null) return;
      if (event.key.length !== 1) return;
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      stopStreamingTtsPlayback();
      setTtsActivePlaying(false);
      setTtsBargeInHintDismissed(true);
      const activeTurnId = streamingTurnIdRef.current;
      if (activeTurnId) {
        streamingTurnIdRef.current = null;
        setStreamingTurnId(null);
        setStreamingText("");
        setSending(false);
        cancelStreamingTurn(activeTurnId, "user_barge_in").catch(() => undefined);
        setTrayStatus("idle").catch(() => undefined);
      }
      messageInputRef.current?.focus();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!pendingInteractiveFocusRef.current) return;
    schedulePrimaryInteractionFocus();
  }, [
    mode,
    onboardingStatus?.has_stored_api_key,
    onboardingStep,
    onboardingVisible,
    schedulePrimaryInteractionFocus
  ]);

  useEffect(() => {
    if (!onboardingVisible) return;
    schedulePrimaryInteractionFocus();
  }, [onboardingStep, onboardingVisible, schedulePrimaryInteractionFocus]);

  useEffect(() => {
    if (!isStreamingEnabled()) {
      return;
    }

    const unsubscribers: Promise<UnlistenFn>[] = [];

    unsubscribers.push(
      listen<LlmTokenPayload>(EVENT_LLM_TOKEN, (event) => {
        if (streamingTurnIdRef.current !== event.payload.turn_id) return;
        setStreamingText((current) => current + event.payload.delta);
      })
    );

    const finalizeStreamingTurn = async (stopTts = false) => {
      streamingTurnIdRef.current = null;
      setStreamingTurnId(null);
      setStreamingText("");
      setSending(false);
      if (stopTts) {
        stopStreamingTtsPlayback();
      }
      await setTrayStatus("idle").catch(() => undefined);
      const active = activeTaskRef.current;
      if (active) {
        await refreshMessages(active.id);
      }
    };

    unsubscribers.push(
      listen<LlmCompletePayload>(EVENT_LLM_COMPLETE, (event) => {
        if (streamingTurnIdRef.current !== event.payload.turn_id) return;
        // do NOT stop TTS here — late-arriving tts_chunk events still need to play.
        void finalizeStreamingTurn(false);
      })
    );

    unsubscribers.push(
      listen<TurnCancelledPayload>(EVENT_TURN_CANCELLED, (event) => {
        if (streamingTurnIdRef.current !== event.payload.turn_id) return;
        const errMsg = extractStreamErrorMessage(event.payload.reason ?? "");
        if (errMsg) {
          setErrorMessage(errMsg);
        }
        // on cancellation, stop TTS immediately.
        void finalizeStreamingTurn(true);
      })
    );

    unsubscribers.push(
      listen<TurnCompletePayload>(EVENT_TURN_COMPLETE, (event) => {
        if (streamingTurnIdRef.current !== event.payload.turn_id) return;
        void finalizeStreamingTurn(false);
      })
    );

    // streaming tts: gated on ttsActiveTurnIdRef so late-arriving chunks after
    // llm_complete still play. gated on quiet mode so no audio in quiet mode.
    unsubscribers.push(
      listen<TtsChunkPayload>(EVENT_TTS_CHUNK, (event) => {
        const { turn_id, phrase_id, audio_b64 } = event.payload;
        if (ttsActiveTurnIdRef.current !== turn_id) return;
        // respect quiet mode: if ambient state has quiet mode on, skip audio.
        if (ambientRef.current?.quiet_mode) return;
        const bytes = Uint8Array.from(atob(audio_b64), (c) => c.charCodeAt(0));
        const blob = new Blob([bytes], { type: "audio/mpeg" });
        const url = URL.createObjectURL(blob);
        const audio = new Audio(url);
        streamTtsQueueRef.current.set(phrase_id, { audio, url });
        scheduleStreamTtsPlayback();
      })
    );

    return () => {
      unsubscribers.forEach((p) =>
        p.then((unlisten) => unlisten()).catch(() => undefined)
      );
    };
  }, [refreshMessages]);

  // phase 20: poll active window context every 3 seconds.
  // phase 20: subscribe to backend context://context-updated events.
  // the backend emits this after every 3-second poll so no client-side interval
  // is needed. fetch once on mount for the initial state (first event may not
  // have fired yet if the overlay opens within the first poll window).
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

  // phase 13: show a brief "indexed" confirmation when the watcher ingests a file.
  useEffect(() => {
    const unsub = listen<{ task_id: number; file_name: string }>(
      "workspace://file-indexed",
      (event) => {
        const active = activeTaskRef.current;
        if (!active || active.id !== event.payload.task_id) return;
        setFileIndexedNotice(`indexed: ${event.payload.file_name}`);
        if (fileIndexedTimerRef.current !== null) {
          window.clearTimeout(fileIndexedTimerRef.current);
        }
        fileIndexedTimerRef.current = window.setTimeout(() => {
          setFileIndexedNotice(null);
          fileIndexedTimerRef.current = null;
        }, 4000);
        // refresh watcher status and messages after new file is indexed.
        if (active) {
          void refreshMessages(active.id);
        }
      }
    );
    return () => {
      unsub.then((fn) => fn()).catch(() => undefined);
      if (fileIndexedTimerRef.current !== null) {
        window.clearTimeout(fileIndexedTimerRef.current);
      }
    };
  }, [refreshMessages]);

  // phase 22: load any in-flight selection capture on mount, then subscribe to
  // capture/failed/cleared events for the lifetime of the overlay window.
  // the overlay is the primary surface that opens after Cmd+Shift+V fires, so
  // this is where the indicator must be shown first.
  useEffect(() => {
    let cancelled = false;
    void getSelectionCaptureIndicator()
      .then((indicator) => { if (!cancelled) setSelectionCaptureIndicator(indicator); })
      .catch(() => undefined);

    const unsubscribers: Promise<UnlistenFn>[] = [];

    unsubscribers.push(
      listen<SelectionCaptureIndicatorDto>("selection://captured", (event) => {
        if (!cancelled) {
          setSelectionCaptureIndicator(event.payload);
          // ensure the overlay is expanded so the indicator is visible
          setMode("expanded");
          void setOverlayMode("expanded").catch(() => undefined);
        }
      })
    );

    unsubscribers.push(
      listen<SelectionCaptureIndicatorDto>("selection://capture-failed", (event) => {
        if (!cancelled) {
          setSelectionCaptureIndicator(event.payload);
          setMode("expanded");
          void setOverlayMode("expanded").catch(() => undefined);
        }
      })
    );

    unsubscribers.push(
      listen("selection://cleared", () => {
        if (!cancelled) setSelectionCaptureIndicator(null);
      })
    );

    return () => {
      cancelled = true;
      unsubscribers.forEach((p) => p.then((fn) => fn()).catch(() => undefined));
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
        // auto-dismiss after 8 seconds.
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

  const probeNotificationPermission = useCallback(async () => {
    try {
      if (typeof Notification === "undefined") {
        await markNotificationPermission("unavailable");
        return;
      }
      let permission = Notification.permission;
      if (permission === "default") {
        permission = await Notification.requestPermission();
      }
      await markNotificationPermission(permission);
    } catch (error) {
      setErrorMessage(String(error));
    }
  }, []);

  const toggleMode = useCallback(async () => {
    const next: OverlayMode = mode === "collapsed" ? "expanded" : "collapsed";
    setMode(next);
    try {
      await setOverlayMode(next);
    } catch (error) {
      setErrorMessage(String(error));
    }
  }, [mode]);

  const handleDismiss = useCallback(async () => {
    try {
      await hideOverlay();
    } catch (error) {
      setErrorMessage(String(error));
    }
  }, []);

  const handleQuietToggle = useCallback(async () => {
    if (!ambient) return;
    try {
      const next = await setQuietMode(!ambient.quiet_mode);
      setAmbient(next);
    } catch (error) {
      setErrorMessage(String(error));
    }
  }, [ambient]);

  const handleRequestAccessibilityPermission = useCallback(async () => {
    setErrorMessage(null);
    try {
      await requestAccessibilityPermission();
      window.setTimeout(() => {
        getAccessibilityPermissionStatus()
          .then((granted) => setAccessibilityPermissionGranted(granted))
          .catch(() => setAccessibilityPermissionGranted(false));
      }, 800);
    } catch (error) {
      setErrorMessage(String(error));
    }
  }, []);

  const handleSwitchTask = useCallback(
    async (taskId: number) => {
      setErrorMessage(null);
      try {
        const next = await setActiveTask(taskId);
        setActiveTaskState(next);
        activeTaskRef.current = next;
        setTasks(await listTasks().catch(() => []));
        await refreshMessages(next.id);
        await refreshCompanionWork(next.id);
        setTaskSwitcherOpen(false);
        setDocSwitchBanner(null);
      } catch (error) {
        setErrorMessage(String(error));
      }
    },
    [refreshCompanionWork, refreshMessages]
  );

  const handleStartTaskFromDocumentTitle = useCallback(async (documentTitle: string) => {
    const title = deriveTaskTitleFromPrompt(documentTitle);
    setErrorMessage(null);
    try {
      const created = await createTask(title);
      const next = await setActiveTask(created.id).catch(() => created);
      setActiveTaskState(next);
      activeTaskRef.current = next;
      setTasks(await listTasks().catch(() => []));
      await refreshMessages(next.id);
      await refreshCompanionWork(next.id);
      setDocSwitchBanner(null);
    } catch (error) {
      setErrorMessage(String(error));
    }
  }, [refreshCompanionWork, refreshMessages]);

  const handleCancelActiveSubtask = useCallback(async () => {
    if (!activeSubtask) return;
    setErrorMessage(null);
    try {
      await cancelSubtask(activeSubtask.id);
      setActiveSubtask(null);
      await refreshCompanionWork(activeSubtask.taskId);
    } catch (error) {
      setErrorMessage(String(error));
    }
  }, [activeSubtask, refreshCompanionWork]);

  const showWriteConfirmation = useCallback((fileName: string) => {
    const id = Date.now();
    setWriteConfirmations((current) => [...current, { id, fileName }].slice(-3));
    const timerId = window.setTimeout(() => {
      setWriteConfirmations((current) => current.filter((item) => item.id !== id));
      writeConfirmationTimersRef.current.delete(id);
    }, WRITE_CONFIRMATION_MS);
    writeConfirmationTimersRef.current.set(id, timerId);
  }, []);

  const handleApproveWriteProposal = useCallback(
    async (proposal: FileWriteProposalDto) => {
      setErrorMessage(null);
      try {
        const result = await approveSubtaskFileWrite(proposal.task_id, proposal.id);
        setPendingWriteProposals((current) =>
          current.filter((item) => item.id !== proposal.id)
        );
        // show the full resolved path so the user knows where the file landed
        showWriteConfirmation(result.resolved_path ?? fileNameFromPath(proposal.proposed_path));
      } catch (error) {
        setErrorMessage(String(error));
      }
    },
    [showWriteConfirmation]
  );

  const handleRejectWriteProposal = useCallback(async (proposal: FileWriteProposalDto) => {
    setErrorMessage(null);
    try {
      await rejectSubtaskFileWrite(proposal.task_id, proposal.id);
      setPendingWriteProposals((current) =>
        current.filter((item) => item.id !== proposal.id)
      );
    } catch (error) {
      setErrorMessage(String(error));
    }
  }, []);

  // phase 29: revision card handlers
  const handleApplyRevision = useCallback(async (revision: RevisionProposalDto) => {
    setErrorMessage(null);
    try {
      await applyRevision(revision.revision_id);
      setPendingRevisions((current) =>
        current.filter((r) => r.revision_id !== revision.revision_id)
      );
      setAlternativeRevisions((current) => {
        const next = { ...current };
        delete next[revision.revision_id];
        return next;
      });
    } catch (error) {
      setErrorMessage(String(error));
    }
  }, []);

  const handleRejectRevision = useCallback(async (revision: RevisionProposalDto) => {
    setErrorMessage(null);
    try {
      await rejectRevision(revision.revision_id);
      setPendingRevisions((current) =>
        current.filter((r) => r.revision_id !== revision.revision_id)
      );
      setAlternativeRevisions((current) => {
        const next = { ...current };
        delete next[revision.revision_id];
        return next;
      });
    } catch (error) {
      setErrorMessage(String(error));
    }
  }, []);

  const handleLoadAlternative = useCallback(
    async (revision: RevisionProposalDto) => {
      if (!activeTask) return;
      setLoadingAlternativeFor(revision.revision_id);
      setErrorMessage(null);
      try {
        // check if an alternative already exists on disk first
        const existing = await listRevisionAlternatives(revision.revision_id).catch(() => []);
        if (existing.length > 0) {
          setAlternativeRevisions((current) => ({
            ...current,
            [revision.revision_id]: existing[0],
          }));
        } else {
          const alt = await generateRevisionAlternative(activeTask.id, revision.revision_id);
          setAlternativeRevisions((current) => ({
            ...current,
            [revision.revision_id]: alt,
          }));
        }
      } catch (error) {
        setErrorMessage(String(error));
      } finally {
        setLoadingAlternativeFor(null);
      }
    },
    [activeTask]
  );

  const handleSubmit = useCallback(
    async (event: FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      const trimmed = input.trim();
      if (!trimmed || sending) return;

      setSending(true);
      setErrorMessage(null);
      setInfoNotice(null);
      let streamingStarted = false;

      try {
        let task = activeTask;
        if (!task) {
          const created = await createTask(deriveTaskTitleFromPrompt(trimmed));
          task = await setActiveTask(created.id).catch(() => created);
          setActiveTaskState(task);
          activeTaskRef.current = task;
          setTasks(await listTasks().catch(() => []));
          await refreshCompanionWork(task.id);
        }

        // classify intent before sending so we can route or bail.
        const { intent, slots } = await classifyOverlayMessageIntentWithFallback(task.id, trimmed);

        if (intent === "unknown") {
          setErrorMessage("Not sure — is this a question, a request to draft something, or a revision?");
          return;
        }

        await setTrayStatus("working").catch(() => undefined);

        if (isStreamingEnabled()) {
          if (streamingTurnIdRef.current) {
            await cancelStreamingTurn(streamingTurnIdRef.current, "user_barge_in").catch(
              () => undefined
            );
          }
          const turnId = await sendMessageStreaming(task.id, trimmed, "text");
          streamingStarted = true;
          ttsActiveTurnIdRef.current = turnId;
          streamingTurnIdRef.current = turnId;
          setStreamingTurnId(turnId);
          setStreamingText("");
          setInput("");
          await refreshMessages(task.id);

          // subtask intent: start a parallel chain after the stream is initiated.
          // startSubtaskChain returns quickly (spawns a rust thread); the
          // companion-started event fires the spinner once the thread is running.
          if (intent === "subtask") {
            const executionType = slots?.draft_type
              ? inferOverlaySubtaskExecutionTypeFromDraftType(slots.draft_type)
              : inferOverlaySubtaskExecutionType(trimmed);
            const description = slots?.instruction ?? trimmed;
            await startSubtaskChain(
              task.id,
              deriveOverlaySubtaskTitle(description),
              description,
              executionType,
              "text"
            ).catch((err: unknown) =>
              setErrorMessage(`Could not start parallel work: ${String(err)}`)
            );
            await refreshCompanionWork(task.id);
          }

          // revision intent: send the chat message (LLM can discuss) then nudge
          // the user to open the workspace where the artifact picker lives.
          if (intent === "revision") {
            setInfoNotice(
              "Revision noted. Open the full workspace to choose a document and apply it."
            );
          }

          return;
        }

        // non-streaming fallback
        await sendMessage(task.id, trimmed, "text");
        setInput("");
        await refreshMessages(task.id);

        if (intent === "subtask") {
          const executionType = slots?.draft_type
            ? inferOverlaySubtaskExecutionTypeFromDraftType(slots.draft_type)
            : inferOverlaySubtaskExecutionType(trimmed);
          const description = slots?.instruction ?? trimmed;
          await startSubtaskChain(
            task.id,
            deriveOverlaySubtaskTitle(description),
            description,
            executionType,
            "text"
          ).catch((err: unknown) =>
            setErrorMessage(`Could not start parallel work: ${String(err)}`)
          );
          await refreshCompanionWork(task.id);
        }

        if (intent === "revision") {
          setInfoNotice(
            "Revision noted. Open the full workspace to choose a document and apply it."
          );
        }
      } catch (error) {
        setErrorMessage(String(error));
      } finally {
        if (!streamingStarted) {
          await setTrayStatus("idle").catch(() => undefined);
          setSending(false);
        }
      }
    },
    [activeTask, input, refreshCompanionWork, refreshMessages, sending]
  );

  const handleOnboardingCancel = useCallback(() => {
    onboardingSnoozedRef.current = true;
    setOnboardingVisible(false);
    setApiKeyValidation(null);
  }, []);

  const handleOnboardingStepOneContinue = useCallback(() => {
    setOnboardingStep(2);
  }, []);

  // apex a1: optionally stores the anthropic key alongside the openai key.
  // stored without a validation call — the model router falls back to openai
  // if the key turns out to be unusable.
  const maybeStoreAnthropicKey = useCallback(async () => {
    const trimmedAnthropic = anthropicKeyInput.trim();
    if (!trimmedAnthropic) {
      return;
    }
    try {
      await storeAnthropicApiKey(trimmedAnthropic);
      setAnthropicKeyInput("");
    } catch (error) {
      console.warn(`[jeff] anthropic_key_store_failed: ${String(error)}`);
    }
  }, [anthropicKeyInput]);

  const handleOnboardingValidateApiKey = useCallback(async () => {
    const trimmed = apiKeyInput.trim();

    if (!trimmed && onboardingStatus?.has_stored_api_key) {
      setApiKeyValidation({
        is_valid: true,
        message: "Using existing stored API key."
      });
      await maybeStoreAnthropicKey();
      setOnboardingStep(3);
      return;
    }

    setOnboardingBusy(true);
    setApiKeyValidation(null);
    try {
      const validation = await validateOpenAiApiKey(trimmed);
      setApiKeyValidation(validation);
      if (!validation.is_valid) {
        return;
      }

      await storeOpenAiApiKey(trimmed);
      await maybeStoreAnthropicKey();
      await refreshOnboarding(false);
      setOnboardingStep(3);
    } catch (error) {
      setApiKeyValidation({
        is_valid: false,
        message: String(error)
      });
    } finally {
      setOnboardingBusy(false);
    }
  }, [apiKeyInput, maybeStoreAnthropicKey, onboardingStatus?.has_stored_api_key, refreshOnboarding]);

  const handleChooseWorkspaceFolder = useCallback(async () => {
    setOnboardingBusy(true);
    setErrorMessage(null);
    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
        title: "Choose your first workspace folder"
      });

      const folderPath =
        typeof selected === "string"
          ? selected
          : Array.isArray(selected) && typeof selected[0] === "string"
            ? selected[0]
            : null;

      if (!folderPath) {
        return;
      }

      await setPreferredWorkspaceFolder(folderPath);
      setWorkspaceFolder(folderPath);
      setShowFolderConnectPrompt(false);
      await refreshOnboarding(false);
    } catch (error) {
      setErrorMessage(String(error));
    } finally {
      setOnboardingBusy(false);
    }
  }, [refreshOnboarding]);

  const handleSkipWorkspaceFolder = useCallback(async () => {
    setOnboardingBusy(true);
    try {
      await clearPreferredWorkspaceFolder();
      setWorkspaceFolder(null);
      await refreshOnboarding(false);
      setOnboardingStep(4);
    } catch (error) {
      setErrorMessage(String(error));
    } finally {
      setOnboardingBusy(false);
    }
  }, [refreshOnboarding]);

  const handleWorkspaceStepContinue = useCallback(() => {
    setOnboardingStep(4);
  }, []);

  const handleFinishOnboarding = useCallback(async () => {
    setOnboardingBusy(true);
    try {
      await completeOnboarding();
      await refreshOnboarding(false);
      setOnboardingVisible(false);
      onboardingSnoozedRef.current = false;
      setOnboardingStep(1);
      setApiKeyValidation(null);
      window.setTimeout(() => messageInputRef.current?.focus(), 50);
    } catch (error) {
      setErrorMessage(String(error));
    } finally {
      setOnboardingBusy(false);
    }
  }, [refreshOnboarding]);

  const handleFixApiKey = useCallback(() => {
    onboardingSnoozedRef.current = false;
    void openOnboardingWizard(2);
  }, [openOnboardingWizard]);

  // play the next queued streaming tts phrase in phrase_id order.
  // called each time a new chunk arrives and each time a phrase finishes.
  function scheduleStreamTtsPlayback() {
    if (streamTtsCurrentRef.current !== null) return;
    const next = streamTtsQueueRef.current.get(streamTtsNextPhraseRef.current);
    if (!next) return;
    streamTtsQueueRef.current.delete(streamTtsNextPhraseRef.current);
    streamTtsNextPhraseRef.current += 1;
    const { audio, url } = next;
    streamTtsCurrentRef.current = audio;
    setTtsActivePlaying(true);

    audio.onended = () => {
      URL.revokeObjectURL(url);
      streamTtsCurrentRef.current = null;
      if (streamTtsQueueRef.current.size === 0) {
        ttsActiveTurnIdRef.current = null;
        setTtsActivePlaying(false);
        void setTrayStatus("idle").catch(() => undefined);
      }
      scheduleStreamTtsPlayback();
    };
    audio.onerror = () => {
      URL.revokeObjectURL(url);
      streamTtsCurrentRef.current = null;
      if (streamTtsQueueRef.current.size === 0) {
        ttsActiveTurnIdRef.current = null;
        setTtsActivePlaying(false);
        void setTrayStatus("idle").catch(() => undefined);
      }
      scheduleStreamTtsPlayback();
    };

    void audio.play().catch(() => {
      streamTtsCurrentRef.current = null;
      if (streamTtsQueueRef.current.size === 0) {
        ttsActiveTurnIdRef.current = null;
        setTtsActivePlaying(false);
      }
    });
  }

  // immediately stop all streaming tts playback and drain the queue.
  function stopStreamingTtsPlayback() {
    if (streamTtsCurrentRef.current) {
      try { streamTtsCurrentRef.current.pause(); } catch { /* ignore */ }
      streamTtsCurrentRef.current = null;
    }
    for (const { audio, url } of streamTtsQueueRef.current.values()) {
      try { audio.pause(); } catch { /* ignore */ }
      URL.revokeObjectURL(url);
    }
    streamTtsQueueRef.current.clear();
    streamTtsNextPhraseRef.current = 1;
    ttsActiveTurnIdRef.current = null;
    setTtsActivePlaying(false);
  }

  // barge-in: stop tts, synchronously unlock input, cancel any in-flight streaming turn.
  async function stopAndBargeIn() {
    stopStreamingTtsPlayback();
    stopPartialStt();
    setTtsActivePlaying(false);
    setTtsBargeInHintDismissed(true);
    const activeTurnId = streamingTurnIdRef.current;
    if (activeTurnId) {
      streamingTurnIdRef.current = null;
      setStreamingTurnId(null);
      setStreamingText("");
      setSending(false);
      await cancelStreamingTurn(activeTurnId, "user_barge_in").catch(() => undefined);
    }
    await setTrayStatus("idle").catch(() => undefined);
  }

  // try to start web speech api recognition for early routing before whisper.
  // on interim result with confidence >= 0.7, submits the transcript immediately.
  function tryStartPartialStt(taskId: number) {
    const win = window as unknown as Record<string, unknown>;
    const SpeechRecognitionCtor = (win["SpeechRecognition"] ?? win["webkitSpeechRecognition"]) as
      | (new () => OverlaySpeechRecognition)
      | undefined;
    if (!SpeechRecognitionCtor) return;

    partialSttSentRef.current = false;
    let recognition: OverlaySpeechRecognition;
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
          // stop the recorder so finalizeVoiceInput skips whisper
          const recorder = mediaRecorderRef.current;
          if (recorder && recorder.state !== "inactive") {
            recorder.stop();
          }
          setRecording(false);
          void submitVoiceMessage(taskId, text);
          break;
        }
      }
    };

    recognition.onerror = () => { speechRecognitionRef.current = null; };
    recognition.onend = () => { speechRecognitionRef.current = null; };

    try {
      recognition.start();
      speechRecognitionRef.current = recognition;
    } catch {
      speechRecognitionRef.current = null;
    }
  }

  function stopPartialStt() {
    if (speechRecognitionRef.current) {
      try { speechRecognitionRef.current.stop(); } catch { /* ignore */ }
      speechRecognitionRef.current = null;
    }
  }

  // shared voice message submission path used by both partial STT and whisper.
  const submitVoiceMessage = useCallback(async (taskId: number, text: string) => {
    setSending(true);
    setErrorMessage(null);
    setInfoNotice(null);
    let streamingStarted = false;
    try {
      // classify intent. for voice, coerce "unknown" to "answer" — discarding a
      // transcribed voice message silently would feel broken.
      const { intent, slots } = await classifyOverlayMessageIntentWithFallback(taskId, text);
      const routedIntent: OverlayRoutedIntent = intent === "unknown" ? "answer" : intent;

      await setTrayStatus("working").catch(() => undefined);
      if (isStreamingEnabled()) {
        if (streamingTurnIdRef.current) {
          await cancelStreamingTurn(streamingTurnIdRef.current, "user_barge_in").catch(() => undefined);
        }
        const turnId = await sendMessageStreaming(taskId, text, "voice");
        streamingStarted = true;
        ttsActiveTurnIdRef.current = turnId;
        streamingTurnIdRef.current = turnId;
        setStreamingTurnId(turnId);
        setStreamingText("");
        await refreshMessages(taskId);

        if (routedIntent === "subtask") {
          const executionType = slots?.draft_type
            ? inferOverlaySubtaskExecutionTypeFromDraftType(slots.draft_type)
            : inferOverlaySubtaskExecutionType(text);
          const description = slots?.instruction ?? text;
          await startSubtaskChain(
            taskId,
            deriveOverlaySubtaskTitle(description),
            description,
            executionType,
            "voice"
          ).catch((err: unknown) =>
            setErrorMessage(`Could not start parallel work: ${String(err)}`)
          );
          await refreshCompanionWork(taskId);
        }

        if (routedIntent === "revision") {
          setInfoNotice(
            "Revision noted. Open the full workspace to choose a document and apply it."
          );
        }
      } else {
        await sendMessage(taskId, text, "voice");
        await refreshMessages(taskId);

        if (routedIntent === "subtask") {
          const executionType = slots?.draft_type
            ? inferOverlaySubtaskExecutionTypeFromDraftType(slots.draft_type)
            : inferOverlaySubtaskExecutionType(text);
          const description = slots?.instruction ?? text;
          await startSubtaskChain(
            taskId,
            deriveOverlaySubtaskTitle(description),
            description,
            executionType,
            "voice"
          ).catch((err: unknown) =>
            setErrorMessage(`Could not start parallel work: ${String(err)}`)
          );
          await refreshCompanionWork(taskId);
        }

        if (routedIntent === "revision") {
          setInfoNotice(
            "Revision noted. Open the full workspace to choose a document and apply it."
          );
        }
      }
    } catch (error) {
      setErrorMessage(String(error));
      await setTrayStatus("idle").catch(() => undefined);
    } finally {
      if (!streamingStarted) setSending(false);
    }
  }, [refreshCompanionWork, refreshMessages]);

  const handleStartVoiceRecording = useCallback(async () => {
    if (recording) return;
    setErrorMessage(null);
    // if jeff is speaking or streaming, barge in first.
    await stopAndBargeIn();
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      const recorder = new MediaRecorder(stream);
      mediaStreamRef.current = stream;
      mediaRecorderRef.current = recorder;
      audioChunksRef.current = [];
      partialSttSentRef.current = false;

      recorder.ondataavailable = (event) => {
        if (event.data.size > 0) {
          audioChunksRef.current.push(event.data);
        }
      };

      recorder.onstop = () => {
        void handleFinalizeVoiceInput();
      };

      recorder.start();
      setRecording(true);

      // start partial stt alongside whisper for early routing.
      const task = activeTaskRef.current;
      if (task) {
        tryStartPartialStt(task.id);
      }
    } catch (error) {
      setErrorMessage("Microphone access denied or unavailable.");
      void error;
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [recording]);

  const handleStopVoiceRecording = useCallback(() => {
    stopPartialStt();
    const recorder = mediaRecorderRef.current;
    if (!recorder || recorder.state === "inactive") return;
    recorder.stop();
    setRecording(false);
    mediaStreamRef.current?.getTracks().forEach((track) => track.stop());
    mediaStreamRef.current = null;
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handleFinalizeVoiceInput = useCallback(async () => {
    stopPartialStt();
    const chunks = audioChunksRef.current;
    audioChunksRef.current = [];
    mediaRecorderRef.current = null;

    // if partial stt already routed the message, skip whisper.
    if (partialSttSentRef.current) {
      partialSttSentRef.current = false;
      mediaStreamRef.current?.getTracks().forEach((track) => track.stop());
      mediaStreamRef.current = null;
      return;
    }

    if (chunks.length === 0) return;

    setSending(true);
    setErrorMessage(null);
    try {
      const mimeType = chunks[0].type || "audio/webm";
      const blob = new Blob(chunks, { type: mimeType });
      const audioBase64 = await blobToBase64(blob);
      const transcription = await transcribeAudio(audioBase64, mimeType);
      const text = transcription.text.trim();
      if (!text) {
        setSending(false);
        return;
      }

      let task = activeTaskRef.current;
      if (!task) {
        const created = await createTask(deriveTaskTitleFromPrompt(text));
        task = await setActiveTask(created.id).catch(() => created);
        setActiveTaskState(task);
        activeTaskRef.current = task;
        setTasks(await listTasks().catch(() => []));
        await refreshCompanionWork(task.id);
      }

      await submitVoiceMessage(task.id, text);
    } catch (error) {
      setErrorMessage(String(error));
      await setTrayStatus("idle").catch(() => undefined);
      setSending(false);
    } finally {
      mediaStreamRef.current?.getTracks().forEach((track) => track.stop());
      mediaStreamRef.current = null;
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshCompanionWork, refreshMessages, submitVoiceMessage]);

  const handleDismissSelectionCapture = useCallback(async () => {
    try {
      await dismissSelectionCapture();
      setSelectionCaptureIndicator(null);
    } catch {
      setSelectionCaptureIndicator(null);
    }
  }, []);

  const handleCrisisNotUrgent = useCallback(async () => {
    const card = crisisCard;
    if (!card) return;
    try {
      await recordCrisisFeedback(card.task_id, card.class, card.evidence);
    } catch {
      // feedback logging should not keep an incorrect card on screen.
    }
    setCrisisCard(null);
  }, [crisisCard]);

  // apex c4: end an active realtime voice session.
  const handleEndVoiceSession = useCallback(() => {
    voiceConnRef.current?.close();
    voiceConnRef.current = null;
    setVoiceMuted(false);
    setVoiceState("closed");
  }, []);

  // apex c4: start (or toggle off) a realtime voice session. mints an ephemeral
  // session in the backend, then opens the WebRTC audio in the browser. falls
  // back to the STT/TTS pipeline when realtime is off or unavailable.
  const handleStartVoiceSession = useCallback(async () => {
    if (recording) {
      handleStopVoiceRecording();
      return;
    }
    if (voiceConnRef.current) {
      handleEndVoiceSession();
      return;
    }
    if (voiceStartPendingRef.current) return;
    voiceStartPendingRef.current = true;
    let fallbackStarted = false;
    const fallBackToVoicePipeline = (notice: string) => {
      if (fallbackStarted) return;
      fallbackStarted = true;
      voiceConnRef.current?.close();
      voiceConnRef.current = null;
      setVoiceMuted(false);
      setVoiceState("fallback");
      setInfoNotice(notice);
      void handleStartVoiceRecording();
    };
    setVoiceState("connecting");
    try {
      const session = await startVoiceSession();
      if (session.fallback || !session.client_secret) {
        fallBackToVoicePipeline(session.notice ?? "Using the voice pipeline.");
        return;
      }
      const conn = await connectRealtimeVoice(session.client_secret, session.model, {
        onTranscript: (role, text) => {
          const task = activeTaskRef.current;
          if (task) {
            void persistVoiceTranscript(task.id, role, text).catch(() => undefined);
          }
        },
        onToolCall: async (name, args) => {
          const task = activeTaskRef.current;
          if (!task) return { action: "none", text: null, error: "no active task" };
          return handleVoiceToolCall(task.id, name, args);
        },
        onStateChange: (next) => {
          if (next === "error") {
            fallBackToVoicePipeline(
              "Realtime voice disconnected; continuing with the voice pipeline."
            );
            return;
          }
          setVoiceState(next);
        }
      });
      if (!conn) {
        fallBackToVoicePipeline("Realtime voice unavailable; using the voice pipeline.");
        return;
      }
      voiceConnRef.current = conn;
      setVoiceState("live");
    } catch {
      fallBackToVoicePipeline("Realtime voice unavailable; using the voice pipeline.");
    } finally {
      voiceStartPendingRef.current = false;
    }
  }, [handleEndVoiceSession, handleStartVoiceRecording, handleStopVoiceRecording, recording]);

  const handleToggleVoiceMute = useCallback(() => {
    setVoiceMuted((prev) => {
      const next = !prev;
      voiceConnRef.current?.setMuted(next);
      return next;
    });
  }, []);

  const openVoiceSessionFromWakeWord = useCallback(async () => {
    if (voiceConnRef.current) {
      return;
    }
    await handleStartVoiceSession();
  }, [handleStartVoiceSession]);

  // apex c4: the backend owns the single global Cmd/Ctrl+Shift+M registration.
  // Do not also install a DOM shortcut: duplicate delivery could close a session
  // immediately after opening it, and Shift+V belongs to selection capture.
  useEffect(() => {
    const unsub = listen("ambient://mic-shortcut", () => {
      void handleStartVoiceSession();
    });
    return () => {
      unsub.then((fn) => fn()).catch(() => undefined);
    };
  }, [handleStartVoiceSession]);

  // apex c5: wake-word detector only sends a wake token; the overlay plays a
  // local acknowledgement cue and opens voice without toggling off an open session.
  useEffect(() => {
    const unsub = listen("wake_word://detected", () => {
      playWakeWordAckCue();
      setMode("expanded");
      void setOverlayMode("expanded").catch(() => undefined);
      void openVoiceSessionFromWakeWord();
    });
    return () => {
      unsub.then((fn) => fn()).catch(() => undefined);
    };
  }, [openVoiceSessionFromWakeWord]);

  const hotkeyLabel = useMemo(
    () => (ambient ? formatHotkey(ambient.hotkey) : ""),
    [ambient]
  );

  const statusLabel = ambient ? describeStatus(ambient.tray_status) : "idle";
  const hasCompanionStreamItems =
    pendingWriteProposals.length > 0 ||
    pendingRevisions.length > 0 ||
    writeConfirmations.length > 0 ||
    speculativeSubtask !== null ||
    streamingTurnId !== null;

  return (
    <div
      className={`overlay-root overlay-${mode}`}
      data-testid="overlay-root"
      data-mode={mode}
    >
      <div className="overlay-header">
        <div className="overlay-status">
          <span
            className={`overlay-status-dot overlay-status-${statusLabel}`}
            aria-hidden
          />
          <span className="overlay-status-label">jeff</span>
          {ambient?.wake_word_armed ? (
            <span className="overlay-wake-word-armed" data-testid="wake-word-armed">
              wake word armed
            </span>
          ) : null}
        </div>
        <div className="overlay-controls">
          {ambient?.quiet_mode ? (
            <button
              type="button"
              className="overlay-quiet-on"
              onClick={handleQuietToggle}
              title="Quiet mode on"
            >
              quiet
            </button>
          ) : (
            <button
              type="button"
              className="overlay-quiet-off"
              onClick={handleQuietToggle}
              title="Toggle quiet mode"
            >
              quiet off
            </button>
          )}
          <button
            type="button"
            className="overlay-toggle"
            onClick={toggleMode}
            data-testid="overlay-toggle-mode"
          >
            {mode === "collapsed" ? "expand" : "collapse"}
          </button>
          <button
            type="button"
            className="overlay-dismiss"
            onClick={handleDismiss}
            title={`Dismiss (${hotkeyLabel})`}
          >
            hide
          </button>
        </div>
      </div>

      {hotkeyConflict ? (
        <div className="overlay-banner overlay-banner-warn">
          {hotkeyConflict}
        </div>
      ) : null}

      {mode === "expanded" ? (
        <div className="overlay-expanded">
          <div className="overlay-task-row">
            <div className="overlay-task-switcher">
              <div className="overlay-task-label-row">
                <span className="overlay-task-label">
                  {activeTask ? activeTask.title : "No active task"}
                </span>
                {tasks.length > 1 ? (
                  <button
                    type="button"
                    className="overlay-task-menu-button"
                    aria-label="switch task"
                    aria-expanded={taskSwitcherOpen}
                    onClick={() => setTaskSwitcherOpen((current) => !current)}
                    data-testid="overlay-task-switcher"
                  >
                    &middot;
                  </button>
                ) : null}
              </div>
              {taskSwitcherOpen ? (
                <div className="overlay-task-menu" data-testid="overlay-task-menu">
                  {taskSwitcherTasks.map((task) => (
                    <button
                      key={task.id}
                      type="button"
                      className="overlay-task-menu-item"
                      onClick={() => void handleSwitchTask(task.id)}
                      data-testid={`overlay-task-option-${task.id}`}
                    >
                      <span
                        className={
                          activeTask?.id === task.id
                            ? "overlay-task-active-dot"
                            : "overlay-task-empty-dot"
                        }
                        aria-hidden
                      />
                      <span>{task.title}</span>
                    </button>
                  ))}
                </div>
              ) : null}
            </div>
          </div>

          {activeContext && activeContext.document_title ? (
            <div className="overlay-context-line">
              {activeContext.app_name} &mdash; {activeContext.document_title}
            </div>
          ) : null}

          {activeTask ? (
            <div className="overlay-watcher-line">
              {watcherStatus?.is_watching ? (
                <>
                  watching{" "}
                  <span className="overlay-watcher-folder">
                    {watcherStatus.watched_path
                      ? watcherStatus.watched_path.split("/").pop()
                      : "folder"}
                  </span>
                  {fileIndexedNotice ? (
                    <span className="overlay-watcher-indexed"> · {fileIndexedNotice}</span>
                  ) : null}
                </>
              ) : (
                <span className="overlay-watcher-idle">no folder connected</span>
              )}
            </div>
          ) : null}

          {activeTask && activeSubtask ? (
            <div className="overlay-subtask-line" data-testid="overlay-active-subtask">
              <span className="overlay-subtask-spinner" aria-hidden />
              <span className="overlay-subtask-label">
                jeff is working on: <strong>{activeSubtask.title}</strong>
              </span>
              <button
                type="button"
                className="overlay-subtask-cancel"
                onClick={() => void handleCancelActiveSubtask()}
                data-testid="overlay-cancel-subtask"
              >
                cancel
              </button>
            </div>
          ) : null}

          {crisisCard ? (
            <div className="overlay-crisis-card" data-testid="overlay-crisis-card">
              <div className="overlay-crisis-header">
                <span>{crisisCard.title}</span>
                <button
                  type="button"
                  onClick={() => setCrisisCard(null)}
                  aria-label="dismiss crisis card"
                >
                  dismiss
                </button>
              </div>
              <p>{crisisCard.message}</p>
              <p className="overlay-crisis-evidence">{crisisCard.evidence}</p>
              {crisisCard.quiet_downgraded ? (
                <p className="overlay-crisis-evidence">quiet mode: persistent card only</p>
              ) : null}
              <div className="overlay-banner-actions">
                <button
                  type="button"
                  onClick={() => void handleCrisisNotUrgent()}
                  data-testid="crisis-not-urgent"
                >
                  this wasn't urgent
                </button>
              </div>
            </div>
          ) : null}

          {onboardingStatus?.onboarding_complete &&
          accessibilityPermissionGranted === false &&
          !accessibilityPromptDismissed &&
          !activeContext ? (
            <div className="overlay-banner overlay-banner-info" data-testid="accessibility-context-prompt">
              <span>Jeff needs accessibility permission to know which document you have open.</span>
              <div className="overlay-banner-actions">
                <button
                  type="button"
                  onClick={() => void handleRequestAccessibilityPermission()}
                  data-testid="request-accessibility-permission"
                >
                  enable
                </button>
                <button type="button" onClick={() => setAccessibilityPromptDismissed(true)}>
                  not now
                </button>
              </div>
            </div>
          ) : null}

          {docSwitchBanner ? (
            <div className="overlay-banner overlay-banner-info" data-testid="doc-switch-banner">
              <span>
                You switched to {docSwitchBanner.document_title}. Want to start or switch tasks?
              </span>
              <div className="overlay-banner-actions">
                <button
                  type="button"
                  onClick={() => void handleStartTaskFromDocumentTitle(docSwitchBanner.document_title)}
                  data-testid="doc-switch-start-task"
                >
                  start task
                </button>
                {docSwitchTaskCandidates.map((task) => (
                  <button
                    type="button"
                    key={task.id}
                    onClick={() => void handleSwitchTask(task.id)}
                  >
                    switch
                  </button>
                ))}
                <button type="button" onClick={() => setDocSwitchBanner(null)}>
                  dismiss
                </button>
              </div>
            </div>
          ) : null}

          {onboardingVisible ? (
            <section className="overlay-onboarding" data-testid="overlay-onboarding">
              <div className="overlay-onboarding-meta" data-testid="overlay-onboarding-step-count">
                Step {onboardingStep} of 5
              </div>

              {onboardingStep === 1 ? (
                <div data-testid="onboarding-step-1" className="overlay-onboarding-step">
                  <h3>What Jeff is</h3>
                  <p>
                    Jeff is your task-focused coworker in a companion window.
                    It keeps context from your task and helps you move work forward.
                    You stay in control of every write and every decision.
                  </p>
                  <div className="overlay-onboarding-actions">
                    <button
                      type="button"
                      ref={onboardingPrimaryActionRef}
                      onClick={handleOnboardingStepOneContinue}
                      data-testid="onboarding-continue-step-1"
                    >
                      Continue
                    </button>
                    <button type="button" onClick={handleOnboardingCancel}>
                      Cancel
                    </button>
                  </div>
                </div>
              ) : null}

              {onboardingStep === 2 ? (
                <div data-testid="onboarding-step-2" className="overlay-onboarding-step">
                  <h3>API key setup</h3>
                  <p>
                    Add your OpenAI API key. Jeff validates it and stores it in macOS Keychain.
                  </p>
                  <input
                    ref={apiKeyInputRef}
                    type="password"
                    className="overlay-input"
                    value={apiKeyInput}
                    onChange={(event) => setApiKeyInput(event.target.value)}
                    placeholder="sk-..."
                    data-testid="onboarding-api-key-input"
                    disabled={onboardingBusy}
                  />
                  {onboardingStatus?.has_stored_api_key ? (
                    <p className="overlay-meta">A key is already available from {onboardingStatus.api_key_source}.</p>
                  ) : null}
                  <p className="overlay-meta">
                    Optional: add an Anthropic API key for Jeff's strongest reasoning. Without it, everything runs on OpenAI.
                  </p>
                  <input
                    type="password"
                    className="overlay-input"
                    value={anthropicKeyInput}
                    onChange={(event) => setAnthropicKeyInput(event.target.value)}
                    placeholder="sk-ant-... (optional)"
                    data-testid="onboarding-anthropic-key-input"
                    disabled={onboardingBusy}
                  />
                  {apiKeyValidation ? (
                    <p
                      className={apiKeyValidation.is_valid ? "overlay-meta" : "overlay-error"}
                      data-testid="onboarding-api-key-validation"
                    >
                      {apiKeyValidation.message}
                    </p>
                  ) : null}
                  <div className="overlay-onboarding-actions">
                    <button
                      type="button"
                      ref={onboardingPrimaryActionRef}
                      onClick={() => void handleOnboardingValidateApiKey()}
                      disabled={onboardingBusy}
                      data-testid="onboarding-continue-step-2"
                    >
                      {onboardingBusy ? "Validating..." : "Validate and continue"}
                    </button>
                    <button type="button" onClick={handleOnboardingCancel} disabled={onboardingBusy}>
                      Cancel
                    </button>
                  </div>
                  <p className="overlay-meta">
                    No key? Use bundled inference — Jeff provides metered access, no key entry.
                  </p>
                  <button
                    type="button"
                    className="overlay-secondary"
                    data-testid="onboarding-inference-bundled"
                    disabled={onboardingBusy}
                    onClick={() => {
                      void setInferenceMode("bundled");
                      setOnboardingStep(3);
                    }}
                  >
                    Use bundled inference (no key)
                  </button>
                </div>
              ) : null}

              {onboardingStep === 3 ? (
                <div data-testid="onboarding-step-3" className="overlay-onboarding-step">
                  <h3>Workspace folder</h3>
                  <p>
                    Pick a first folder for Jeff to watch, or skip for now.
                  </p>
                  <p className="overlay-meta" data-testid="onboarding-workspace-selection">
                    {workspaceFolder ? `Selected: ${workspaceFolder}` : "No folder selected yet."}
                  </p>
                  <div className="overlay-onboarding-actions">
                    <button
                      type="button"
                      ref={onboardingPrimaryActionRef}
                      onClick={() => void handleChooseWorkspaceFolder()}
                      disabled={onboardingBusy}
                      data-testid="onboarding-choose-folder"
                    >
                      Choose folder
                    </button>
                    <button
                      type="button"
                      onClick={handleWorkspaceStepContinue}
                      disabled={onboardingBusy}
                      data-testid="onboarding-continue-step-3"
                    >
                      Continue
                    </button>
                    <button
                      type="button"
                      onClick={() => void handleSkipWorkspaceFolder()}
                      disabled={onboardingBusy}
                      data-testid="onboarding-skip-folder"
                    >
                      Skip for now
                    </button>
                  </div>
                  <div className="overlay-onboarding-actions">
                    <button type="button" onClick={handleOnboardingCancel} disabled={onboardingBusy}>
                      Cancel
                    </button>
                  </div>
                </div>
              ) : null}

              {onboardingStep === 4 ? (
                <div data-testid="onboarding-step-4" className="overlay-onboarding-step">
                  <h3>Window context</h3>
                  <p>
                    Jeff can see which app and document you have open to give better
                    answers without you describing your screen. This requires macOS
                    Accessibility permission.
                  </p>
                  {accessibilityPermissionGranted ? (
                    <p
                      className="overlay-meta"
                      data-testid="onboarding-accessibility-granted"
                    >
                      Permission granted. Jeff will track your active window.
                    </p>
                  ) : (
                    <div className="overlay-onboarding-actions">
                      <button
                        type="button"
                        ref={
                          accessibilityPermissionGranted
                            ? undefined
                            : onboardingPrimaryActionRef
                        }
                        onClick={() => void handleRequestAccessibilityPermission()}
                        data-testid="onboarding-enable-accessibility"
                      >
                        Enable
                      </button>
                    </div>
                  )}
                  <div className="overlay-onboarding-actions">
                    <button
                      type="button"
                      ref={
                        accessibilityPermissionGranted
                          ? onboardingPrimaryActionRef
                          : undefined
                      }
                      onClick={() => setOnboardingStep(5)}
                      data-testid="onboarding-continue-step-4"
                    >
                      {accessibilityPermissionGranted ? "Continue" : "Skip for now"}
                    </button>
                    <button type="button" onClick={handleOnboardingCancel}>
                      Cancel
                    </button>
                  </div>
                </div>
              ) : null}

              {onboardingStep === 5 ? (
                <div data-testid="onboarding-step-5" className="overlay-onboarding-step">
                  <h3>Ready</h3>
                  <p>
                    You are ready to use Jeff. Press {hotkeyLabel || "Cmd/Ctrl Shift J"} any time to summon it.
                  </p>
                  <div className="overlay-onboarding-actions">
                    <button
                      type="button"
                      ref={onboardingPrimaryActionRef}
                      onClick={() => void handleFinishOnboarding()}
                      disabled={onboardingBusy}
                      data-testid="onboarding-complete"
                    >
                      Start with your first message
                    </button>
                    <button type="button" onClick={handleOnboardingCancel} disabled={onboardingBusy}>
                      Cancel
                    </button>
                  </div>
                </div>
              ) : null}
            </section>
          ) : (
            <>
              {!activeTask ? (
                <div className="overlay-banner overlay-banner-info" data-testid="overlay-no-active-task">
                  Tell me what you're working on.
                </div>
              ) : null}

              {showFolderConnectPrompt ? (
                <div className="overlay-banner overlay-banner-info" data-testid="overlay-folder-prompt">
                  <span>
                    {activeContext?.document_title
                      ? `Jeff can see ${activeContext.document_title} is open.`
                      : accessibilityPermissionGranted
                        ? "Jeff sees your active window."
                        : ""}{" "}
                    <button
                      type="button"
                      className="overlay-inline-link"
                      onClick={() => void handleChooseWorkspaceFolder()}
                      data-testid="overlay-folder-prompt-connect"
                    >
                      Connect a folder
                    </button>{" "}
                    to give Jeff full context.
                  </span>
                  <div className="overlay-banner-actions">
                    <button
                      type="button"
                      onClick={() => {
                        folderPromptShownRef.current = true;
                        setShowFolderConnectPrompt(false);
                      }}
                    >
                      not now
                    </button>
                  </div>
                </div>
              ) : null}

              <div className="overlay-messages" data-testid="overlay-messages">
                {messages.length === 0 && !hasCompanionStreamItems ? (
                  <div className="overlay-empty">No recent messages.</div>
                ) : (
                  messages.map((message) => (
                    <div
                      key={message.id}
                      className={`overlay-message overlay-message-${message.role}${message.message_kind.startsWith("proactive_") ? " overlay-message-proactive" : ""}`}
                    >
                      <div className="overlay-message-role">
                        {message.role === "assistant" ? "jeff" : message.role}
                      </div>
                      <div className="overlay-message-body">{message.content}</div>
                    </div>
                  ))
                )}
                {pendingWriteProposals.map((proposal) => (
                  <div
                    key={proposal.id}
                    className="overlay-message overlay-message-assistant overlay-write-card"
                    data-testid="overlay-file-write-proposal"
                  >
                    <div className="overlay-message-role">jeff</div>
                    <div className="overlay-message-body">
                      <span className="overlay-write-card-kicker">file write approval</span>
                      <strong className="overlay-write-path">
                        {displayProposalPath(proposal.proposed_path)}
                      </strong>
                      <pre className="overlay-write-excerpt">
                        + {proposalExcerpt(proposal.proposed_content)}
                      </pre>
                    </div>
                    <div className="overlay-write-actions">
                      <button
                        type="button"
                        className="overlay-write-approve"
                        onClick={() => void handleApproveWriteProposal(proposal)}
                        data-testid={`overlay-file-write-approve-${proposal.id}`}
                      >
                        approve
                      </button>
                      <button
                        type="button"
                        className="overlay-write-reject"
                        onClick={() => void handleRejectWriteProposal(proposal)}
                        data-testid={`overlay-file-write-reject-${proposal.id}`}
                      >
                        reject
                      </button>
                    </div>
                  </div>
                ))}
                {pendingRevisions.map((revision) => {
                  const alt = alternativeRevisions[revision.revision_id] ?? null;
                  const hasRationale = !!revision.rationale;
                  const altLoading = loadingAlternativeFor === revision.revision_id;
                  return (
                    <div
                      key={revision.revision_id}
                      className="overlay-message overlay-message-assistant overlay-revision-card"
                      data-testid="overlay-revision-proposal"
                    >
                      <div className="overlay-message-role">jeff</div>
                      <div className="overlay-message-body">
                        <span className="overlay-write-card-kicker">revision proposal</span>
                        {hasRationale ? (
                          <p className="overlay-revision-rationale" data-testid="overlay-revision-rationale">
                            {revision.rationale}
                          </p>
                        ) : null}
                        <pre className="overlay-revision-proposed" data-testid="overlay-revision-proposed">
                          {revision.proposed_text.length > 300
                            ? revision.proposed_text.slice(0, 300) + "..."
                            : revision.proposed_text}
                        </pre>
                      </div>
                      <div className="overlay-write-actions">
                        <button
                          type="button"
                          className="overlay-write-approve"
                          onClick={() => void handleApplyRevision(revision)}
                          data-testid={`overlay-revision-apply-${revision.revision_id}`}
                        >
                          apply
                        </button>
                        {hasRationale && !alt ? (
                          <button
                            type="button"
                            className="overlay-revision-alt-btn"
                            onClick={() => void handleLoadAlternative(revision)}
                            disabled={altLoading}
                            data-testid={`overlay-revision-alt-${revision.revision_id}`}
                          >
                            {altLoading ? "..." : "see alternative"}
                          </button>
                        ) : null}
                        <button
                          type="button"
                          className="overlay-write-reject"
                          onClick={() => void handleRejectRevision(revision)}
                          data-testid={`overlay-revision-reject-${revision.revision_id}`}
                        >
                          dismiss
                        </button>
                      </div>
                      {alt ? (
                        <div className="overlay-revision-alt-card" data-testid={`overlay-revision-alt-card-${revision.revision_id}`}>
                          <span className="overlay-write-card-kicker">alternative approach</span>
                          {alt.rationale ? (
                            <p className="overlay-revision-rationale">
                              {alt.rationale}
                            </p>
                          ) : null}
                          <pre className="overlay-revision-proposed">
                            {alt.proposed_text.length > 300
                              ? alt.proposed_text.slice(0, 300) + "..."
                              : alt.proposed_text}
                          </pre>
                          <div className="overlay-write-actions">
                            <button
                              type="button"
                              className="overlay-write-approve"
                              onClick={() => void handleApplyRevision(alt)}
                              data-testid={`overlay-revision-alt-apply-${alt.revision_id}`}
                            >
                              apply alternative
                            </button>
                          </div>
                        </div>
                      ) : null}
                    </div>
                  );
                })}
                {writeConfirmations.map((confirmation) => (
                  <div
                    key={confirmation.id}
                    className="overlay-context-line overlay-write-confirmation"
                    data-testid="overlay-file-written-confirmation"
                  >
                    {confirmation.fileName} written
                  </div>
                ))}
                {streamingTurnId ? (
                  <div className="overlay-message overlay-message-assistant overlay-message-streaming">
                    <div className="overlay-message-role">jeff</div>
                    <div className="overlay-message-body">
                      {streamingText.length > 0 ? streamingText : "thinking..."}
                    </div>
                  </div>
                ) : null}
                {speculativeSubtask ? (
                  <div className="overlay-message overlay-message-assistant" data-testid="overlay-speculative-subtask">
                    <div className="overlay-message-role">jeff</div>
                    <div className="overlay-message-body">
                      {speculativeSubtask.result_summary ? (
                        <p className="overlay-revision-rationale" data-testid="overlay-subtask-assessment">
                          {speculativeSubtask.result_summary}
                        </p>
                      ) : null}
                      I drafted {speculativeSubtask.description || speculativeSubtask.title || "something"} in the background.
                    </div>
                    <div className="overlay-banner-actions overlay-speculative-actions">
                      <button
                        type="button"
                        onClick={() => {
                          void acceptSubtaskResult(speculativeSubtask.subtask_id).catch(() => undefined);
                          setSpeculativeSubtask(null);
                        }}
                      >
                        keep it
                      </button>
                      <button
                        type="button"
                        onClick={() => {
                          void rejectSubtaskResult(speculativeSubtask.subtask_id).catch(() => undefined);
                          setSpeculativeSubtask(null);
                        }}
                      >
                        dismiss
                      </button>
                    </div>
                  </div>
                ) : null}
                <div ref={messagesEndRef} />
              </div>


              {selectionCaptureIndicator ? (
                <div
                  className={`overlay-banner ${
                    selectionCaptureIndicator.status === "failed"
                      ? "overlay-banner-warn"
                      : "overlay-banner-info"
                  }`}
                  data-testid="overlay-selection-capture-indicator"
                >
                  <span className="overlay-selection-capture-message">
                    {selectionCaptureIndicator.message}
                    {selectionCaptureIndicator.document_title ? (
                      <span className="overlay-selection-capture-doc">
                        {" "}— {selectionCaptureIndicator.document_title}
                      </span>
                    ) : null}
                  </span>
                  <div className="overlay-banner-actions">
                    <button
                      type="button"
                      onClick={() => void handleDismissSelectionCapture()}
                      data-testid="overlay-selection-capture-dismiss"
                    >
                      dismiss
                    </button>
                  </div>
                </div>
              ) : null}


              {ttsActivePlaying && !ttsBargeInHintDismissed ? (
                <div className="overlay-context-line" data-testid="overlay-tts-hint">
                  jeff is speaking — type to interrupt
                </div>
              ) : null}

              <div className="overlay-voice-control" data-testid="voice-session-control">
                <button
                  type="button"
                  data-testid="voice-session-toggle"
                  onClick={() => void handleStartVoiceSession()}
                  title="Talk to Jeff (Cmd/Ctrl+Shift+M)"
                >
                  {voiceState === "live" ? "End voice" : "Talk to Jeff"}
                </button>
                {voiceState === "live" ? (
                  <button
                    type="button"
                    data-testid="voice-session-mute"
                    onClick={handleToggleVoiceMute}
                  >
                    {voiceMuted ? "Unmute" : "Mute"}
                  </button>
                ) : null}
                <span className="overlay-voice-state" data-testid="voice-session-state">
                  {voiceState === "idle" ? "" : `voice: ${voiceState}`}
                </span>
              </div>

              <form className="overlay-input-row" onSubmit={handleSubmit}>
                <input
                  ref={messageInputRef}
                  className="overlay-input"
                  data-testid="overlay-input"
                  type="text"
                  placeholder={
                    recording
                      ? "Recording — click mic to send"
                      : activeTask
                        ? selectionCaptureIndicator?.status === "captured"
                          ? "Ask about the captured text"
                          : "Say something to Jeff"
                        : "What are you working on right now?"
                  }
                  value={input}
                  onChange={(event) => {
                    if (ttsActiveTurnIdRef.current !== null) {
                      stopStreamingTtsPlayback();
                      setTtsBargeInHintDismissed(true);
                    }
                    setInput(event.target.value);
                  }}
                  disabled={sending || recording}
                />
                <button
                  type="button"
                  className={`overlay-mic${recording ? " overlay-mic-active" : ""}`}
                  data-testid="overlay-mic-button"
                  title={recording ? "Stop recording and send" : "Voice input"}
                  disabled={sending}
                  onClick={() => recording ? handleStopVoiceRecording() : void handleStartVoiceRecording()}
                >
                  {recording ? "stop" : "mic"}
                </button>
                <button
                  type="submit"
                  className="overlay-send"
                  disabled={sending || recording || input.trim().length === 0}
                >
                  {sending ? "…" : activeTask ? "send" : "start"}
                </button>
              </form>

              {infoNotice ? (
                <div className="overlay-banner overlay-banner-info" data-testid="overlay-info-notice">
                  <span>{infoNotice}</span>
                  <div className="overlay-banner-actions">
                    <button type="button" onClick={() => setInfoNotice(null)}>
                      dismiss
                    </button>
                  </div>
                </div>
              ) : null}

              {errorMessage ? (
                <div className="overlay-error">
                  {errorMessage}
                  {isApiKeyIssue(errorMessage) ? (
                    <button
                      type="button"
                      className="overlay-inline-action"
                      onClick={handleFixApiKey}
                      data-testid="overlay-fix-api-key"
                    >
                      Update API key
                    </button>
                  ) : null}
                </div>
              ) : null}
            </>
          )}
        </div>
      ) : (
        <div className="overlay-collapsed-body">
          <button
            type="button"
            className="overlay-collapsed-summon"
            onClick={toggleMode}
          >
            {activeTask ? activeTask.title : "Tap to start"}
          </button>
          {activeContext && activeContext.document_title ? (
            <span className="overlay-context-hint">
              {activeContext.app_name} &mdash; {activeContext.document_title}
            </span>
          ) : null}
          {hotkeyLabel ? (
            <span className="overlay-hotkey-hint">{hotkeyLabel}</span>
          ) : null}
        </div>
      )}
    </div>
  );
}
