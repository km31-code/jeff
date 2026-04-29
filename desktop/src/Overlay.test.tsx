import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import Overlay from "./Overlay";

const invokeMock = vi.fn();
const openDialogMock = vi.fn();
const eventHandlers = new Map<string, Set<(event: { payload: unknown }) => void>>();
let streamingEnabled = false;

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

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: (...args: unknown[]) => openDialogMock(...args)
}));

vi.mock("./streamClient", () => ({
  isStreamingEnabled: () => streamingEnabled,
  EVENT_LLM_TOKEN: "stream://llm_token",
  EVENT_LLM_COMPLETE: "stream://llm_complete",
  EVENT_TURN_CANCELLED: "stream://turn_cancelled",
  EVENT_TURN_COMPLETE: "stream://turn_complete",
  EVENT_TTS_CHUNK: "stream://tts_chunk"
}));

afterEach(() => {
  cleanup();
  invokeMock.mockReset();
  openDialogMock.mockReset();
  eventHandlers.clear();
  streamingEnabled = false;
});

function emitEvent(eventName: string, payload: unknown) {
  eventHandlers.get(eventName)?.forEach((handler) => handler({ payload }));
}

type OverlayMockOptions = {
  onboardingComplete?: boolean;
  hasStoredKey?: boolean;
  preferredWorkspaceFolder?: string | null;
  activeTask?: { id: number; title: string } | null;
  tasks?: Array<{ id: number; title: string; is_active?: boolean }>;
  subtasks?: Array<{ subtask_id: number; task_id: number; title: string; status: string }>;
  fileWriteProposals?: Array<{
    id: number;
    subtask_id: number;
    step_id: number | null;
    task_id: number;
    proposed_path: string;
    proposed_content: string;
    status: string;
    proposed_at: string;
    resolved_at: string | null;
  }>;
  accessibilityGranted?: boolean;
};

