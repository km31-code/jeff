// phase 12: typed event listener helpers for stream:// Tauri events.
// mirrors the payload structs in src-tauri/src/streaming.rs.

import { listen, UnlistenFn } from "@tauri-apps/api/event";

export type TurnId = string;

export interface LlmTokenPayload {
  turn_id: TurnId;
  delta: string;
  index: number;
}

export interface LlmCompletePayload {
  turn_id: TurnId;
  full_text: string;
  cancelled: boolean;
  ttft_ms: number | null;
  total_ms: number;
}

export interface TtsChunkPayload {
  turn_id: TurnId;
  phrase_id: number;
  audio_b64: string;
  first_audio_ms: number;
}

export interface TurnCancelledPayload {
  turn_id: TurnId;
  reason: string;
  partial_text: string;
  elapsed_ms: number;
}

export interface TurnCompletePayload {
  turn_id: TurnId;
  duration_ms: number;
  ttft_ms: number | null;
  first_audio_ms: number | null;
}

// event name constants. must match streaming.rs EVENT_* constants.
export const EVENT_LLM_TOKEN = "stream://llm_token";
export const EVENT_LLM_COMPLETE = "stream://llm_complete";
export const EVENT_TTS_CHUNK = "stream://tts_chunk";
export const EVENT_TURN_CANCELLED = "stream://turn_cancelled";
export const EVENT_TURN_COMPLETE = "stream://turn_complete";

// typed wrappers around listen() for all stream events.

export function onLlmToken(
  handler: (payload: LlmTokenPayload) => void
): Promise<UnlistenFn> {
  return listen<LlmTokenPayload>(EVENT_LLM_TOKEN, (event) =>
    handler(event.payload)
  );
}

export function onLlmComplete(
  handler: (payload: LlmCompletePayload) => void
): Promise<UnlistenFn> {
  return listen<LlmCompletePayload>(EVENT_LLM_COMPLETE, (event) =>
    handler(event.payload)
  );
}

export function onTtsChunk(
  handler: (payload: TtsChunkPayload) => void
): Promise<UnlistenFn> {
  return listen<TtsChunkPayload>(EVENT_TTS_CHUNK, (event) =>
    handler(event.payload)
  );
}

export function onTurnCancelled(
  handler: (payload: TurnCancelledPayload) => void
): Promise<UnlistenFn> {
  return listen<TurnCancelledPayload>(EVENT_TURN_CANCELLED, (event) =>
    handler(event.payload)
  );
}

export function onTurnComplete(
  handler: (payload: TurnCompletePayload) => void
): Promise<UnlistenFn> {
  return listen<TurnCompletePayload>(EVENT_TURN_COMPLETE, (event) =>
    handler(event.payload)
  );
}

// checks whether the VITE_JEFF_STREAMING feature flag is active.
// defaults to true when the env var is not set (streaming is the default
// in dev), and can be explicitly disabled with VITE_JEFF_STREAMING=0.
export function isStreamingEnabled(): boolean {
  if (typeof import.meta === "undefined") return false;
  const val = (import.meta.env as Record<string, string | undefined>)
    .VITE_JEFF_STREAMING;
  return val !== "0" && val !== "false";
}
