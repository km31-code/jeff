import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "./App";

type TaskDto = {
  id: number;
  title: string;
  slug: string;
  workspace_path: string;
  created_at: string;
  updated_at: string;
  is_active: boolean;
};

type ChatMessageDto = {
  id: number;
  task_id: number;
  session_id: number | null;
  role: string;
  message_source: string;
  message_kind: string;
  content: string;
  created_at: string;
};

type RevisionDto = {
  revision_id: number;
  task_id: number;
  artifact_id: number;
  target_start_offset: number;
  target_end_offset: number;
  target_description: string;
  original_text: string;
  proposed_text: string;
  instruction_text: string;
  instruction_source: string;
  rationale: string | null;
  grounding_notes: string | null;
  retrieval_confidence: number;
  status: string;
  created_at: string;
  updated_at: string;
};

type ArtifactVersionDto = {
  version_id: number;
  task_id: number;
  artifact_id: number;
  revision_id: number | null;
  version_reason: string;
  content_preview: string;
  content_length: number;
  created_at: string;
};

type SubTaskDto = {
  subtask_id: number;
  task_id: number;
  title: string;
  description: string;
  execution_type: string;
  status: string;
  result_review_status: string;
  created_at: string;
  updated_at: string;
  result_summary: string | null;
  result_payload: string | null;
  instruction_source: string;
  parent_context_snapshot: string;
  error_message: string | null;
};

type SessionModeStateDto = {
  task_id: number;
  current_mode: string;
  mode_reason: string;
  waiting_on_user_decision: boolean;
  last_engine_decision: string;
  active_artifact_id: number | null;
  updated_at: string;
};

type SuggestionDto = {
  suggestion_id: number;
  task_id: number;
  title: string;
  description: string;
  suggestion_type: string;
  source_reason: string;
  status: string;
  suggestion_key: string;
  linked_context: string | null;
  linked_subtask_type: string | null;
  linked_revision_intent: string | null;
  created_at: string;
  updated_at: string;
};

type EventLogEntryDto = {
  id: number;
  task_id: number;
  event_type: string;
  payload_json: string;
  created_at: string;
};

type PrivacyCenterDashboardDto = {
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
  local_runtime: LocalRuntimeStatusDto;
};

type LocalRuntimeStatusDto = {
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
  deterministic_fallback_enabled: boolean;
  last_error: string | null;
  disk_available_bytes: number | null;
  installed_model_bytes: number;
};

type RelationalProfileDto = {
  stated_goals: Array<{
    id: number;
    task_id: number;
    goal_text: string;
    stated_at: string;
    status: "active" | "completed" | "abandoned";
    updated_at: string;
  }>;
  struggle_patterns: Array<{
    id: number;
    pattern_text: string;
    task_ids_json: string;
    first_seen: string;
    last_seen: string;
    occurrence_count: number;
  }>;
  collaboration_style: {
    prefers_opinions: number;
    wants_explanations: number;
    delegation_comfort: number;
    interruption_tolerance: number;
  };
  trust_metrics: {
    times_accepted_opinion: number;
    times_pushed_back: number;
    times_asked_for_more: number;
  };
};

type StreamTrackLike = {
  stop: ReturnType<typeof vi.fn>;
};

type StreamLike = {
  getTracks: () => StreamTrackLike[];
};

const invokeMock = vi.fn();
let streamingEnabled = false;
const eventHandlers = new Map<string, Set<(event: { payload: unknown }) => void>>();

function emitTauriEvent(eventName: string, payload: unknown) {
  const handlers = eventHandlers.get(eventName);
  if (!handlers) return;
  for (const handler of handlers) {
    handler({ payload });
  }
}

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args)
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: (eventName: string, handler: (event: { payload: unknown }) => void) => {
    const handlers = eventHandlers.get(eventName) ?? new Set();
    handlers.add(handler);
    eventHandlers.set(eventName, handlers);
    return Promise.resolve(() => {
      const current = eventHandlers.get(eventName);
      if (!current) return;
      current.delete(handler);
      if (current.size === 0) {
        eventHandlers.delete(eventName);
      }
    });
  }
}));

vi.mock("./streamClient", () => ({
  isStreamingEnabled: () => streamingEnabled,
  EVENT_LLM_TOKEN: "stream://llm_token",
  EVENT_LLM_COMPLETE: "stream://llm_complete",
  EVENT_TTS_CHUNK: "stream://tts_chunk",
  EVENT_TURN_CANCELLED: "stream://turn_cancelled",
  EVENT_TURN_COMPLETE: "stream://turn_complete",
}));

afterEach(() => {
  cleanup();
  invokeMock.mockReset();
  streamingEnabled = false;
  eventHandlers.clear();
  window.localStorage.removeItem("jeff_show_debug_panels");
});