function setupInvokeMock(options: OverlayMockOptions = {}) {
  const onboardingComplete = options.onboardingComplete ?? false;
  let hasStoredKey = options.hasStoredKey ?? false;
  let preferredWorkspaceFolder = options.preferredWorkspaceFolder ?? null;
  let activeTask = options.activeTask ?? null;
  const toTaskDto = (task: { id: number; title: string; is_active?: boolean }) => ({
    id: task.id,
    title: task.title,
    slug: `task-${task.id}`,
    workspace_path: `/tmp/task-${task.id}`,
    created_at: "2026-04-22T00:00:00Z",
    updated_at: "2026-04-22T00:00:00Z",
    is_active: task.is_active ?? task.id === activeTask?.id
  });
  const taskRows =
    options.tasks?.map(toTaskDto) ??
    (activeTask
      ? [
          toTaskDto({
            id: activeTask.id,
            title: activeTask.title,
            is_active: true
          })
        ]
      : []);
  let subtasks = options.subtasks ?? [];
  let fileWriteProposals = options.fileWriteProposals ?? [];

  invokeMock.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
    if (command === "ambient_get_state") {
      return {
        tray_status: "idle",
        overlay_mode: "expanded",
        overlay_visible: true,
        hotkey: "CmdOrCtrl+Shift+J",
        hotkey_registered: true,
        notification_permission: "granted",
        single_instance: true,
        quiet_mode: false
      };
    }

    if (command === "set_overlay_mode" || command === "ambient_set_overlay_mode") {
      return {
        tray_status: "idle",
        overlay_mode: String(args?.mode ?? "expanded"),
        overlay_visible: true,
        hotkey: "CmdOrCtrl+Shift+J",
        hotkey_registered: true,
        notification_permission: "granted",
        single_instance: true,
        quiet_mode: false
      };
    }

    if (command === "get_active_task") {
      if (!activeTask) {
        return null;
      }
      return toTaskDto({ ...activeTask, is_active: true });
    }

    if (command === "list_messages") {
      return [];
    }

    if (command === "list_tasks") {
      return taskRows.map((task) => ({
        ...task,
        is_active: task.id === activeTask?.id
      }));
    }

    if (command === "set_active_task") {
      const taskId = Number(args?.taskId);
      const next = taskRows.find((task) => task.id === taskId);
      if (!next) {
        throw new Error(`unknown task ${taskId}`);
      }
      activeTask = { id: next.id, title: next.title };
      return { ...next, is_active: true };
    }

    if (command === "get_active_window_context") {
      return null;
    }

    if (command === "get_watcher_status") {
      return { task_id: activeTask?.id ?? 1, is_watching: false, watched_path: null };
    }

    if (command === "get_accessibility_permission_status") {
      return options.accessibilityGranted ?? true;
    }

    if (command === "request_accessibility_permission") {
      return null;
    }

    if (command === "mark_notification_permission") {
      return {
        tray_status: "idle",
        overlay_mode: "expanded",
        overlay_visible: true,
        hotkey: "CmdOrCtrl+Shift+J",
        hotkey_registered: true,
        notification_permission: "granted",
        single_instance: true,
        quiet_mode: false
      };
    }

    if (command === "get_onboarding_status") {
      return {
        onboarding_complete: onboardingComplete,
        has_stored_api_key: hasStoredKey,
        api_key_source: hasStoredKey ? "keychain" : "none",
        preferred_workspace_folder: preferredWorkspaceFolder
      };
    }

    if (command === "validate_openai_api_key") {
      const apiKey = String(args?.apiKey ?? "").trim();
      if (!apiKey || !apiKey.startsWith("sk-")) {
        return {
          is_valid: false,
          message: "Your API key isn't working. Open settings to update it."
        };
      }
      return {
        is_valid: true,
        message: "API key validated successfully."
      };
    }

    if (command === "store_openai_api_key") {
      hasStoredKey = true;
      return null;
    }

    if (command === "set_preferred_workspace_folder") {
      preferredWorkspaceFolder = String(args?.folderPath ?? "");
      return null;
    }

    if (command === "clear_preferred_workspace_folder") {
      preferredWorkspaceFolder = null;
      return null;
    }

    if (command === "complete_onboarding") {
      return null;
    }

    if (command === "send_message") {
      return {
        assistant_response: "done",
        retrieved_chunks: [],
        cancelled: false
      };
    }

    if (command === "send_message_streaming") {
      return "turn-1";
    }

    if (command === "cancel_streaming_turn") {
      return true;
    }

    if (
      command === "ambient_set_tray_status" ||
      command === "ambient_set_quiet_mode" ||
      command === "ambient_notification_clicked" ||
      command === "ambient_hide_overlay" ||
      command === "ambient_set_workspace_mode"
    ) {
      return null;
    }

    if (command === "list_subtasks") {
      const taskId = Number(args?.taskId);
      return subtasks
        .filter((subtask) => subtask.task_id === taskId)
        .map((subtask) => ({
          description: "",
          execution_type: "draft_generation",
          result_review_status: "unreviewed",
          created_at: "2026-04-22T00:00:00Z",
          updated_at: "2026-04-22T00:00:00Z",
          result_summary: null,
          result_payload: null,
          instruction_source: "system",
          parent_context_snapshot: "{}",
          error_message: null,
          ...subtask
        }));
    }

    if (command === "cancel_subtask") {
      const subtaskId = Number(args?.subtaskId);
      subtasks = subtasks.map((subtask) =>
        subtask.subtask_id === subtaskId ? { ...subtask, status: "cancelled" } : subtask
      );
      return subtasks.find((subtask) => subtask.subtask_id === subtaskId) ?? null;
    }

    if (command === "list_file_write_proposals") {
      const taskId = Number(args?.taskId);
      return fileWriteProposals.filter((proposal) => proposal.task_id === taskId);
    }

    if (command === "approve_subtask_file_write" || command === "reject_subtask_file_write") {
      const proposalId = Number(args?.proposalId);
      const action = command === "approve_subtask_file_write" ? "approved" : "rejected";
      fileWriteProposals = fileWriteProposals.map((proposal) =>
        proposal.id === proposalId ? { ...proposal, status: action } : proposal
      );
      return {
        id: 1,
        task_id: Number(args?.taskId),
        subtask_id: 101,
        proposal_id: proposalId,
        action,
        proposed_path: "notes.md",
        resolved_at: "2026-04-22T00:00:00Z"
      };
    }

    if (command === "accept_subtask_result" || command === "reject_subtask_result") {
      return null;
    }

    if (command === "dismiss_proactive_trigger") {
      return null;
    }

    throw new Error(`unexpected command ${command}`);
  });
}

