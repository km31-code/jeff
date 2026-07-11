import { invoke } from "@tauri-apps/api/core";

// phase 11 ambient-presence client. kept separate from tauriClient.ts so the
// ambient surface can evolve without touching the phase 1-10 ipc contract.

export type TrayStatus = "idle" | "listening" | "working";
export type OverlayMode = "collapsed" | "expanded" | "workspace";

export interface AmbientStateDto {
  tray_status: TrayStatus;
  overlay_mode: OverlayMode;
  overlay_visible: boolean;
  hotkey: string;
  hotkey_registered: boolean;
  notification_permission: string;
  single_instance: boolean;
  quiet_mode: boolean;
  wake_word_armed: boolean;
}

export interface NotificationPayload {
  title: string;
  body: string;
  context_kind?: string | null;
  context_id?: number | null;
}

export function isOverlayWindow(): boolean {
  return window.location.hash === "#overlay";
}

export function toggleOverlay(): Promise<void> {
  return invoke("ambient_toggle_overlay");
}

export function showOverlay(): Promise<void> {
  return invoke("ambient_show_overlay");
}

export function hideOverlay(): Promise<void> {
  return invoke("ambient_hide_overlay");
}

export function setWorkspaceMode(open: boolean): Promise<void> {
  return invoke("ambient_set_workspace_mode", { open });
}

export function openPrivacyCenter(): Promise<void> {
  return invoke("ambient_open_privacy_center");
}

export function openOnboarding(): Promise<void> {
  return invoke("ambient_open_onboarding");
}

// opens the onboarding wizard at a specific step. use step=2 when recovering
// from an API key error so the user lands directly on key setup.
export function openOnboardingAtStep(step: number): Promise<void> {
  return invoke("ambient_open_onboarding_at_step", { step });
}

export function setOverlayMode(mode: OverlayMode): Promise<AmbientStateDto> {
  return invoke("ambient_set_overlay_mode", { mode });
}

export function setTrayStatus(status: TrayStatus): Promise<AmbientStateDto> {
  return invoke("ambient_set_tray_status", { status });
}

export function setQuietMode(quiet: boolean): Promise<AmbientStateDto> {
  return invoke("ambient_set_quiet_mode", { quiet });
}

export function getAmbientState(): Promise<AmbientStateDto> {
  return invoke("ambient_get_state");
}

export function quitApp(): Promise<void> {
  return invoke("ambient_quit_app");
}

export function sendNotification(payload: NotificationPayload): Promise<void> {
  return invoke("ambient_notify", { payload });
}

export function markNotificationPermission(
  permission: string
): Promise<AmbientStateDto> {
  return invoke("ambient_mark_notification_permission", { permission });
}

export function reportNotificationClicked(
  contextKind: string | null,
  contextId: number | null
): Promise<AmbientStateDto> {
  return invoke("ambient_notification_clicked", {
    contextKind,
    contextId
  });
}