function setupInvokeMock(options?: {
  failCommands?: Record<string, string>;
  accessibilityGranted?: boolean;
}) {
  const now = "2026-04-19T00:00:00Z";

  const tasks: TaskDto[] = [
    {
      id: 1,
      title: "history storymap",
      slug: "history-storymap",
      workspace_path: "/tmp/jeff_data/tasks/history-storymap",
      created_at: now,
      updated_at: now,
      is_active: true
    }
  ];

  const artifacts = [
    {
      id: 10,
      task_id: 1,
      file_name: "notes.md",
      file_extension: "md",
      original_path: "/tmp/notes.md",
      stored_path: "/tmp/jeff_data/tasks/history-storymap/artifacts/notes.md",
      created_at: now,
      updated_at: now,
      chunk_count: 4
    }
  ];

  let artifactContent =
    "The intro summarizes events.\n\nThe thesis is broad and needs clearer citizenship framing.\n\nEvidence should tie to course readings and primary sources.";
  const initialArtifactContent = artifactContent;

  const messages: ChatMessageDto[] = [
    {
      id: 1,
      task_id: 1,
      session_id: 1,
      role: "assistant",
      message_source: "assistant",
      message_kind: "assistant_nudge",
      content: "Your intro still needs one primary source tied to course readings.",
      created_at: now
    }
  ];

  let nextMessageId = 2;
  let nextRevisionId = 1;
  let nextVersionId = 1;
  let proactiveMode = true;
  let nextSubtaskId = 1;
  let persistedActiveArtifactId: number | null = 10;
  let nextEventId = 1;
  let nextStreamingTurnId = 1;
  let activeStreamingTurnId: string | null = null;
  let activeStreamingPlaceholderId: number | null = null;
  let watcherStatus = {
    task_id: 1,
    is_watching: false,
    watched_path: null as string | null
  };
  let onboardingStatus: {
    onboarding_complete: boolean;
    has_stored_api_key: boolean;
    api_key_source: string;
    preferred_workspace_folder: string | null;
  } = {
    onboarding_complete: true,
    has_stored_api_key: true,
    api_key_source: "keychain",
    preferred_workspace_folder: "/tmp/jeff_data/tasks/history-storymap"
  };
  let workspacePromptDismissed = false;
  let clipboardCaptureEnabled = false;
  const localRuntimeStatus: LocalRuntimeStatusDto = {
    enabled: true,
    healthy: false,
    running: false,
    mode: "deterministic_local",
    sidecar_configured: false,
    sidecar_pid: null,
    endpoint: "http://127.0.0.1:17631",
    model_dir: "/tmp/jeff_data/models",
    reasoning_model_id: "local-reflex-llamacpp",
    reasoning_model_path: "/tmp/jeff_data/models/reflex-instruct.gguf",
    reasoning_model_present: false,
    embedding_model_id: "local-hash-embedding-v1",
    embedding_model_path: "/tmp/jeff_data/models/embedding.gguf",
    embedding_model_present: false,
    deterministic_fallback_enabled: true,
    last_error: null,
    disk_available_bytes: 1_000_000_000,
    installed_model_bytes: 0
  };
  let privacyDashboard: PrivacyCenterDashboardDto = {
    active_task_id: 1,
    active_task_title: "history storymap",
    workspace_watcher_enabled: true,
    workspace_folder_path: "/tmp/jeff_data/tasks/history-storymap",
    workspace_watched_file_count: 0,
    workspace_watcher_running: false,
    clipboard_capture_enabled: false,
    clipboard_capture_reminder: "Clipboard capture is off by default.",
    active_window_context_enabled: true,
    accessibility_permission_status: options?.accessibilityGranted === false ? "not granted" : "granted",
    proactive_triggers_enabled: true,
    user_profile_memory_enabled: false,
    user_profile_signal_count: 0,
    calendar_context_enabled: false,
    calendar_permission_status: "not requested",
    selection_capture_enabled: true,
    typing_activity_enabled: true,
    tts_voice: "alloy",
    available_tts_voices: ["alloy", "nova", "shimmer"],
    local_runtime: localRuntimeStatus
  };
  let relationalProfile: RelationalProfileDto = {
    stated_goals: [],
    struggle_patterns: [],
    collaboration_style: {
      prefers_opinions: 0.5,
      wants_explanations: 0.5,
      delegation_comfort: 0.5,
      interruption_tolerance: 0.5
    },
    trust_metrics: {
      times_accepted_opinion: 0,
      times_pushed_back: 0,
      times_asked_for_more: 0
    }
  };
  const recentlyLearned: Array<{
    id: number;
    task_id: number;
    source: "file" | "clipboard";
    display_label: string;
    preview_text: string;
    ingested_at: string;
  }> = [];

  const pendingRevisions: RevisionDto[] = [];
  const versions: ArtifactVersionDto[] = [];
  const subtasks: SubTaskDto[] = [];
  const events: EventLogEntryDto[] = [];
  const subtaskPollCountdown = new Map<number, number>();
  const completedAnnounced = new Set<number>();
  let nextSuggestionId = 3;
  const suggestions: SuggestionDto[] = [
    {
      suggestion_id: 1,
      task_id: 1,
      title: "Tighten intro argument",
      description: "Propose a focused revision to tighten the thesis and tie it to evidence requirements.",
      suggestion_type: "propose_revision",
      source_reason: "Intro drafting appears broad relative to rubric evidence requirements.",
      status: "pending",
      suggestion_key: "writing-tighten-argument",
      linked_context: JSON.stringify({ active_artifact_id: 10 }),
      linked_subtask_type: null,
      linked_revision_intent: "tighten thesis and tie to primary source evidence",
      created_at: now,
      updated_at: now
    },
    {
      suggestion_id: 2,
      task_id: 1,
      title: "Draft stronger intro in parallel",
      description: "Run a bounded intro drafting subtask while you keep editing.",
      suggestion_type: "propose_subtask",
      source_reason: "Writing mode with strong retrieval support allows bounded parallel drafting.",
      status: "pending",
      suggestion_key: "writing-propose-subtask-intro",
      linked_context: JSON.stringify({ active_artifact_id: 10 }),
      linked_subtask_type: "draft_generation",
      linked_revision_intent: null,
      created_at: now,
      updated_at: now
    }
  ];
  const dismissedSuggestionKeys = new Set<string>();

  let sessionModeState: SessionModeStateDto = {
    task_id: 1,
    current_mode: "writing",
    mode_reason: "active artifact and drafting activity detected",
    waiting_on_user_decision: true,
    last_engine_decision: "generated_suggestions",
    active_artifact_id: 10,
    updated_at: now
  };

  function appendEvent(eventType: string, payload: Record<string, unknown>) {
    events.push({
      id: nextEventId,
      task_id: 1,
      event_type: eventType,
      payload_json: JSON.stringify(payload),
      created_at: now
    });
    nextEventId += 1;
  }

  appendEvent("task_created", { task_id: 1 });
  appendEvent("task_activated", { task_id: 1 });
  appendEvent("artifact_selected", { artifact_id: 10 });

  function finalizeStreamingTurn(content: string) {
    if (activeStreamingPlaceholderId === null) return;
    const placeholder = messages.find((item) => item.id === activeStreamingPlaceholderId);
    if (!placeholder) return;
    placeholder.message_kind = "assistant_answer";
    placeholder.content = content;
    appendEvent("message_stream_completed", { turn_id: activeStreamingTurnId });
    activeStreamingTurnId = null;
    activeStreamingPlaceholderId = null;
  }

  function cancelStreamingTurn(partial: string) {
    if (activeStreamingPlaceholderId === null) return;
    const placeholder = messages.find((item) => item.id === activeStreamingPlaceholderId);
    if (!placeholder) return;
    placeholder.message_kind = "assistant_interrupted";
    placeholder.content = partial.trim().length > 0 ? partial : "(interrupted)";
    appendEvent("message_stream_cancelled", { turn_id: activeStreamingTurnId });
    activeStreamingTurnId = null;
    activeStreamingPlaceholderId = null;
  }

  function buildSnapshot(instruction: string, executionType: string) {
    return JSON.stringify({
      task_summary: "Summary placeholder for 'history storymap'.",
      instruction,
      execution_type: executionType,
      recent_messages: messages.slice(-3).map((message) => `${message.role}: ${message.content}`),
      retrieved_chunks: [
        {
          chunk_id: 201,
          task_id: 1,
          artifact_id: 10,
          artifact_file_name: "rubric.txt",
          artifact_stored_path: "/tmp/jeff_data/tasks/history-storymap/artifacts/rubric.txt",
          chunk_text: "Use primary source evidence and citizenship framing tied to course readings.",
          position_index: 1,
          similarity_score: 0.9
        }
      ]
    });
  }

  function announceSubtaskMessage(content: string) {
    messages.push({
      id: nextMessageId,
      task_id: 1,
      session_id: 1,
      role: "assistant",
      message_source: "assistant",
      message_kind: "system_status_event",
      content,
      created_at: now
    });
    nextMessageId += 1;
    appendEvent("subtask_event", { content });
  }

  function advanceSubtasks() {
    for (const subtask of subtasks) {
      if (subtask.status === "pending") {
        subtask.status = "running";
        subtask.updated_at = now;
        continue;
      }

      if (subtask.status !== "running") {
        continue;
      }

      const remaining = subtaskPollCountdown.get(subtask.subtask_id) ?? 0;
      if (remaining > 0) {
        subtaskPollCountdown.set(subtask.subtask_id, remaining - 1);
        continue;
      }

      subtask.status = "completed";
      subtask.result_summary = `Completed ${subtask.execution_type} subtask`;
      subtask.result_payload =
        "This intro now frames the argument through citizenship debates and ties claims to course readings and primary sources.";
      subtask.updated_at = now;
      if (!completedAnnounced.has(subtask.subtask_id)) {
        completedAnnounced.add(subtask.subtask_id);
        announceSubtaskMessage(`Subtask #${subtask.subtask_id} completed: ${subtask.title}`);
      }
    }
  }

  function listPendingSuggestions(): SuggestionDto[] {
    return suggestions
      .filter((item) => item.status === "pending")
      .slice()
      .sort((left, right) => right.suggestion_id - left.suggestion_id);
  }

  function maybeGenerateSuggestions(activeArtifactId: number | null) {
    if (listPendingSuggestions().length > 0 || activeArtifactId === null) {
      return;
    }

    if (!dismissedSuggestionKeys.has("writing-tighten-argument")) {
      suggestions.push({
        suggestion_id: nextSuggestionId,
        task_id: 1,
        title: "Tighten intro argument",
        description: "Propose a focused revision to tighten the thesis and tie it to evidence requirements.",
        suggestion_type: "propose_revision",
        source_reason: "Intro drafting appears broad relative to rubric evidence requirements.",
        status: "pending",
        suggestion_key: "writing-tighten-argument",
        linked_context: JSON.stringify({ active_artifact_id: activeArtifactId }),
        linked_subtask_type: null,
        linked_revision_intent: "tighten thesis and tie to primary source evidence",
        created_at: now,
        updated_at: now
      });
      nextSuggestionId += 1;
    }
  }

  invokeMock.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
    const failureMessage = options?.failCommands?.[command];
    if (failureMessage) {
      throw new Error(failureMessage);
    }

    if (command === "list_tasks") {
      return [...tasks];
    }

    if (command === "get_active_task") {
      return tasks.find((task) => task.is_active) ?? null;
    }

    if (command === "get_active_window_context") {
      return null;
    }

    if (command === "get_accessibility_permission_status") {
      return options?.accessibilityGranted ?? true;
    }

    if (command === "request_accessibility_permission") {
      return null;
    }

    if (command === "get_privacy_center_dashboard") {
      return { ...privacyDashboard };
    }

    if (command === "get_local_runtime_status") {
      return { ...privacyDashboard.local_runtime };
    }

    if (command === "start_local_runtime") {
      privacyDashboard = {
        ...privacyDashboard,
        local_runtime: {
          ...privacyDashboard.local_runtime,
          running: true,
          healthy: true,
          mode: "sidecar"
        }
      };
      return { ...privacyDashboard.local_runtime };
    }

    if (command === "stop_local_runtime") {
      privacyDashboard = {
        ...privacyDashboard,
        local_runtime: {
          ...privacyDashboard.local_runtime,
          running: false,
          healthy: false,
          mode: "deterministic_local"
        }
      };
      return { ...privacyDashboard.local_runtime };
    }

    if (command === "delete_local_model") {
      privacyDashboard = {
        ...privacyDashboard,
        local_runtime: {
          ...privacyDashboard.local_runtime,
          reasoning_model_present: args?.kind === "reasoning" ? false : privacyDashboard.local_runtime.reasoning_model_present,
          embedding_model_present: args?.kind === "embedding" ? false : privacyDashboard.local_runtime.embedding_model_present
        }
      };
      return { ...privacyDashboard.local_runtime };
    }

    if (command === "download_local_model") {
      privacyDashboard = {
        ...privacyDashboard,
        local_runtime: {
          ...privacyDashboard.local_runtime,
          reasoning_model_present: args?.kind === "reasoning" ? true : privacyDashboard.local_runtime.reasoning_model_present,
          embedding_model_present: args?.kind === "embedding" ? true : privacyDashboard.local_runtime.embedding_model_present,
          installed_model_bytes: 42
        }
      };
      return { ...privacyDashboard.local_runtime };
    }

    if (command === "get_relational_profile") {
      return {
        ...relationalProfile,
        stated_goals: [...relationalProfile.stated_goals],
        struggle_patterns: [...relationalProfile.struggle_patterns],
        collaboration_style: { ...relationalProfile.collaboration_style },
        trust_metrics: { ...relationalProfile.trust_metrics }
      };
    }

    if (command === "get_selection_capture_indicator") {
      return null;
    }

    if (command === "dismiss_selection_capture") {
      return null;
    }

    if (command === "get_selection_bridge_status") {
      return {
        enabled: true,
        port: 47832,
        token: "test-selection-token"
      };
    }

    if (command === "set_tts_voice") {
      privacyDashboard = {
        ...privacyDashboard,
        tts_voice: String(args?.voice ?? "alloy")
      };
      return { ...privacyDashboard };
    }

    if (command === "set_privacy_surface_enabled") {
      const surface = String(args?.surface ?? "");
      const enabled = Boolean(args?.enabled);
      if (surface === "workspace_watcher") {
        privacyDashboard = {
          ...privacyDashboard,
          workspace_watcher_enabled: enabled,
          workspace_watcher_running: enabled
        };
      }
      if (surface === "clipboard_capture") {
        clipboardCaptureEnabled = enabled;
        privacyDashboard = {
          ...privacyDashboard,
          clipboard_capture_enabled: enabled
        };
      }
      if (surface === "active_window_context") {
        privacyDashboard = {
          ...privacyDashboard,
          active_window_context_enabled: enabled
        };
      }
      if (surface === "proactive_triggers") {
        proactiveMode = enabled;
        privacyDashboard = {
          ...privacyDashboard,
          proactive_triggers_enabled: enabled
        };
      }
      if (surface === "user_profile_memory") {
        privacyDashboard = {
          ...privacyDashboard,
          user_profile_memory_enabled: enabled
        };
      }
      if (surface === "calendar_context") {
        privacyDashboard = {
          ...privacyDashboard,
          calendar_context_enabled: enabled
        };
      }
      if (surface === "selection_capture") {
        privacyDashboard = {
          ...privacyDashboard,
          selection_capture_enabled: enabled
        };
      }
      if (surface === "typing_activity") {
        privacyDashboard = {
          ...privacyDashboard,
          typing_activity_enabled: enabled
        };
      }
      return { ...privacyDashboard };
    }

    if (command === "clear_user_profile_memory") {
      relationalProfile = {
        ...relationalProfile,
        stated_goals: [],
        struggle_patterns: [],
        collaboration_style: {
          prefers_opinions: 0.5,
          wants_explanations: 0.5,
          delegation_comfort: 0.5,
          interruption_tolerance: 0.5
        },
        trust_metrics: {
          times_accepted_opinion: 0,
          times_pushed_back: 0,
          times_asked_for_more: 0
        }
      };
      privacyDashboard = {
        ...privacyDashboard,
        user_profile_signal_count: 0
      };
      return { ...privacyDashboard };
    }

    if (command === "delete_stated_goal") {
      relationalProfile = {
        ...relationalProfile,
        stated_goals: relationalProfile.stated_goals.filter((goal) => goal.id !== args?.id)
      };
      return { ...relationalProfile };
    }

    if (command === "delete_struggle_pattern") {
      relationalProfile = {
        ...relationalProfile,
        struggle_patterns: relationalProfile.struggle_patterns.filter((pattern) => pattern.id !== args?.id)
      };
      return { ...relationalProfile };
    }

    if (command === "clear_relational_profile") {
      relationalProfile = {
        ...relationalProfile,
        stated_goals: [],
        struggle_patterns: []
      };
      return { ...relationalProfile };
    }

    if (command === "list_proactive_trigger_audit_log") {
      return [];
    }

    if (command === "get_synthesis_log") {
      return [];
    }

    if (command === "clear_active_task_data") {
      messages.length = 0;
      recentlyLearned.length = 0;
      return {
        cleared: true,
        active_task_id: 1,
        message: "Active task data cleared. The task record was kept."
      };
    }

    if (command === "clear_all_jeff_data") {
      tasks.length = 0;
      messages.length = 0;
      onboardingStatus = {
        onboarding_complete: false,
        has_stored_api_key: false,
        api_key_source: "none",
        preferred_workspace_folder: null
      };
      privacyDashboard = {
        ...privacyDashboard,
        active_task_id: null,
        active_task_title: null,
        workspace_watcher_running: false,
        clipboard_capture_enabled: false
      };
      return {
        cleared: true,
        active_task_id: null,
        message: "All Jeff data cleared. Jeff is back in first-run state."
      };
    }

    if (command === "get_onboarding_status") {
      return { ...onboardingStatus };
    }

    if (command === "get_workspace_prompt_dismissed") {
      return workspacePromptDismissed;
    }

    if (command === "set_workspace_prompt_dismissed") {
      workspacePromptDismissed = Boolean(args?.dismissed);
      return null;
    }

    if (command === "get_task_summary") {
      return {
        task_id: 1,
        summary_text: "Summary placeholder for 'history storymap'.",
        updated_at: now
      };
    }

    if (command === "get_task_workspace") {
      return {
        task_id: 1,
        slug: "history-storymap",
        workspace_path: "/tmp/jeff_data/tasks/history-storymap",
        exists_on_disk: true
      };
    }

    if (command === "start_workspace_watcher") {
      watcherStatus = {
        task_id: 1,
        is_watching: true,
        watched_path: "/tmp/jeff_data/tasks/history-storymap"
      };
      return { ...watcherStatus };
    }

    if (command === "stop_workspace_watcher") {
      watcherStatus = {
        task_id: 1,
        is_watching: false,
        watched_path: null
      };
      return { ...watcherStatus };
    }

    if (command === "get_watcher_status") {
      return { ...watcherStatus };
    }

    if (command === "list_recently_learned") {
      return [...recentlyLearned];
    }

    if (command === "set_clipboard_capture") {
      clipboardCaptureEnabled = Boolean(args?.enabled);
      return null;
    }

    if (command === "get_clipboard_capture_setting") {
      return clipboardCaptureEnabled;
    }

    if (command === "classify_message_intent") {
      const text = String(args?.messageText ?? "").trim().toLowerCase();
      if (text.includes("fix this intro")) {
        return {
          intent: "revision",
          confidence: 0.94,
          slots: {
            target_description: "intro",
            instruction: "fix this intro",
            draft_type: null,
            topic: null
          }
        };
      }
      if (text.includes("draft better intro")) {
        return {
          intent: "subtask",
          confidence: 0.92,
          slots: {
            target_description: null,
            instruction: "draft better intro",
            draft_type: "draft_generation",
            topic: null
          }
        };
      }
      if (text === "ok" || text === "...") {
        return {
          intent: "unknown",
          confidence: 0.2,
          slots: {
            target_description: null,
            instruction: null,
            draft_type: null,
            topic: null
          }
        };
      }
      return {
        intent: "answer",
        confidence: 0.88,
        slots: {
          target_description: null,
          instruction: null,
          draft_type: null,
          topic: "requirements"
        }
      };
    }

    if (command === "list_open_resources") {
      return [];
    }

    if (command === "list_artifacts") {
      return [...artifacts];
    }

    if (command === "get_artifact_content") {
      return {
        artifact_id: 10,
        task_id: 1,
        file_name: "notes.md",
        file_extension: "md",
        stored_path: "/tmp/jeff_data/tasks/history-storymap/artifacts/notes.md",
        content: artifactContent,
        is_editable: true
      };
    }

    if (command === "list_pending_revisions") {
      return pendingRevisions.filter((item) => item.status === "pending");
    }

    if (command === "list_task_pending_revisions") {
      return pendingRevisions.filter((item) => item.status === "pending");
    }

    if (command === "list_artifact_versions") {
      return [...versions];
    }

    if (command === "list_messages") {
      return [...messages];
    }

    if (command === "list_recent_events") {
      const limit = Number(args?.limit ?? 20);
      return events.slice(-limit);
    }

    if (command === "list_subtasks") {
      advanceSubtasks();
      return subtasks
        .slice()
        .sort((left, right) => right.subtask_id - left.subtask_id);
    }

    if (command === "list_file_write_proposals") {
      return [];
    }

    if (command === "list_subtask_steps") {
      return [];
    }

    if (command === "list_write_audit_log") {
      return [];
    }

    if (command === "get_session_mode_state") {
      return { ...sessionModeState };
    }

    if (command === "get_active_artifact_selection") {
      return persistedActiveArtifactId;
    }

    if (command === "set_active_artifact_selection") {
      const artifactIdArg = args?.artifactId;
      if (typeof artifactIdArg === "number") {
        persistedActiveArtifactId = artifactIdArg;
      } else {
        persistedActiveArtifactId = null;
      }
      appendEvent("artifact_selection_updated", {
        artifact_id: persistedActiveArtifactId
      });
      return persistedActiveArtifactId;
    }

    if (command === "list_suggestions") {
      return listPendingSuggestions();
    }

    if (command === "evaluate_next_suggestions") {
      const activeArtifactIdRaw = args?.activeArtifactId;
      const activeArtifactId =
        typeof activeArtifactIdRaw === "number"
          ? activeArtifactIdRaw
          : activeArtifactIdRaw === null
            ? null
            : 10;
      const beforeIds = new Set(listPendingSuggestions().map((item) => item.suggestion_id));
      maybeGenerateSuggestions(activeArtifactId);
      const pending = listPendingSuggestions();
      const generated = pending.filter((item) => !beforeIds.has(item.suggestion_id));

      sessionModeState = {
        ...sessionModeState,
        current_mode: activeArtifactId === null ? "quiet_observing" : "writing",
        mode_reason:
          activeArtifactId === null
            ? "no active artifact selected"
            : "active artifact and drafting activity detected",
        waiting_on_user_decision: pending.length > 0,
        last_engine_decision: pending.length > 0 ? "generated_suggestions" : "no_suggestion_after_dedup_filters",
        active_artifact_id: activeArtifactId,
        updated_at: now
      };

      return {
        mode_state: { ...sessionModeState },
        suggestions: pending,
        generated_suggestions: generated,
        decision_reason: pending.length > 0 ? "generated_suggestions" : "no_suggestion_after_dedup_filters",
        no_op: pending.length === 0,
        evidence_score: 0.86,
        active_artifact_id: activeArtifactId,
        suppression_state: "none",
        retrieved_chunks: [
          {
            chunk_id: 610,
            task_id: 1,
            artifact_id: 10,
            artifact_file_name: "rubric.txt",
            artifact_stored_path: "/tmp/jeff_data/tasks/history-storymap/artifacts/rubric.txt",
            chunk_text: "Each section should connect thesis, primary source evidence, and course readings.",
            position_index: 1,
            similarity_score: 0.88
          }
        ]
      };
    }

    if (command === "dismiss_suggestion") {
      const suggestionId = Number(args?.suggestionId ?? 0);
      const suggestion = suggestions.find((item) => item.suggestion_id === suggestionId);
      if (!suggestion) {
        throw new Error("suggestion not found");
      }
      suggestion.status = "dismissed";
      suggestion.updated_at = now;
      dismissedSuggestionKeys.add(suggestion.suggestion_key);
      sessionModeState = {
        ...sessionModeState,
        waiting_on_user_decision: listPendingSuggestions().length > 0,
        last_engine_decision: "dismissed_suggestion",
        updated_at: now
      };
      appendEvent("suggestion_dismissed", { suggestion_id: suggestionId });
      return { ...suggestion };
    }

    if (command === "explain_suggestion") {
      const suggestionId = Number(args?.suggestionId ?? 0);
      const suggestion = suggestions.find((item) => item.suggestion_id === suggestionId);
      if (!suggestion) {
        throw new Error("suggestion not found");
      }
      return `${suggestion.title}: ${suggestion.source_reason}`;
    }

    if (command === "accept_suggestion") {
      const suggestionId = Number(args?.suggestionId ?? 0);
      const suggestion = suggestions.find((item) => item.suggestion_id === suggestionId);
      if (!suggestion) {
        throw new Error("suggestion not found");
      }
      suggestion.status = "accepted";
      suggestion.updated_at = now;

      if (suggestion.suggestion_type === "propose_subtask") {
        const subtask: SubTaskDto = {
          subtask_id: nextSubtaskId,
          task_id: 1,
          title: suggestion.title,
          description: suggestion.description,
          execution_type: suggestion.linked_subtask_type ?? "draft_generation",
          status: "pending",
          result_review_status: "unreviewed",
          created_at: now,
          updated_at: now,
          result_summary: null,
          result_payload: null,
          instruction_source: "system",
          parent_context_snapshot: buildSnapshot(suggestion.description, suggestion.linked_subtask_type ?? "draft_generation"),
          error_message: null
        };
        subtasks.push(subtask);
        subtaskPollCountdown.set(subtask.subtask_id, 1);
        nextSubtaskId += 1;
        announceSubtaskMessage(`Subtask #${subtask.subtask_id} started: ${subtask.title}.`);

        sessionModeState = {
          ...sessionModeState,
          waiting_on_user_decision: listPendingSuggestions().length > 0,
          last_engine_decision: "accepted_suggestion_started_subtask",
          updated_at: now
        };
        appendEvent("suggestion_accepted", {
          suggestion_id: suggestion.suggestion_id,
          action_type: "subtask_started"
        });

        return {
          suggestion: { ...suggestion },
          action_type: "subtask_started",
          followup_message: null,
          revision_result: null,
          subtask
        };
      }

      const instruction = suggestion.linked_revision_intent ?? suggestion.description;
      const originalText = artifactContent.slice(0, 35);
      const proposedText =
        "This introduction narrows the thesis and directly links citizenship arguments to course readings and primary-source evidence.";
      const revision: RevisionDto = {
        revision_id: nextRevisionId,
        task_id: 1,
        artifact_id: 10,
        target_start_offset: 0,
        target_end_offset: 35,
        target_description: "selected range 0..35",
        original_text: originalText,
        proposed_text: proposedText,
        instruction_text: instruction,
        instruction_source: "system",
        rationale: "Routed from accepted suggestion",
        grounding_notes: "Grounded in rubric and notes",
        retrieval_confidence: 0.85,
        status: "pending",
        created_at: now,
        updated_at: now
      };
      pendingRevisions.push(revision);
      nextRevisionId += 1;
      messages.push({
        id: nextMessageId,
        task_id: 1,
        session_id: 1,
        role: "assistant",
        message_source: "assistant",
        message_kind: "assistant_revision_proposal",
        content: `Revision proposal #${revision.revision_id} created from accepted suggestion.`,
        created_at: now
      });
      nextMessageId += 1;

      sessionModeState = {
        ...sessionModeState,
        current_mode: "revising",
        waiting_on_user_decision: listPendingSuggestions().length > 0,
        last_engine_decision: "accepted_suggestion_created_revision",
        updated_at: now
      };
      appendEvent("suggestion_accepted", {
        suggestion_id: suggestion.suggestion_id,
        action_type: "revision_proposal_created"
      });

      return {
        suggestion: { ...suggestion },
        action_type: "revision_proposal_created",
        followup_message: null,
        revision_result: {
          proposal: revision,
          retrieved_chunks: [
            {
              chunk_id: 612,
              task_id: 1,
              artifact_id: 10,
              artifact_file_name: "rubric.txt",
              artifact_stored_path: "/tmp/jeff_data/tasks/history-storymap/artifacts/rubric.txt",
              chunk_text: "Tighten thesis and connect claims to primary source evidence and readings.",
              position_index: 2,
              similarity_score: 0.9
            }
          ],
          active_artifact_id: 10,
          used_start_offset: 0,
          used_end_offset: 35,
          selection_source: "explicit_range",
          confidence: 0.85,
          grounding_notes: "Grounded in rubric and notes",
          context_source: "suggestion_acceptance"
        },
        subtask: null
      };
    }

    if (command === "create_subtask" || command === "start_subtask_chain") {
      const title = String(args?.title ?? "");
      const description = String(args?.description ?? "");
      const executionType = String(args?.executionType ?? "draft_generation");
      const source = String(args?.instructionSource ?? "text");

      const subtask: SubTaskDto = {
        subtask_id: nextSubtaskId,
        task_id: 1,
        title,
        description,
        execution_type: executionType,
        status: "pending",
        result_review_status: "unreviewed",
        created_at: now,
        updated_at: now,
        result_summary: null,
        result_payload: null,
        instruction_source: source,
        parent_context_snapshot: buildSnapshot(description, executionType),
        error_message: null
      };
      subtasks.push(subtask);
      subtaskPollCountdown.set(subtask.subtask_id, 1);
      nextSubtaskId += 1;
      announceSubtaskMessage(`Subtask #${subtask.subtask_id} started: ${subtask.title}.`);
      appendEvent("subtask_created", {
        subtask_id: subtask.subtask_id,
        execution_type: subtask.execution_type
      });
      return subtask;
    }

    if (command === "cancel_subtask") {
      const subtaskId = Number(args?.subtaskId ?? 0);
      const subtask = subtasks.find((item) => item.subtask_id === subtaskId);
      if (!subtask) {
        throw new Error("subtask not found");
      }
      subtask.status = "cancelled";
      subtask.error_message = "cancelled_by_user";
      subtask.updated_at = now;
      announceSubtaskMessage(`Subtask #${subtask.subtask_id} cancelled.`);
      appendEvent("subtask_cancelled", { subtask_id: subtask.subtask_id });
      return subtask;
    }

    if (command === "accept_subtask_result") {
      const subtaskId = Number(args?.subtaskId ?? 0);
      const subtask = subtasks.find((item) => item.subtask_id === subtaskId);
      if (!subtask) {
        throw new Error("subtask not found");
      }
      subtask.result_review_status = "accepted";
      subtask.updated_at = now;
      appendEvent("subtask_review_status_updated", {
        subtask_id: subtask.subtask_id,
        result_review_status: "accepted"
      });
      return subtask;
    }

    if (command === "reject_subtask_result") {
      const subtaskId = Number(args?.subtaskId ?? 0);
      const subtask = subtasks.find((item) => item.subtask_id === subtaskId);
      if (!subtask) {
        throw new Error("subtask not found");
      }
      subtask.result_review_status = "rejected";
      subtask.updated_at = now;
      appendEvent("subtask_review_status_updated", {
        subtask_id: subtask.subtask_id,
        result_review_status: "rejected"
      });
      return subtask;
    }

    if (command === "suggest_subtask") {
      return {
        task_id: 1,
        title: "Draft a stronger intro",
        description: "Draft one tighter intro paragraph linking citizenship framing to course readings.",
        execution_type: "draft_generation",
        instruction_source: "system",
        reason: "High-impact bounded drafting step based on your current materials.",
        parent_context_snapshot: buildSnapshot("suggested intro subtask", "draft_generation"),
        retrieved_chunks: [
          {
            chunk_id: 202,
            task_id: 1,
            artifact_id: 10,
            artifact_file_name: "rubric.txt",
            artifact_stored_path: "/tmp/jeff_data/tasks/history-storymap/artifacts/rubric.txt",
            chunk_text: "Each section should tie argument, evidence, and readings.",
            position_index: 3,
            similarity_score: 0.88
          }
        ]
      };
    }

    if (command === "refine_subtask") {
      const parentId = Number(args?.subtaskId ?? 0);
      const instruction = String(args?.instruction ?? "");
      const parent = subtasks.find((item) => item.subtask_id === parentId);
      if (!parent) {
        throw new Error("parent subtask not found");
      }

      const subtask: SubTaskDto = {
        subtask_id: nextSubtaskId,
        task_id: 1,
        title: `Refine: ${parent.title}`,
        description: instruction,
        execution_type: parent.execution_type,
        status: "pending",
        result_review_status: "unreviewed",
        created_at: now,
        updated_at: now,
        result_summary: null,
        result_payload: null,
        instruction_source: String(args?.instructionSource ?? "text"),
        parent_context_snapshot: buildSnapshot(instruction, parent.execution_type),
        error_message: null
      };
      subtasks.push(subtask);
      subtaskPollCountdown.set(subtask.subtask_id, 1);
      nextSubtaskId += 1;
      return subtask;
    }

    if (command === "convert_subtask_to_revision") {
      const subtaskId = Number(args?.subtaskId ?? 0);
      const subtask = subtasks.find((item) => item.subtask_id === subtaskId);
      if (!subtask || subtask.status !== "completed") {
        throw new Error("subtask not completed");
      }

      const originalText = artifactContent.slice(0, 35);
      const proposedText =
        "This introduction advances a tighter thesis by linking citizenship debates to course readings and primary evidence.";
      const revision: RevisionDto = {
        revision_id: nextRevisionId,
        task_id: 1,
        artifact_id: 10,
        target_start_offset: 0,
        target_end_offset: 35,
        target_description: "selected range 0..35",
        original_text: originalText,
        proposed_text: proposedText,
        instruction_text: `Converted from subtask #${subtaskId}`,
        instruction_source: "system",
        rationale: "Subtask conversion",
        grounding_notes: "Grounded in subtask snapshot and retrieved chunks",
        retrieval_confidence: 0.83,
        status: "pending",
        created_at: now,
        updated_at: now
      };
      pendingRevisions.push(revision);
      subtask.result_review_status = "converted";
      nextRevisionId += 1;
      appendEvent("subtask_converted_to_revision", {
        subtask_id: subtask.subtask_id,
        revision_id: revision.revision_id
      });

      messages.push({
        id: nextMessageId,
        task_id: 1,
        session_id: 1,
        role: "assistant",
        message_source: "assistant",
        message_kind: "assistant_revision_proposal",
        content: `Revision proposal #${revision.revision_id} created for notes.md (selected range 0..35).`,
        created_at: now
      });
      nextMessageId += 1;

      return {
        proposal: revision,
        retrieved_chunks: [
          {
            chunk_id: 301,
            task_id: 1,
            artifact_id: 10,
            artifact_file_name: "rubric.txt",
            artifact_stored_path: "/tmp/jeff_data/tasks/history-storymap/artifacts/rubric.txt",
            chunk_text: "Use citizenship framing, primary sources, and course readings.",
            position_index: 2,
            similarity_score: 0.89
          }
        ],
        active_artifact_id: 10,
        used_start_offset: 0,
        used_end_offset: 35,
        selection_source: "explicit_range",
        confidence: 0.83,
        grounding_notes: "Grounded in subtask context",
        context_source: "subtask_result"
      };
    }

    if (command === "get_coworking_status") {
      return {
        state: "silent_observing",
        proactive_mode: proactiveMode,
        user_typing: false,
        user_speaking: false,
        session_mode: "quiet",
        pause_threshold_seconds: 12,
        nudge_cooldown_seconds: 45,
        interruption_suppression_seconds: 25,
        low_confidence_suppression_seconds: 20,
        cooldown_remaining_seconds: 0,
        last_decision_reason: "awaiting_activity"
      };
    }

    if (
      command === "set_user_typing" ||
      command === "set_user_speaking" ||
      command === "set_assistant_speaking"
    ) {
      return {
        state: "silent_observing",
        proactive_mode: proactiveMode,
        user_typing: command === "set_user_typing" ? Boolean(args?.isTyping) : false,
        user_speaking: command === "set_user_speaking" ? Boolean(args?.isSpeaking) : false,
        session_mode: "quiet",
        pause_threshold_seconds: 12,
        nudge_cooldown_seconds: 45,
        interruption_suppression_seconds: 25,
        low_confidence_suppression_seconds: 20,
        cooldown_remaining_seconds: 0,
        last_decision_reason: "status_updated"
      };
    }

    if (command === "set_proactive_mode") {
      proactiveMode = Boolean(args?.enabled);
      appendEvent("proactive_mode_set", { enabled: proactiveMode });
      return {
        state: "idle",
        proactive_mode: proactiveMode,
        user_typing: false,
        user_speaking: false,
        session_mode: "quiet",
        pause_threshold_seconds: 12,
        nudge_cooldown_seconds: 45,
        interruption_suppression_seconds: 25,
        low_confidence_suppression_seconds: 20,
        cooldown_remaining_seconds: 0,
        last_decision_reason: proactiveMode ? "proactive_mode_enabled" : "proactive_mode_disabled"
      };
    }

    if (command === "evaluate_proactive_nudge") {
      return {
        status: {
          state: "silent_observing",
          proactive_mode: proactiveMode,
          user_typing: false,
          user_speaking: false,
          session_mode: "quiet",
          pause_threshold_seconds: 12,
          nudge_cooldown_seconds: 45,
          interruption_suppression_seconds: 25,
          low_confidence_suppression_seconds: 20,
          cooldown_remaining_seconds: 0,
          last_decision_reason: proactiveMode ? "pause_not_long_enough" : "proactive_mode_disabled"
        },
        decision_event_type: "system_status_event",
        decision_reason: proactiveMode ? "pause_not_long_enough" : "proactive_mode_disabled",
        nudge: null
      };
    }

    if (command === "cancel_interaction") {
      return 1;
    }

    if (command === "ambient_set_tray_status") {
      return {
        tray_status: String(args?.status ?? "idle"),
        overlay_mode: "collapsed",
        overlay_visible: false,
        hotkey: "CmdOrCtrl+Shift+J",
        hotkey_registered: true,
        notification_permission: "granted",
        single_instance: true,
        quiet_mode: false
      };
    }

    if (command === "ambient_open_onboarding") {
      return null;
    }

    if (command === "propose_artifact_revision") {
      const instruction = String(args?.instruction ?? "");
      const source = String(args?.instructionSource ?? "typed");
      const originalText = artifactContent.slice(0, 35);
      const proposedText =
        "Rather than only describing events, this section analyzes how citizenship claims shaped policy and evidence use.";

      const revision: RevisionDto = {
        revision_id: nextRevisionId,
        task_id: 1,
        artifact_id: 10,
        target_start_offset: 0,
        target_end_offset: 35,
        target_description: "selected range 0..35",
        original_text: originalText,
        proposed_text: proposedText,
        instruction_text: instruction,
        instruction_source: source,
        rationale: "Shifted to analysis",
        grounding_notes: "Grounded in rubric and notes",
        retrieval_confidence: 0.82,
        status: "pending",
        created_at: now,
        updated_at: now
      };
      pendingRevisions.push(revision);
      nextRevisionId += 1;
      appendEvent("revision_proposed", { revision_id: revision.revision_id });

      messages.push({
        id: nextMessageId,
        task_id: 1,
        session_id: 1,
        role: "assistant",
        message_source: "assistant",
        message_kind: "assistant_revision_proposal",
        content: `Revision proposal #${revision.revision_id} created for notes.md (selected range 0..35).`,
        created_at: now
      });
      nextMessageId += 1;

      return {
        proposal: revision,
        retrieved_chunks: [
          {
            chunk_id: 101,
            task_id: 1,
            artifact_id: 10,
            artifact_file_name: "rubric.txt",
            artifact_stored_path: "/tmp/jeff_data/tasks/history-storymap/artifacts/rubric.txt",
            chunk_text: "Use primary source evidence and analytical framing in each section.",
            position_index: 2,
            similarity_score: 0.91
          }
        ],
        active_artifact_id: 10,
        used_start_offset: 0,
        used_end_offset: 35,
        selection_source: "explicit_range",
        confidence: 0.82,
        grounding_notes: "Grounded in rubric and notes",
        context_source: "direct_instruction"
      };
    }

    if (command === "apply_revision") {
      const revisionId = Number(args?.revisionId ?? 0);
      const revision = pendingRevisions.find((item) => item.revision_id === revisionId);
      if (!revision) {
        throw new Error("revision not found");
      }

      const previousContent = artifactContent;
      artifactContent = artifactContent.replace(revision.original_text, revision.proposed_text);
      revision.status = "accepted";

      const version: ArtifactVersionDto = {
        version_id: nextVersionId,
        task_id: 1,
        artifact_id: 10,
        revision_id: revision.revision_id,
        version_reason: `before_apply_revision_${revision.revision_id}`,
        content_preview: previousContent.slice(0, 60),
        content_length: previousContent.length,
        created_at: now
      };
      versions.unshift(version);
      nextVersionId += 1;
      appendEvent("revision_applied", { revision_id: revision.revision_id });

      messages.push({
        id: nextMessageId,
        task_id: 1,
        session_id: 1,
        role: "assistant",
        message_source: "assistant",
        message_kind: "assistant_revision_status",
        content: `Applied revision #${revision.revision_id} and saved a version snapshot.`,
        created_at: now
      });
      nextMessageId += 1;

      return {
        revision,
        artifact_content: {
          artifact_id: 10,
          task_id: 1,
          file_name: "notes.md",
          file_extension: "md",
          stored_path: "/tmp/jeff_data/tasks/history-storymap/artifacts/notes.md",
          content: artifactContent,
          is_editable: true
        },
        version_snapshot: version
      };
    }

    if (command === "reject_revision") {
      const revisionId = Number(args?.revisionId ?? 0);
      const revision = pendingRevisions.find((item) => item.revision_id === revisionId);
      if (!revision) {
        throw new Error("revision not found");
      }

      revision.status = "rejected";
      appendEvent("revision_rejected", { revision_id: revision.revision_id });
      messages.push({
        id: nextMessageId,
        task_id: 1,
        session_id: 1,
        role: "assistant",
        message_source: "assistant",
        message_kind: "assistant_revision_status",
        content: `Rejected revision #${revision.revision_id}.`,
        created_at: now
      });
      nextMessageId += 1;

      return revision;
    }

    if (command === "revert_artifact_to_version") {
      const versionId = Number(args?.versionId ?? 0);
      const version = versions.find((item) => item.version_id === versionId);
      if (!version) {
        throw new Error("version not found");
      }

      artifactContent = initialArtifactContent;
      appendEvent("artifact_reverted", { version_id: version.version_id });

      messages.push({
        id: nextMessageId,
        task_id: 1,
        session_id: 1,
        role: "assistant",
        message_source: "assistant",
        message_kind: "assistant_revision_status",
        content: `Reverted artifact to version #${version.version_id}.`,
        created_at: now
      });
      nextMessageId += 1;

      return {
        artifact_id: 10,
        task_id: 1,
        file_name: "notes.md",
        file_extension: "md",
        stored_path: "/tmp/jeff_data/tasks/history-storymap/artifacts/notes.md",
        content: artifactContent,
        is_editable: true
      };
    }

    if (command === "transcribe_audio") {
      return {
        text: "cancel running subtask"
      };
    }

    if (command === "send_message") {
      const userContent = String(args?.message ?? "");
      const source = String(args?.source ?? "text");

      messages.push({
        id: nextMessageId,
        task_id: 1,
        session_id: 1,
        role: "user",
        message_source: source,
        message_kind: userContent.endsWith("?") ? "user_direct_question" : "user_statement",
        content: userContent,
        created_at: now
      });
      nextMessageId += 1;
      appendEvent("message_sent", { source, role: "user" });

      messages.push({
        id: nextMessageId,
        task_id: 1,
        session_id: 1,
        role: "assistant",
        message_source: "assistant",
        message_kind: "assistant_answer",
        content: "Use primary sources, course readings, and evidence requirements from the rubric.",
        created_at: now
      });
      nextMessageId += 1;
      appendEvent("message_sent", { source: "assistant", role: "assistant" });

      return {
        assistant_response: "Use primary sources, course readings, and evidence requirements from the rubric.",
        retrieved_chunks: [
          {
            chunk_id: 102,
            task_id: 1,
            artifact_id: 10,
            artifact_file_name: "rubric.txt",
            artifact_stored_path: "/tmp/jeff_data/tasks/history-storymap/artifacts/rubric.txt",
            chunk_text: "Primary source requirement and evidence expectations for each section.",
            position_index: 2,
            similarity_score: 0.91
          }
        ],
        cancelled: false
      };
    }

    if (command === "send_message_streaming") {
      const userContent = String(args?.message ?? "");
      const source = String(args?.source ?? "text");
      const turnId = `stream-turn-${nextStreamingTurnId}`;
      nextStreamingTurnId += 1;
      activeStreamingTurnId = turnId;

      messages.push({
        id: nextMessageId,
        task_id: 1,
        session_id: 1,
        role: "user",
        message_source: source,
        message_kind: userContent.endsWith("?") ? "user_direct_question" : "user_statement",
        content: userContent,
        created_at: now
      });
      nextMessageId += 1;

      const placeholderId = nextMessageId;
      messages.push({
        id: placeholderId,
        task_id: 1,
        session_id: 1,
        role: "assistant",
        message_source: "assistant",
        message_kind: "assistant_partial",
        content: "",
        created_at: now
      });
      nextMessageId += 1;
      activeStreamingPlaceholderId = placeholderId;

      appendEvent("message_stream_started", { turn_id: turnId, source });
      return turnId;
    }

    if (command === "cancel_streaming_turn") {
      cancelStreamingTurn("");
      return true;
    }

    if (command === "synthesize_speech") {
      return {
        audio_base64: "AQ==",
        mime_type: "audio/mpeg"
      };
    }

    if (command === "set_active_task") {
      tasks[0].is_active = true;
      return tasks[0];
    }

    if (command === "create_task" || command === "import_artifact") {
      return null;
    }

    throw new Error(`unexpected command ${command}`);
  });

  return {
    getActiveStreamingTurnId: () => activeStreamingTurnId,
    finalizeStreamingTurn,
    cancelStreamingTurn,
    setOnboardingStatus: (next: typeof onboardingStatus) => {
      onboardingStatus = next;
    },
    setWorkspacePromptDismissed: (next: boolean) => {
      workspacePromptDismissed = next;
    }
  };
}

