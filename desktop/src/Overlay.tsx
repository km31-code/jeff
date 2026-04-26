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
  reportNotificationClicked,
  setOverlayMode,
  setTrayStatus,
  setQuietMode,
  showWorkspace
} from "./ambientClient";
import {
  ActiveWindowContextDto,
  ApiKeyValidationDto,
  ChatMessageDto,
  OnboardingStatusDto,
  SelectionCaptureIndicatorDto,
  TaskDto,
  WatcherStatusDto,
  cancelStreamingTurn,
  clearPreferredWorkspaceFolder,
  completeOnboarding,
  createTask,
  dismissSelectionCapture,
  getActiveTask,
  getActiveWindowContext,
  getAccessibilityPermissionStatus,
  getOnboardingStatus,
  getSelectionCaptureIndicator,
  getWatcherStatus,
  listTasks,
  listMessages,
  recordTaskFocus,
  requestAccessibilityPermission,
  sendMessage,
  sendMessageStreaming,
  setActiveTask,
  setPreferredWorkspaceFolder,
  storeOpenAiApiKey,
  transcribeAudio,
  triggerTaskResume,
  validateOpenAiApiKey
} from "./tauriClient";

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

type PendingNotificationContext = { kind: string | null; id: number | null };
type OnboardingStep = 1 | 2 | 3 | 4 | 5;
type OverlayShownPayload = { interactive?: boolean };

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