describe("Overlay onboarding", () => {
  it("renders the five-step onboarding wizard and completes it", async () => {
    const user = userEvent.setup();
    openDialogMock.mockResolvedValue("/Users/tester/Documents");
    setupInvokeMock({ onboardingComplete: false, activeTask: null });

    render(<Overlay onOpenWorkspace={() => undefined} />);

    // step 1: intro
    await screen.findByTestId("onboarding-step-1");
    expect(screen.getByTestId("overlay-onboarding-step-count")).toHaveTextContent("Step 1 of 5");
    await user.click(screen.getByTestId("onboarding-continue-step-1"));

    // step 2: api key
    await screen.findByTestId("onboarding-step-2");
    expect(screen.getByTestId("overlay-onboarding-step-count")).toHaveTextContent("Step 2 of 5");
    await user.type(screen.getByTestId("onboarding-api-key-input"), "sk-test-key");
    await user.click(screen.getByTestId("onboarding-continue-step-2"));

    // step 3: workspace folder
    await screen.findByTestId("onboarding-step-3");
    expect(screen.getByTestId("overlay-onboarding-step-count")).toHaveTextContent("Step 3 of 5");
    await user.click(screen.getByTestId("onboarding-choose-folder"));
    await waitFor(() =>
      expect(screen.getByTestId("onboarding-workspace-selection").textContent).toContain(
        "/Users/tester/Documents"
      )
    );
    await user.click(screen.getByTestId("onboarding-continue-step-3"));

    // step 4: accessibility permission (mock returns granted by default)
    await screen.findByTestId("onboarding-step-4");
    expect(screen.getByTestId("overlay-onboarding-step-count")).toHaveTextContent("Step 4 of 5");
    await screen.findByTestId("onboarding-accessibility-granted");
    await user.click(screen.getByTestId("onboarding-continue-step-4"));

    // step 5: ready
    await screen.findByTestId("onboarding-step-5");
    expect(screen.getByTestId("overlay-onboarding-step-count")).toHaveTextContent("Step 5 of 5");
    await user.click(screen.getByTestId("onboarding-complete"));

    await waitFor(() => {
      expect(screen.queryByTestId("overlay-onboarding")).not.toBeInTheDocument();
    });

    expect(invokeMock).toHaveBeenCalledWith("complete_onboarding");
  });

  it("can cancel onboarding without marking completion", async () => {
    const user = userEvent.setup();
    setupInvokeMock({ onboardingComplete: false, activeTask: null });

    render(<Overlay onOpenWorkspace={() => undefined} />);

    await screen.findByTestId("onboarding-step-1");
    await user.click(screen.getByRole("button", { name: "Cancel" }));

    await waitFor(() => {
      expect(screen.queryByTestId("overlay-onboarding")).not.toBeInTheDocument();
    });

    expect(invokeMock).not.toHaveBeenCalledWith("complete_onboarding");
  });

  it("shows no-active-task prompt when onboarding is complete", async () => {
    setupInvokeMock({ onboardingComplete: true, activeTask: null });

    render(<Overlay onOpenWorkspace={() => undefined} />);

    expect(await screen.findByTestId("overlay-no-active-task")).toHaveTextContent(
      "Tell me what you're working on."
    );
  });

  it("focuses the message input after an interactive overlay summon", async () => {
    setupInvokeMock({
      onboardingComplete: true,
      activeTask: { id: 7, title: "runtime smoke task" }
    });

    render(<Overlay onOpenWorkspace={() => undefined} />);

    const input = await screen.findByTestId("overlay-input");
    emitEvent("ambient://overlay-shown", { interactive: true });

    await waitFor(() => {
      expect(input).toHaveFocus();
    });
  });

  it("keeps streaming sends in working state until the stream completes", async () => {
    const user = userEvent.setup();
    streamingEnabled = true;
    setupInvokeMock({
      onboardingComplete: true,
      activeTask: { id: 7, title: "runtime smoke task" }
    });

    render(<Overlay onOpenWorkspace={() => undefined} />);

    await user.type(await screen.findByTestId("overlay-input"), "runtime ping");
    await user.click(screen.getByRole("button", { name: "send" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("send_message_streaming", {
        taskId: 7,
        message: "runtime ping",
        source: "text"
      });
    });

    expect(invokeMock).toHaveBeenCalledWith("ambient_set_tray_status", {
      status: "working"
    });
    expect(invokeMock).not.toHaveBeenCalledWith("ambient_set_tray_status", {
      status: "idle"
    });

    emitEvent("stream://llm_complete", {
      turn_id: "turn-1",
      full_text: "pong",
      cancelled: false,
      ttft_ms: 10,
      total_ms: 20
    });

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("ambient_set_tray_status", {
        status: "idle"
      });
      expect(screen.getByTestId("overlay-input")).not.toBeDisabled();
    });
  });

  it("requests accessibility permission only after explicit click", async () => {
    const user = userEvent.setup();
    setupInvokeMock({
      onboardingComplete: true,
      activeTask: { id: 1, title: "history storymap" },
      accessibilityGranted: false
    });

    render(<Overlay onOpenWorkspace={() => undefined} />);

    expect(await screen.findByTestId("accessibility-context-prompt")).toHaveTextContent(
      "Jeff needs accessibility permission to know which document you have open."
    );
    expect(invokeMock).not.toHaveBeenCalledWith("request_accessibility_permission");

    await user.click(screen.getByTestId("request-accessibility-permission"));
    expect(invokeMock).toHaveBeenCalledWith("request_accessibility_permission");
  });

  it("switches tasks inline without showing an open workspace button", async () => {
    const user = userEvent.setup();
    setupInvokeMock({
      onboardingComplete: true,
      activeTask: { id: 1, title: "history storymap" },
      tasks: [
        { id: 1, title: "history storymap", is_active: true },
        { id: 2, title: "physics outline" },
        { id: 3, title: "literature review" }
      ]
    });

    render(<Overlay onOpenWorkspace={() => undefined} />);

    await screen.findByText("history storymap");
    expect(screen.queryByRole("button", { name: /open full workspace/i })).not.toBeInTheDocument();

    await user.click(screen.getByTestId("overlay-task-switcher"));
    expect(await screen.findByTestId("overlay-task-menu")).toHaveTextContent("physics outline");

    await user.click(screen.getByTestId("overlay-task-option-2"));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_active_task", { taskId: 2 });
      expect(screen.getByText("physics outline")).toBeInTheDocument();
    });
  });

  it("restores and cancels a running companion subtask", async () => {
    const user = userEvent.setup();
    setupInvokeMock({
      onboardingComplete: true,
      activeTask: { id: 7, title: "runtime smoke task" },
      subtasks: [
        {
          subtask_id: 101,
          task_id: 7,
          title: "draft intro in parallel",
          status: "running"
        }
      ]
    });

    render(<Overlay onOpenWorkspace={() => undefined} />);

    expect(await screen.findByTestId("overlay-active-subtask")).toHaveTextContent(
      "jeff is working on: draft intro in parallel"
    );

    await user.click(screen.getByTestId("overlay-cancel-subtask"));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("cancel_subtask", { subtaskId: 101 });
      expect(screen.queryByTestId("overlay-active-subtask")).not.toBeInTheDocument();
    });
  });

  it("shows companion subtask events and pending file write approvals", async () => {
    const user = userEvent.setup();
    setupInvokeMock({
      onboardingComplete: true,
      activeTask: { id: 7, title: "runtime smoke task" }
    });

    render(<Overlay onOpenWorkspace={() => undefined} />);

    await screen.findByTestId("overlay-input");
    emitEvent("subtask://companion-started", {
      subtask_id: 202,
      task_id: 7,
      title: "outline citations"
    });

    expect(await screen.findByTestId("overlay-active-subtask")).toHaveTextContent(
      "outline citations"
    );

    emitEvent("subtask://companion-write-proposal", {
      id: 303,
      subtask_id: 202,
      step_id: 1,
      task_id: 7,
      proposed_path: "/tmp/runtime-smoke/notes.md",
      proposed_content: "These are the first proposed notes for the draft introduction.",
      status: "pending_approval",
      proposed_at: "2026-04-22T00:00:00Z",
      resolved_at: null
    });

    expect(await screen.findByTestId("overlay-file-write-proposal")).toHaveTextContent("notes.md");
    await user.click(screen.getByTestId("overlay-file-write-approve-303"));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("approve_subtask_file_write", {
        taskId: 7,
        proposalId: 303
      });
      expect(screen.queryByTestId("overlay-file-write-proposal")).not.toBeInTheDocument();
      expect(screen.getByTestId("overlay-file-written-confirmation")).toHaveTextContent(
        "notes.md written"
      );
    });

    emitEvent("subtask://companion-complete", {
      subtask_id: 202,
      task_id: 7,
      final_status: "completed"
    });

    await waitFor(() => {
      expect(screen.queryByTestId("overlay-active-subtask")).not.toBeInTheDocument();
    });
  });
});