function installAudioAndUrlMocks() {
  const playMock = vi
    .spyOn(window.HTMLMediaElement.prototype, "play")
    .mockImplementation(async () => undefined);

  const originalCreateObjectURL = (URL as unknown as { createObjectURL?: (blob: Blob) => string })
    .createObjectURL;
  const originalRevokeObjectURL = (URL as unknown as { revokeObjectURL?: (url: string) => void })
    .revokeObjectURL;

  const createObjectURLMock = vi.fn(() => "blob:test");
  const revokeObjectURLMock = vi.fn(() => undefined);
  (URL as unknown as { createObjectURL: (blob: Blob) => string }).createObjectURL = createObjectURLMock;
  (URL as unknown as { revokeObjectURL: (url: string) => void }).revokeObjectURL = revokeObjectURLMock;

  return {
    playMock,
    restore: () => {
      playMock.mockRestore();
      (URL as unknown as { createObjectURL?: (blob: Blob) => string }).createObjectURL = originalCreateObjectURL;
      (URL as unknown as { revokeObjectURL?: (url: string) => void }).revokeObjectURL = originalRevokeObjectURL;
    }
  };
}

function installMediaRecorderMocks() {
  const trackStopMock = vi.fn();
  const fakeStream: StreamLike = {
    getTracks: () => [{ stop: trackStopMock }]
  };

  const getUserMediaMock = vi.fn().mockResolvedValue(fakeStream as unknown as MediaStream);
  const originalMediaDevices = navigator.mediaDevices;
  const originalMediaRecorder = globalThis.MediaRecorder;

  Object.defineProperty(navigator, "mediaDevices", {
    configurable: true,
    value: {
      getUserMedia: getUserMediaMock
    }
  });

  class FakeMediaRecorder {
    public state: RecordingState = "inactive";
    public ondataavailable: ((event: BlobEvent) => void) | null = null;
    public onstop: ((this: MediaRecorder, event: Event) => void) | null = null;

    constructor(_stream: MediaStream) {
      // no-op
    }

    start() {
      this.state = "recording";
    }

    stop() {
      this.state = "inactive";
      const blob = new Blob(["voice-audio"], { type: "audio/webm" });
      this.ondataavailable?.({ data: blob } as BlobEvent);
      this.onstop?.call(this as unknown as MediaRecorder, new Event("stop"));
    }
  }

  (globalThis as unknown as { MediaRecorder: typeof MediaRecorder }).MediaRecorder =
    FakeMediaRecorder as unknown as typeof MediaRecorder;

  return {
    getUserMediaMock,
    trackStopMock,
    restore: () => {
      Object.defineProperty(navigator, "mediaDevices", {
        configurable: true,
        value: originalMediaDevices
      });

      if (originalMediaRecorder) {
        (globalThis as unknown as { MediaRecorder: typeof MediaRecorder }).MediaRecorder = originalMediaRecorder;
      } else {
        delete (globalThis as unknown as { MediaRecorder?: typeof MediaRecorder }).MediaRecorder;
      }
    }
  };
}

