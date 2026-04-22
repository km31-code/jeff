import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import {
  EVENT_LLM_COMPLETE,
  EVENT_LLM_TOKEN,
  EVENT_TURN_CANCELLED,
  EVENT_TURN_COMPLETE,
  LlmCompletePayload,
  LlmTokenPayload,
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
  cancelStreamingTurn,
  ChatMessageDto,
  TaskDto,
  getActiveTask,
  listMessages,
  sendMessage,
  sendMessageStreaming
} from "./tauriClient";

// phase 11 overlay: ambient presence window. collapsed is a compact status
// bar, expanded shows the last few messages and a send box. this is not the
// full workspace — it is the always-there companion surface.

type PendingNotificationContext = { kind: string | null; id: number | null };

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

export default function Overlay(): JSX.Element {
  const [ambient, setAmbient] = useState<AmbientStateDto | null>(null);
  const [mode, setMode] = useState<OverlayMode>("collapsed");
  const [activeTask, setActiveTaskState] = useState<TaskDto | null>(null);
  const [messages, setMessages] = useState<ChatMessageDto[]>([]);
  const [input, setInput] = useState("");
  const [sending, setSending] = useState(false);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [hotkeyConflict, setHotkeyConflict] = useState<string | null>(null);
  const [notificationContext, setNotificationContext] =
    useState<PendingNotificationContext | null>(null);
  const [streamingTurnId, setStreamingTurnId] = useState<string | null>(null);
  const [streamingText, setStreamingText] = useState("");
  const activeTaskRef = useRef<TaskDto | null>(null);
  const streamingTurnIdRef = useRef<string | null>(null);
  const pendingExpandRef = useRef(false);

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
      const list = await listMessages(taskId);
      // overlay shows only the tail of the conversation.
      setMessages(list.slice(-6));
    } catch (error) {
      setErrorMessage(String(error));
    }
  }, []);

  const refreshActiveTask = useCallback(async () => {
    try {
      const task = await getActiveTask();
      setActiveTaskState(task);
      if (task) {
        await refreshMessages(task.id);
      } else {
        setMessages([]);
      }
    } catch (error) {
      setErrorMessage(String(error));
    }
  }, [refreshMessages]);

  // initial load + notification permission probing.
  useEffect(() => {
    refreshAmbient();
    refreshActiveTask();
    probeNotificationPermission();
  }, [refreshAmbient, refreshActiveTask]);

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
      listen("ambient://overlay-shown", () => {
        refreshActiveTask();
      })
    );

    return () => {
      unsubscribers.forEach((p) =>
        p.then((unlisten) => unlisten()).catch(() => undefined)
      );
    };
  }, [refreshActiveTask]);

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

    const finalizeStreamingTurn = async () => {
      streamingTurnIdRef.current = null;
      setStreamingTurnId(null);
      setStreamingText("");
      setSending(false);
      await setTrayStatus("idle").catch(() => undefined);
      const active = activeTaskRef.current;
      if (active) {
        await refreshMessages(active.id);
      }
    };

    unsubscribers.push(
      listen<LlmCompletePayload>(EVENT_LLM_COMPLETE, (event) => {
        if (streamingTurnIdRef.current !== event.payload.turn_id) return;
        void finalizeStreamingTurn();
      })
    );

    unsubscribers.push(
      listen<TurnCancelledPayload>(EVENT_TURN_CANCELLED, (event) => {
        if (streamingTurnIdRef.current !== event.payload.turn_id) return;
        void finalizeStreamingTurn();
      })
    );

    unsubscribers.push(
      listen<TurnCompletePayload>(EVENT_TURN_COMPLETE, (event) => {
        if (streamingTurnIdRef.current !== event.payload.turn_id) return;
        void finalizeStreamingTurn();
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

  const handleSubmit = useCallback(
    async (event: FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      const trimmed = input.trim();
      if (!trimmed || sending) return;
      if (!activeTask) {
        setErrorMessage("No active task. Open the full workspace to pick one.");
        return;
      }

      setSending(true);
      setErrorMessage(null);
      await setTrayStatus("working").catch(() => undefined);

      if (isStreamingEnabled()) {
        try {
          if (streamingTurnIdRef.current) {
            await cancelStreamingTurn(streamingTurnIdRef.current, "user_barge_in").catch(
              () => undefined
            );
          }
          const turnId = await sendMessageStreaming(activeTask.id, trimmed, "text");
          streamingTurnIdRef.current = turnId;
          setStreamingTurnId(turnId);
          setStreamingText("");
          setInput("");
          return;
        } catch (error) {
          setErrorMessage(String(error));
          await setTrayStatus("idle").catch(() => undefined);
          setSending(false);
          return;
        }
      }

      try {
        await sendMessage(activeTask.id, trimmed, "text");
        setInput("");
        await refreshMessages(activeTask.id);
      } catch (error) {
        setErrorMessage(String(error));
      } finally {
        await setTrayStatus("idle").catch(() => undefined);
        setSending(false);
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
          </div>

          <form className="overlay-input-row" onSubmit={handleSubmit}>
            <input
              className="overlay-input"
              data-testid="overlay-input"
              type="text"
              placeholder="Say something to Jeff"
              value={input}
              onChange={(event) => setInput(event.target.value)}
              disabled={sending}
            />
            <button
              type="submit"
              className="overlay-send"
              disabled={sending || input.trim().length === 0}
            >
              {sending ? "…" : "send"}
            </button>
          </form>

          {errorMessage ? (
            <div className="overlay-error">{errorMessage}</div>
          ) : null}
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
          {hotkeyLabel ? (
            <span className="overlay-hotkey-hint">{hotkeyLabel}</span>
          ) : null}
        </div>
      )}
    </div>
  );
}
