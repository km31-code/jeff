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
  accessibilityGranted?: boolean;
};

function setupInvokeMock(options: OverlayMockOptions = {}) {
  const onboardingComplete = options.onboardingComplete ?? false;
  let hasStoredKey = options.hasStoredKey ?? false;
  let preferredWorkspaceFolder = options.preferredWorkspaceFolder ?? null;
  const activeTask = options.activeTask ?? null;

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
      return {
        id: activeTask.id,
        title: activeTask.title,
        slug: "demo-task",
        workspace_path: "/tmp/demo",
        created_at: "2026-04-22T00:00:00Z",
        updated_at: "2026-04-22T00:00:00Z",
        is_active: true
      };
    }

    if (command === "list_messages") {
      return [];
    }

    if (command === "list_tasks") {
      if (!activeTask) {
        return [];
      }
      return [
        {
          id: activeTask.id,
          title: activeTask.title,
          slug: "demo-task",
          workspace_path: "/tmp/demo",
          created_at: "2026-04-22T00:00:00Z",
          updated_at: "2026-04-22T00:00:00Z",
          is_active: true
        }
      ];
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
      command === "ambient_show_workspace"
    ) {
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

    render(<Overlay />);

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

    render(<Overlay />);

    await screen.findByTestId("onboarding-step-1");
    await user.click(screen.getByRole("button", { name: "Cancel" }));

    await waitFor(() => {
      expect(screen.queryByTestId("overlay-onboarding")).not.toBeInTheDocument();
    });

    expect(invokeMock).not.toHaveBeenCalledWith("complete_onboarding");
  });

  it("shows no-active-task prompt when onboarding is complete", async () => {
    setupInvokeMock({ onboardingComplete: true, activeTask: null });

    render(<Overlay />);

    expect(await screen.findByTestId("overlay-no-active-task")).toHaveTextContent(
      "Tell me what you're working on."
    );
  });

  it("focuses the message input after an interactive overlay summon", async () => {
    setupInvokeMock({
      onboardingComplete: true,
      activeTask: { id: 7, title: "runtime smoke task" }
    });

    render(<Overlay />);

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

    render(<Overlay />);

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

    render(<Overlay />);

    expect(await screen.findByTestId("accessibility-context-prompt")).toHaveTextContent(
      "Jeff needs accessibility permission to know which document you have open."
    );
    expect(invokeMock).not.toHaveBeenCalledWith("request_accessibility_permission");

    await user.click(screen.getByTestId("request-accessibility-permission"));
    expect(invokeMock).toHaveBeenCalledWith("request_accessibility_permission");
  });
});