function installBlobArrayBufferPolyfill() {
  const originalArrayBuffer = Blob.prototype.arrayBuffer;

  if (!originalArrayBuffer) {
    Blob.prototype.arrayBuffer = function arrayBufferPolyfill(this: Blob): Promise<ArrayBuffer> {
      return new Promise((resolve, reject) => {
        const reader = new FileReader();
        reader.onerror = () => reject(reader.error ?? new Error("failed to read blob"));
        reader.onload = () => resolve(reader.result as ArrayBuffer);
        reader.readAsArrayBuffer(this);
      });
    };
  }

  return {
    restore: () => {
      if (originalArrayBuffer) {
        Blob.prototype.arrayBuffer = originalArrayBuffer;
      } else {
        delete (Blob.prototype as { arrayBuffer?: Blob["arrayBuffer"] }).arrayBuffer;
      }
    }
  };
}

describe("App", () => {
  it("preserves revision and chat baseline behavior", async () => {
    setupInvokeMock();
    const audioMocks = installAudioAndUrlMocks();

    try {
      render(<App />);

      await screen.findByTestId("home-resume-screen");
      await userEvent.click(screen.getByTestId("continue-task-button"));
      await screen.findByTestId("workspace-screen");
      await userEvent.click(screen.getByTestId("toggle-full-workspace"));

      await userEvent.type(
        screen.getByTestId("revision-instruction-input"),
        "make this section more analytical"
      );
      await userEvent.click(screen.getByRole("button", { name: /propose revision/i }));

      const pendingRevisionList = await screen.findByTestId("pending-revisions-list");
      expect(pendingRevisionList).toHaveTextContent("make this section more analytical");
      await userEvent.click(within(pendingRevisionList).getByRole("button", { name: /accept/i }));
      expect(await screen.findByTestId("artifact-versions-list")).toHaveTextContent("before_apply_revision");

      const chatInput = screen.getByLabelText(/chat input/i);
      await userEvent.type(chatInput, "What are the primary source requirements?");
      await userEvent.click(screen.getByRole("button", { name: /^send$/i }));

      expect(await screen.findByText(/primary sources, course readings/i)).toBeInTheDocument();
      expect(audioMocks.playMock).toHaveBeenCalled();
    } finally {
      audioMocks.restore();
    }
  });

  it("supports bounded subtask lifecycle, parallel chat, and result-to-revision conversion", async () => {
    setupInvokeMock();
    const audioMocks = installAudioAndUrlMocks();

    try {
      render(<App />);

      await screen.findByTestId("home-resume-screen");
      await userEvent.click(screen.getByTestId("continue-task-button"));
      await screen.findByTestId("workspace-screen");
      await userEvent.click(screen.getByTestId("toggle-full-workspace"));

      await userEvent.type(screen.getByTestId("subtask-instruction-input"), "draft a better intro");
      await userEvent.click(screen.getByRole("button", { name: /create subtask/i }));

      expect(await screen.findByTestId("active-subtasks-list")).toHaveTextContent("draft a better intro");

      const chatInput = screen.getByLabelText(/chat input/i);
      await userEvent.type(chatInput, "what are the requirements");
      await userEvent.click(screen.getByRole("button", { name: /^send$/i }));
      expect(await screen.findByTestId("chat-history")).toHaveTextContent("Answer");

      await waitFor(
        async () => {
          expect(await screen.findByTestId("completed-subtasks-list")).toHaveTextContent("Completed");
        },
        { timeout: 3500 }
      );

      await userEvent.click(screen.getByRole("button", { name: /convert to revision/i }));
      expect(await screen.findByTestId("pending-revisions-list")).toHaveTextContent("Converted from subtask");
      expect(await screen.findByTestId("chat-history")).toHaveTextContent("Revision Proposal");
    } finally {
      audioMocks.restore();
    }
  });

  it("supports canceling running subtasks via UI and voice controls", async () => {
    setupInvokeMock();
    const mediaMocks = installMediaRecorderMocks();
    const audioMocks = installAudioAndUrlMocks();
    const blobPolyfill = installBlobArrayBufferPolyfill();

    try {
      render(<App />);

      await screen.findByTestId("home-resume-screen");
      await userEvent.click(screen.getByTestId("continue-task-button"));
      await screen.findByTestId("workspace-screen");
      await userEvent.click(screen.getByTestId("toggle-full-workspace"));

      await userEvent.type(screen.getByTestId("subtask-instruction-input"), "expand this outline");
      await userEvent.click(screen.getByRole("button", { name: /create subtask/i }));
      expect(await screen.findByTestId("active-subtasks-list")).toHaveTextContent("expand this outline");

      await userEvent.click(screen.getByRole("button", { name: /^cancel$/i }));
      expect(await screen.findByTestId("completed-subtasks-list")).toHaveTextContent("cancelled");

      await userEvent.type(screen.getByTestId("subtask-instruction-input"), "draft another intro");
      await userEvent.click(screen.getByRole("button", { name: /create subtask/i }));
      expect(await screen.findByTestId("active-subtasks-list")).toHaveTextContent("draft another intro");

      await userEvent.click(screen.getByTestId("voice-cancel-subtask-button"));
      expect(await screen.findByTestId("recording-indicator")).toHaveTextContent("cancel_subtask");
      await userEvent.click(screen.getByTestId("record-toggle"));

      expect(invokeMock).toHaveBeenCalledWith("cancel_subtask", expect.any(Object));
      expect(mediaMocks.getUserMediaMock).toHaveBeenCalled();
      expect(mediaMocks.trackStopMock).toHaveBeenCalled();
    } finally {
      mediaMocks.restore();
      audioMocks.restore();
      blobPolyfill.restore();
    }
  });

  it("supports dismissing suggestions and suppresses immediate reappearance", async () => {
    setupInvokeMock();
    const audioMocks = installAudioAndUrlMocks();

    try {
      render(<App />);

      await screen.findByTestId("home-resume-screen");
      await userEvent.click(screen.getByTestId("continue-task-button"));
      await screen.findByTestId("workspace-screen");
      await userEvent.click(screen.getByTestId("toggle-full-workspace"));

      expect(await screen.findByTestId("suggestions-list")).toHaveTextContent("Tighten intro argument");
      await userEvent.click(screen.getByTestId("suggestion-dismiss-1"));
      expect(await screen.findByTestId("suggestion-action-message")).toHaveTextContent("Dismissed suggestion");

      await userEvent.click(screen.getByTestId("suggestions-refresh-button"));
      expect(screen.queryByText("Tighten intro argument")).not.toBeInTheDocument();
    } finally {
      audioMocks.restore();
    }
  });

  it("routes accepted suggestions into revision proposals and bounded subtasks", async () => {
    setupInvokeMock();
    const audioMocks = installAudioAndUrlMocks();

    try {
      render(<App />);

      await screen.findByTestId("home-resume-screen");
      await userEvent.click(screen.getByTestId("continue-task-button"));
      await screen.findByTestId("workspace-screen");
      await userEvent.click(screen.getByTestId("toggle-full-workspace"));

      await userEvent.click(screen.getByTestId("suggestion-accept-1"));
      expect(await screen.findByTestId("pending-revisions-list")).toHaveTextContent("tighten thesis");
      expect(await screen.findByTestId("suggestion-action-message")).toHaveTextContent("Revision proposal created");

      await userEvent.click(screen.getByTestId("suggestion-accept-2"));
      expect(await screen.findByTestId("active-subtasks-list")).toHaveTextContent("Draft stronger intro in parallel");
      expect(await screen.findByTestId("suggestion-action-message")).toHaveTextContent("subtask started");
    } finally {
      audioMocks.restore();
    }
  });

  it("keeps action center and runtime inspector coherent through resume and mixed pending work", async () => {
    setupInvokeMock();
    const audioMocks = installAudioAndUrlMocks();

    try {
      render(<App />);

      await screen.findByTestId("home-resume-screen");
      await userEvent.click(screen.getByTestId("continue-task-button"));
      await screen.findByTestId("workspace-screen");
      await userEvent.click(screen.getByTestId("toggle-full-workspace"));
      await userEvent.click(screen.getByTestId("debug-panels-toggle"));

      await userEvent.click(screen.getByTestId("suggestion-accept-1"));
      expect(await screen.findByTestId("action-center-revisions-list")).toHaveTextContent("#1");

      await userEvent.click(screen.getByTestId("suggestion-accept-2"));
      expect(await screen.findByTestId("action-center-active-subtasks-list")).toHaveTextContent("#1");
      expect(await screen.findByTestId("runtime-events-list")).toHaveTextContent("suggestion_accepted");

      await userEvent.click(screen.getByRole("button", { name: /back to home/i }));
      await screen.findByTestId("home-resume-screen");
      await userEvent.click(screen.getByTestId("continue-task-button"));
      await screen.findByTestId("workspace-screen");
      await userEvent.click(screen.getByTestId("toggle-full-workspace"));

      expect(await screen.findByTestId("action-center-revisions-list")).toHaveTextContent("#1");
      expect(await screen.findByTestId("runtime-inspector-panel")).toHaveTextContent("Active task: 1");
    } finally {
      audioMocks.restore();
    }
  });

  it("surfaces provider failures and keeps visible text output when TTS fails", async () => {
    setupInvokeMock({ failCommands: { synthesize_speech: "tts unavailable" } });

    render(<App />);

    await screen.findByTestId("home-resume-screen");
    await userEvent.click(screen.getByTestId("continue-task-button"));
    await screen.findByTestId("workspace-screen");

    await userEvent.type(screen.getByLabelText(/chat input/i), "what are requirements");
    await userEvent.click(screen.getByRole("button", { name: /^send$/i }));

    expect(await screen.findByTestId("chat-history")).toHaveTextContent("Use primary sources, course readings");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Text-to-speech failed (response text still available in chat): tts unavailable"
    );
  });

  it("opens in companion mode by default with low-cognitive-load entry", async () => {
    setupInvokeMock();
    render(<App />);

    await screen.findByTestId("home-resume-screen");
    await userEvent.click(screen.getByTestId("continue-task-button"));
    await screen.findByTestId("workspace-screen");

    expect(await screen.findByTestId("companion-context-header")).toHaveTextContent("Task: history storymap");
    expect(screen.queryByTestId("action-center-panel")).not.toBeInTheDocument();
    expect(screen.queryByTestId("next-suggestions-panel")).not.toBeInTheDocument();
    expect(screen.getByTestId("companion-inline-actions")).toBeInTheDocument();
  });

  it("routes conversational requests into revision and subtask actions without panel navigation", async () => {
    setupInvokeMock();
    render(<App />);

    await screen.findByTestId("home-resume-screen");
    await userEvent.click(screen.getByTestId("continue-task-button"));
    await screen.findByTestId("workspace-screen");

    await userEvent.type(screen.getByLabelText(/chat input/i), "fix this intro");
    await userEvent.click(screen.getByRole("button", { name: /^send$/i }));
    expect(await screen.findByTestId("companion-revision-card")).toBeInTheDocument();
    expect(invokeMock).toHaveBeenCalledWith("propose_artifact_revision", expect.any(Object));

      await userEvent.type(screen.getByLabelText(/chat input/i), "draft better intro");
      await userEvent.click(screen.getByRole("button", { name: /^send$/i }));
      expect(invokeMock).toHaveBeenCalledWith("start_subtask_chain", expect.any(Object));
    });

  it("uses clarify path when classifier returns unknown intent", async () => {
    setupInvokeMock();
    render(<App />);

    await screen.findByTestId("home-resume-screen");
    await userEvent.click(screen.getByTestId("continue-task-button"));
    await screen.findByTestId("workspace-screen");

    await userEvent.type(screen.getByLabelText(/chat input/i), "ok");
    await userEvent.click(screen.getByRole("button", { name: /^send$/i }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Jeff needs clarification");
    const sendCalls = invokeMock.mock.calls.filter((call) => call[0] === "send_message");
    expect(sendCalls).toHaveLength(0);
  });

  it("streams chat responses token-by-token and finalizes via stream completion events", async () => {
    streamingEnabled = true;
    const streaming = setupInvokeMock();
    render(<App />);

    await screen.findByTestId("home-resume-screen");
    await userEvent.click(screen.getByTestId("continue-task-button"));
    await screen.findByTestId("workspace-screen");

    await userEvent.type(screen.getByLabelText(/chat input/i), "what are requirements");
    await userEvent.click(screen.getByRole("button", { name: /^send$/i }));

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("send_message_streaming", expect.any(Object))
    );

    const turnId = streaming.getActiveStreamingTurnId() as string;
    expect(turnId).toBeTruthy();

    emitTauriEvent("stream://llm_token", {
      turn_id: turnId,
      delta: "Streamed",
      index: 0
    });
    emitTauriEvent("stream://llm_token", {
      turn_id: turnId,
      delta: " answer",
      index: 1
    });

    expect(await screen.findByTestId("streaming-message")).toHaveTextContent("Streamed answer");

    streaming.finalizeStreamingTurn("Streamed final answer.");
    emitTauriEvent("stream://llm_complete", {
      turn_id: turnId,
      full_text: "Streamed final answer.",
      cancelled: false,
      ttft_ms: 25,
      total_ms: 110
    });
    emitTauriEvent("stream://turn_complete", {
      turn_id: turnId,
      duration_ms: 110,
      ttft_ms: 25,
      first_audio_ms: 60
    });

    await waitFor(() =>
      expect(screen.queryByTestId("streaming-message")).not.toBeInTheDocument()
    );
    expect(await screen.findByTestId("chat-history")).toHaveTextContent("Streamed final answer.");
  });

  it("cleans up streaming UI state on turn_cancelled events", async () => {
    streamingEnabled = true;
    const streaming = setupInvokeMock();
    render(<App />);

    await screen.findByTestId("home-resume-screen");
    await userEvent.click(screen.getByTestId("continue-task-button"));
    await screen.findByTestId("workspace-screen");

    await userEvent.type(screen.getByLabelText(/chat input/i), "draft something");
    await userEvent.click(screen.getByRole("button", { name: /^send$/i }));

    await waitFor(() =>
      expect(streaming.getActiveStreamingTurnId()).toBeTruthy()
    );
    const turnId = streaming.getActiveStreamingTurnId() as string;

    emitTauriEvent("stream://llm_token", {
      turn_id: turnId,
      delta: "partial",
      index: 0
    });
    expect(await screen.findByTestId("streaming-message")).toHaveTextContent("partial");

    streaming.cancelStreamingTurn("(interrupted)");
    emitTauriEvent("stream://turn_cancelled", {
      turn_id: turnId,
      reason: "user_barge_in",
      partial_text: "(interrupted)",
      elapsed_ms: 42
    });

    await waitFor(() =>
      expect(screen.queryByTestId("streaming-message")).not.toBeInTheDocument()
    );
    expect(await screen.findByTestId("chat-history")).toHaveTextContent("(interrupted)");
  });

  // phase 18 tests -----------------------------------------------------------

  it("shows workspace soft prompt when onboarding is complete but no folder is set", async () => {
    const mocks = setupInvokeMock();
    mocks.setOnboardingStatus({
      onboarding_complete: true,
      has_stored_api_key: true,
      api_key_source: "keychain",
      preferred_workspace_folder: null
    });

    render(<App />);

    await screen.findByTestId("home-resume-screen");
    await userEvent.click(screen.getByTestId("continue-task-button"));
    await screen.findByTestId("workspace-screen");

    expect(await screen.findByTestId("companion-workspace-soft-prompt")).toBeInTheDocument();
  });

  it("offers accessibility permission with explicit user action", async () => {
    const mocks = setupInvokeMock({ accessibilityGranted: false });
    mocks.setOnboardingStatus({
      onboarding_complete: true,
      has_stored_api_key: true,
      api_key_source: "keychain",
      preferred_workspace_folder: "/tmp/jeff_data/tasks/history-storymap"
    });

    render(<App />);

    await screen.findByTestId("home-resume-screen");
    await userEvent.click(screen.getByTestId("continue-task-button"));
    await screen.findByTestId("workspace-screen");

    expect(await screen.findByTestId("accessibility-context-prompt")).toHaveTextContent(
      "Jeff needs accessibility permission to know which document you have open."
    );
    await userEvent.click(screen.getByTestId("request-accessibility-permission"));
    expect(invokeMock).toHaveBeenCalledWith("request_accessibility_permission");
  });

  it("document-switch nudge includes a start-task action", async () => {
    setupInvokeMock();

    render(<App />);

    await screen.findByTestId("home-resume-screen");
    await userEvent.click(screen.getByTestId("continue-task-button"));
    await screen.findByTestId("workspace-screen");

    for (const handler of eventHandlers.get("context://document-switch") ?? []) {
      handler({ payload: { app_name: "TextEdit", document_title: "Civil Rights Draft" } });
    }

    expect(await screen.findByTestId("doc-switch-banner")).toHaveTextContent(
      "Civil Rights Draft"
    );
    expect(screen.getByTestId("doc-switch-start-task")).toBeInTheDocument();
  });

  it("opens Privacy Center and persists a sensing toggle", async () => {
    setupInvokeMock();

    render(<App />);

    await screen.findByTestId("home-resume-screen");
    await userEvent.click(screen.getByTestId("continue-task-button"));
    await screen.findByTestId("workspace-screen");

    await userEvent.click(screen.getByTestId("privacy-center-open"));
    expect(await screen.findByTestId("privacy-center-panel")).toHaveTextContent("What Jeff knows");
    expect(screen.getByTestId("privacy-surface-workspace")).toHaveTextContent("Workspace watcher");
    expect(screen.getByTestId("privacy-surface-active-window")).toHaveTextContent(
      "Accessibility permission: granted"
    );
    expect(screen.getByTestId("privacy-surface-selection-capture")).toHaveTextContent(
      "Browser bridge: 127.0.0.1:47832"
    );
    expect(screen.getByTestId("privacy-surface-typing-activity")).toHaveTextContent(
      "Rate-only"
    );

    await userEvent.click(screen.getByTestId("privacy-toggle-active-window-context"));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("set_privacy_surface_enabled", {
        surface: "active_window_context",
        enabled: false
      })
    );
  });

  it("shows and dismisses selected-text capture indicators", async () => {
    setupInvokeMock();

    render(<App />);

    await screen.findByTestId("home-resume-screen");

    emitTauriEvent("selection://captured", {
      status: "captured",
      app_name: "TextEdit",
      document_title: "Essay Draft",
      captured_at: 1,
      word_count: 8,
      source_type: "native_accessibility",
      message: "Captured 8 words from TextEdit"
    });

    expect(await screen.findByTestId("selection-capture-indicator")).toHaveTextContent(
      "Captured 8 words from TextEdit"
    );
    expect(screen.getByTestId("selection-capture-indicator")).toHaveTextContent("Essay Draft");

    await userEvent.click(screen.getByTestId("selection-capture-dismiss"));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("dismiss_selection_capture"));
    expect(screen.queryByTestId("selection-capture-indicator")).not.toBeInTheDocument();
  });

  it("persists TTS voice selection from Privacy Center", async () => {
    setupInvokeMock();

    render(<App />);

    await screen.findByTestId("home-resume-screen");
    await userEvent.click(screen.getByTestId("continue-task-button"));
    await screen.findByTestId("workspace-screen");

    await userEvent.click(screen.getByTestId("privacy-center-open"));
    const select = await screen.findByTestId("tts-voice-select");
    await userEvent.selectOptions(select, "nova");

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("set_tts_voice", { voice: "nova" })
    );
  });

  it("Privacy Center clear all requires explicit confirmation text", async () => {
    setupInvokeMock();

    render(<App />);

    await screen.findByTestId("home-resume-screen");
    await userEvent.click(screen.getByTestId("continue-task-button"));
    await screen.findByTestId("workspace-screen");

    await userEvent.click(screen.getByTestId("privacy-center-open"));
    await screen.findByTestId("privacy-center-panel");

    const clearAllButton = screen.getByTestId("privacy-clear-all-data");
    expect(clearAllButton).toBeDisabled();

    await userEvent.type(screen.getByTestId("privacy-clear-all-confirmation"), "CLEAR JEFF");
    expect(clearAllButton).not.toBeDisabled();
    await userEvent.click(clearAllButton);

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("clear_all_jeff_data"));
  });

  it("hides workspace soft prompt when folder is configured", async () => {
    setupInvokeMock();

    render(<App />);

    await screen.findByTestId("home-resume-screen");
    await userEvent.click(screen.getByTestId("continue-task-button"));
    await screen.findByTestId("workspace-screen");

    expect(screen.queryByTestId("companion-workspace-soft-prompt")).not.toBeInTheDocument();
  });

  it("persists workspace prompt dismissal to backend when skip is clicked", async () => {
    const mocks = setupInvokeMock();
    mocks.setOnboardingStatus({
      onboarding_complete: true,
      has_stored_api_key: true,
      api_key_source: "keychain",
      preferred_workspace_folder: null
    });

    render(<App />);

    await screen.findByTestId("home-resume-screen");
    await userEvent.click(screen.getByTestId("continue-task-button"));
    await screen.findByTestId("workspace-screen");

    const prompt = await screen.findByTestId("companion-workspace-soft-prompt");
    await userEvent.click(within(prompt).getByRole("button", { name: /skip for now/i }));

    await waitFor(() =>
      expect(screen.queryByTestId("companion-workspace-soft-prompt")).not.toBeInTheDocument()
    );
    expect(invokeMock).toHaveBeenCalledWith("set_workspace_prompt_dismissed", { dismissed: true });
  });

  it("shows API key error banner with Update API key CTA on auth failure", async () => {
    setupInvokeMock({ failCommands: { send_message: "OPENAI_API_KEY is not configured" } });

    render(<App />);

    await screen.findByTestId("home-resume-screen");
    await userEvent.click(screen.getByTestId("continue-task-button"));
    await screen.findByTestId("workspace-screen");

    const chatInput = screen.getByLabelText(/chat input/i);
    await userEvent.type(chatInput, "hello");
    await userEvent.click(screen.getByRole("button", { name: /^send$/i }));

    const banner = await screen.findByTestId("jeff-error-banner");
    expect(banner).toHaveTextContent("API key");
    expect(within(banner).getByTestId("jeff-error-fix-api-key")).toBeInTheDocument();
  });

  it("Update API key CTA calls openOnboardingAtStep(2) to land on key setup directly", async () => {
    setupInvokeMock({ failCommands: { send_message: "OPENAI_API_KEY is not configured" } });

    render(<App />);

    await screen.findByTestId("home-resume-screen");
    await userEvent.click(screen.getByTestId("continue-task-button"));
    await screen.findByTestId("workspace-screen");

    await userEvent.type(screen.getByLabelText(/chat input/i), "hello");
    await userEvent.click(screen.getByRole("button", { name: /^send$/i }));

    const fixButton = await screen.findByTestId("jeff-error-fix-api-key");
    await userEvent.click(fixButton);

    expect(invokeMock).toHaveBeenCalledWith(
      "ambient_open_onboarding_at_step",
      expect.objectContaining({ step: 2 })
    );
  });

  it("workspace soft prompt does not appear when prompt was previously dismissed", async () => {
    const mocks = setupInvokeMock();
    mocks.setOnboardingStatus({
      onboarding_complete: true,
      has_stored_api_key: true,
      api_key_source: "keychain",
      preferred_workspace_folder: null
    });
    mocks.setWorkspacePromptDismissed(true);

    render(<App />);

    await screen.findByTestId("home-resume-screen");
    await userEvent.click(screen.getByTestId("continue-task-button"));
    await screen.findByTestId("workspace-screen");

    // wait for initial load to complete before asserting absence
    await screen.findByTestId("companion-context-header");
    expect(screen.queryByTestId("companion-workspace-soft-prompt")).not.toBeInTheDocument();
  });
});