export default function Overlay(): JSX.Element {
  const [ambient, setAmbient] = useState<AmbientStateDto | null>(null);
  const [mode, setMode] = useState<OverlayMode>("collapsed");
  const [activeTask, setActiveTaskState] = useState<TaskDto | null>(null);
  const [messages, setMessages] = useState<ChatMessageDto[]>([]);
  const [input, setInput] = useState("");
  const [sending, setSending] = useState(false);
  const [recording, setRecording] = useState(false);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);

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
  const [notificationContext, setNotificationContext] =
    useState<PendingNotificationContext | null>(null);
  const [streamingTurnId, setStreamingTurnId] = useState<string | null>(null);
  const [streamingText, setStreamingText] = useState("");

  // phase 20: active window context driven by backend context://context-updated events.
  const [activeContext, setActiveContext] = useState<ActiveWindowContextDto | null>(null);
  const [docSwitchBanner, setDocSwitchBanner] = useState<{ app_name: string; document_title: string } | null>(null);
  const [tasks, setTasks] = useState<TaskDto[]>([]);
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

  // phase 15: reorientation banner shown when user returns to a task after 5+
  // minutes away. auto-dismisses after 8 seconds.
  const [reorientationBanner, setReorientationBanner] = useState<string | null>(null);
  const reorientationTimerRef = useRef<number | null>(null);

  const [onboardingStatus, setOnboardingStatus] =
    useState<OnboardingStatusDto | null>(null);
  const [onboardingVisible, setOnboardingVisible] = useState(false);
  const [onboardingStep, setOnboardingStep] = useState<OnboardingStep>(1);
  const [onboardingBusy, setOnboardingBusy] = useState(false);
  const [apiKeyInput, setApiKeyInput] = useState("");
  const [apiKeyValidation, setApiKeyValidation] =
    useState<ApiKeyValidationDto | null>(null);
  const [workspaceFolder, setWorkspaceFolder] = useState<string | null>(null);

  const activeTaskRef = useRef<TaskDto | null>(null);
  const docSwitchTaskCandidates = useMemo(
    () => tasks.filter((task) => !activeTask || task.id !== activeTask.id).slice(0, 3),
    [activeTask, tasks]
  );
  const streamingTurnIdRef = useRef<string | null>(null);
  const pendingExpandRef = useRef(false);
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
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages, streamingTurnId, streamingText]);

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
      // overlay shows only the tail of the conversation.
      setMessages(list.slice(-6));
      if (status) setWatcherStatus(status);
    } catch (error) {
      setErrorMessage(String(error));
    }
  }, []);

  const refreshActiveTask = useCallback(async () => {
    try {
      const task = await getActiveTask();
      setActiveTaskState(task);
      setTasks(await listTasks().catch(() => []));
      if (task) {
        await refreshMessages(task.id);
      } else {
        setMessages([]);
      }
    } catch (error) {
      setErrorMessage(String(error));
    }
  }, [refreshMessages]);

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
          setNotificationContext({
            kind: event.payload.context_kind ?? null,
            id: event.payload.context_id ?? null
          });
          pendingExpandRef.current = true;
          setMode("expanded");
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

        // phase 15: when the user explicitly summons the overlay, check for
        // reorientation (5+ min absence) and record the focus timestamp.
        // mirrors what App.tsx does on main-window focus for overlay-first users.
        if (event.payload?.interactive) {
          const task = activeTaskRef.current;
          if (task) {
            void triggerTaskResume(task.id)
              .then((result) => {
                if (result.summary && result.summary.trim().length > 0) {
                  if (reorientationTimerRef.current !== null) {
                    window.clearTimeout(reorientationTimerRef.current);
                  }
                  setReorientationBanner(result.summary.trim());
                  reorientationTimerRef.current = window.setTimeout(() => {
                    setReorientationBanner(null);
                    reorientationTimerRef.current = null;
                  }, 8000);
                }
              })
              .catch(() => undefined);
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
  }, [openOnboardingWizard, refreshActiveTask, refreshOnboarding, schedulePrimaryInteractionFocus]);

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

  // if a notification click deep-linked us while hidden, the backend has
  // already set mode=expanded. make sure it is applied locally too.
  useEffect(() => {
    if (pendingExpandRef.current) {
      pendingExpandRef.current = false;
      setOverlayMode("expanded").catch(() => undefined);
    }
  }, [notificationContext]);

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

  const handleOpenWorkspace = useCallback(async () => {
    try {
      await showWorkspace();
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
        setDocSwitchBanner(null);
      } catch (error) {
        setErrorMessage(String(error));
      }
    },
    [refreshMessages]
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
      setDocSwitchBanner(null);
    } catch (error) {
      setErrorMessage(String(error));
    }
  }, [refreshMessages]);

  const handleSubmit = useCallback(
    async (event: FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      const trimmed = input.trim();
      if (!trimmed || sending) return;

      setSending(true);
      setErrorMessage(null);
      let streamingStarted = false;

      try {
        let task = activeTask;
        if (!task) {
          const created = await createTask(deriveTaskTitleFromPrompt(trimmed));
          task = await setActiveTask(created.id).catch(() => created);
          setActiveTaskState(task);
          activeTaskRef.current = task;
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
          return;
        }

        await sendMessage(task.id, trimmed, "text");
        setInput("");
        await refreshMessages(task.id);
      } catch (error) {
        setErrorMessage(String(error));
      } finally {
        if (!streamingStarted) {
          await setTrayStatus("idle").catch(() => undefined);
          setSending(false);
        }
      }
    },
    [activeTask, input, refreshMessages, sending]
  );

  const ackNotificationContext = useCallback(async () => {
    if (!notificationContext) return;
    try {
      await reportNotificationClicked(
        notificationContext.kind,
        notificationContext.id
      );
    } catch {
      // ignore — event already fired locally.
    }
    setNotificationContext(null);
  }, [notificationContext]);

  const handleOnboardingCancel = useCallback(() => {
    onboardingSnoozedRef.current = true;
    setOnboardingVisible(false);
    setApiKeyValidation(null);
  }, []);

  const handleOnboardingStepOneContinue = useCallback(() => {
    setOnboardingStep(2);
  }, []);

  const handleOnboardingValidateApiKey = useCallback(async () => {
    const trimmed = apiKeyInput.trim();

    if (!trimmed && onboardingStatus?.has_stored_api_key) {
      setApiKeyValidation({
        is_valid: true,
        message: "Using existing stored API key."
      });
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
  }, [apiKeyInput, onboardingStatus?.has_stored_api_key, refreshOnboarding]);

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

    audio.onended = () => {
      URL.revokeObjectURL(url);
      streamTtsCurrentRef.current = null;
      if (streamTtsQueueRef.current.size === 0) {
        ttsActiveTurnIdRef.current = null;
        void setTrayStatus("idle").catch(() => undefined);
      }
      scheduleStreamTtsPlayback();
    };
    audio.onerror = () => {
      URL.revokeObjectURL(url);
      streamTtsCurrentRef.current = null;
      if (streamTtsQueueRef.current.size === 0) {
        ttsActiveTurnIdRef.current = null;
        void setTrayStatus("idle").catch(() => undefined);
      }
      scheduleStreamTtsPlayback();
    };

    void audio.play().catch(() => {
      streamTtsCurrentRef.current = null;
      if (streamTtsQueueRef.current.size === 0) {
        ttsActiveTurnIdRef.current = null;
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
  }

  // barge-in: stop tts and cancel any in-flight streaming turn.
  async function stopAndBargeIn() {
    stopStreamingTtsPlayback();
    stopPartialStt();
    const activeTurnId = streamingTurnIdRef.current;
    if (activeTurnId) {
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
    let streamingStarted = false;
    try {
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
      } else {
        await sendMessage(taskId, text, "voice");
        await refreshMessages(taskId);
      }
    } catch (error) {
      setErrorMessage(String(error));
      await setTrayStatus("idle").catch(() => undefined);
    } finally {
      if (!streamingStarted) setSending(false);
    }
  }, [refreshMessages]);

  const handleStartVoiceRecording = useCallback(async () => {
    if (recording || sending) return;
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
  }, [recording, sending]);

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
  }, [refreshMessages, submitVoiceMessage]);

  const handleDismissSelectionCapture = useCallback(async () => {
    try {
      await dismissSelectionCapture();
      setSelectionCaptureIndicator(null);
    } catch {
      setSelectionCaptureIndicator(null);
    }
  }, []);

  const hotkeyLabel = useMemo(
    () => (ambient ? formatHotkey(ambient.hotkey) : ""),
    [ambient]
  );

  const statusLabel = ambient ? describeStatus(ambient.tray_status) : "idle";

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
          <span className="overlay-status-label">Jeff · {statusLabel}</span>
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
            <div className="overlay-task-label">
              {activeTask ? activeTask.title : "No active task"}
            </div>
            <button
              type="button"
              className="overlay-workspace-link"
              onClick={handleOpenWorkspace}
            >
              open full workspace
            </button>
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

          {notificationContext ? (
            <div className="overlay-banner overlay-banner-info">
              <span>
                opened from notification
                {notificationContext.kind
                  ? ` · ${notificationContext.kind}`
                  : ""}
                {notificationContext.id !== null
                  ? ` #${notificationContext.id}`
                  : ""}
              </span>
              <button type="button" onClick={ackNotificationContext}>
                ok
              </button>
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

              <div className="overlay-messages" data-testid="overlay-messages">
                {messages.length === 0 ? (
                  <div className="overlay-empty">No recent messages.</div>
                ) : (
                  messages.map((message) => (
                    <div
                      key={message.id}
                      className={`overlay-message overlay-message-${message.role}`}
                    >
                      <div className="overlay-message-role">{message.role}</div>
                      <div className="overlay-message-body">{message.content}</div>
                    </div>
                  ))
                )}
                {streamingTurnId ? (
                  <div className="overlay-message overlay-message-assistant">
                    <div className="overlay-message-role">assistant</div>
                    <div className="overlay-message-body">
                      {streamingText.length > 0 ? streamingText : "thinking..."}
                    </div>
                  </div>
                ) : null}
                <div ref={messagesEndRef} />
              </div>

              {reorientationBanner ? (
                <div className="overlay-banner overlay-banner-info" data-testid="overlay-reorientation-banner">
                  <span className="overlay-selection-capture-message">{reorientationBanner}</span>
                  <div className="overlay-banner-actions">
                    <button
                      type="button"
                      onClick={() => {
                        if (reorientationTimerRef.current !== null) {
                          window.clearTimeout(reorientationTimerRef.current);
                          reorientationTimerRef.current = null;
                        }
                        setReorientationBanner(null);
                      }}
                    >
                      dismiss
                    </button>
                  </div>
                </div>
              ) : null}

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
                        : "Tell me what you're working on"
                  }
                  value={input}
                  onChange={(event) => setInput(event.target.value)}
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
