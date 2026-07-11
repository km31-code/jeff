// apex c4: browser side of the realtime voice session. Jeff's backend mints an
// ephemeral client secret (start_voice_session); this opens a WebRTC call
// directly to OpenAI, streams the microphone, plays model audio, and reports
// finalized transcripts + tool calls back through Jeff's normal command path.
//
// Every terminal path owns the same cleanup routine. Returning null means the
// caller can safely start the existing STT/TTS pipeline without leaving a live
// microphone, peer connection, data channel, audio element, or fetch behind.

const REALTIME_CALLS_URL = "https://api.openai.com/v1/realtime/calls";
const CONNECT_TIMEOUT_MS = 15_000;
const DISCONNECTED_GRACE_MS = 2_000;

export type RealtimeToolResult = unknown | Promise<unknown>;

export interface RealtimeCallbacks {
  onTranscript?: (role: "user" | "assistant", text: string) => void;
  onToolCall?: (
    name: string,
    args: Record<string, unknown>,
    callId: string
  ) => RealtimeToolResult;
  onStateChange?: (state: "connecting" | "live" | "closed" | "error") => void;
}

export interface RealtimeConnection {
  close: () => void;
  setMuted: (muted: boolean) => void;
  sendContext: (instructions: string) => boolean;
}

type RealtimeSender = (event: Record<string, unknown>) => boolean;

// Whether this environment can run a realtime WebRTC voice session at all.
export function realtimeVoiceSupported(): boolean {
  return (
    typeof RTCPeerConnection !== "undefined" &&
    typeof navigator !== "undefined" &&
    !!navigator.mediaDevices &&
    typeof navigator.mediaDevices.getUserMedia === "function" &&
    typeof FormData !== "undefined"
  );
}

function serializeToolOutput(value: unknown): string {
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value ?? null);
  } catch {
    return JSON.stringify({ ok: false, error: "tool result was not serializable" });
  }
}

// Open a realtime voice connection. The current WebRTC contract uses a
// multipart POST to /v1/realtime/calls with separate SDP and session parts.
export async function connectRealtimeVoice(
  clientSecret: string,
  model: string,
  callbacks: RealtimeCallbacks
): Promise<RealtimeConnection | null> {
  if (!realtimeVoiceSupported() || !clientSecret.trim() || !model.trim()) {
    return null;
  }

  callbacks.onStateChange?.("connecting");
  const pc = new RTCPeerConnection();
  const audioEl = new Audio();
  const abortController = new AbortController();
  const handledToolCalls = new Set<string>();
  let microphone: MediaStream | null = null;
  let channel: RTCDataChannel | null = null;
  let timeoutId: ReturnType<typeof setTimeout> | null = null;
  let disconnectId: ReturnType<typeof setTimeout> | null = null;
  let terminal = false;

  const clearTimers = () => {
    if (timeoutId !== null) clearTimeout(timeoutId);
    if (disconnectId !== null) clearTimeout(disconnectId);
    timeoutId = null;
    disconnectId = null;
  };

  const cleanup = () => {
    clearTimers();
    abortController.abort();
    microphone?.getTracks().forEach((track) => track.stop());
    microphone = null;
    if (channel && channel.readyState !== "closed") channel.close();
    channel = null;
    pc.ontrack = null;
    pc.onconnectionstatechange = null;
    pc.oniceconnectionstatechange = null;
    if (pc.connectionState !== "closed") pc.close();
    audioEl.pause();
    audioEl.srcObject = null;
  };

  const finish = (state: "closed" | "error") => {
    if (terminal) return;
    terminal = true;
    cleanup();
    callbacks.onStateChange?.(state);
  };

  const sendEvent: RealtimeSender = (event) => {
    if (!channel || channel.readyState !== "open" || terminal) return false;
    try {
      channel.send(JSON.stringify(event));
      return true;
    } catch {
      finish("error");
      return false;
    }
  };

  try {
    audioEl.autoplay = true;
    pc.ontrack = (event) => {
      const [stream] = event.streams;
      if (stream) audioEl.srcObject = stream;
    };

    pc.onconnectionstatechange = () => {
      if (pc.connectionState === "failed") {
        finish("error");
      } else if (pc.connectionState === "closed") {
        finish("closed");
      } else if (pc.connectionState === "disconnected" && disconnectId === null) {
        disconnectId = setTimeout(() => {
          disconnectId = null;
          if (pc.connectionState === "disconnected") finish("error");
        }, DISCONNECTED_GRACE_MS);
      } else if (pc.connectionState === "connected" && disconnectId !== null) {
        clearTimeout(disconnectId);
        disconnectId = null;
      }
    };
    pc.oniceconnectionstatechange = () => {
      if (pc.iceConnectionState === "failed") finish("error");
    };

    microphone = await navigator.mediaDevices.getUserMedia({ audio: true });
    if (terminal) return null;
    const audioTracks = microphone.getAudioTracks();
    if (audioTracks.length === 0) throw new Error("microphone returned no audio track");
    for (const track of audioTracks) pc.addTrack(track, microphone);

    channel = pc.createDataChannel("oai-events");
    channel.onmessage = (event) => {
      void handleRealtimeEvent(event.data, callbacks, sendEvent, handledToolCalls);
    };
    channel.onerror = () => finish("error");
    channel.onclose = () => {
      if (!terminal && pc.connectionState !== "closed") finish("error");
    };

    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);
    if (!offer.sdp) throw new Error("realtime offer did not contain SDP");

    const body = new FormData();
    body.append("sdp", new Blob([offer.sdp], { type: "application/sdp" }), "offer.sdp");
    body.append(
      "session",
      new Blob([JSON.stringify({ type: "realtime", model })], {
        type: "application/json"
      }),
      "session.json"
    );
    timeoutId = setTimeout(() => abortController.abort(), CONNECT_TIMEOUT_MS);
    const response = await fetch(REALTIME_CALLS_URL, {
      method: "POST",
      body,
      headers: { Authorization: `Bearer ${clientSecret}` },
      signal: abortController.signal
    });
    if (timeoutId !== null) clearTimeout(timeoutId);
    timeoutId = null;
    if (!response.ok) throw new Error(`realtime call failed (${response.status})`);
    const answerSdp = await response.text();
    if (!answerSdp.trim()) throw new Error("realtime call returned empty SDP");
    await pc.setRemoteDescription({ type: "answer", sdp: answerSdp });
    if (terminal) return null;

    callbacks.onStateChange?.("live");
    return {
      close: () => finish("closed"),
      setMuted: (muted: boolean) => {
        microphone?.getAudioTracks().forEach((track) => {
          track.enabled = !muted;
        });
      },
      sendContext: (instructions: string) => {
        const clean = instructions.trim();
        if (!clean) return false;
        return sendEvent({
          type: "session.update",
          session: { type: "realtime", instructions: clean }
        });
      }
    };
  } catch {
    finish("error");
    return null;
  }
}

// Parse one Realtime server event and surface finalized transcripts/tool calls.
// For a function call we always answer with function_call_output (success or
// error) and then ask the model to continue the response.
export async function handleRealtimeEvent(
  raw: unknown,
  callbacks: RealtimeCallbacks,
  sendEvent?: RealtimeSender,
  handledToolCalls: Set<string> = new Set()
): Promise<void> {
  let event: Record<string, unknown>;
  try {
    event = typeof raw === "string" ? JSON.parse(raw) : (raw as Record<string, unknown>);
  } catch {
    return;
  }
  if (!event || typeof event !== "object") return;
  const type = typeof event.type === "string" ? event.type : "";

  if (type === "conversation.item.input_audio_transcription.completed") {
    const text = typeof event.transcript === "string" ? event.transcript.trim() : "";
    if (text) callbacks.onTranscript?.("user", text);
    return;
  }
  if (
    type === "response.output_audio_transcript.done" ||
    type === "response.audio_transcript.done"
  ) {
    const text = typeof event.transcript === "string" ? event.transcript.trim() : "";
    if (text) callbacks.onTranscript?.("assistant", text);
    return;
  }
  if (type !== "response.function_call_arguments.done") return;

  const name = typeof event.name === "string" ? event.name.trim() : "";
  const callId = typeof event.call_id === "string" ? event.call_id.trim() : "";
  if (!name || !callId || handledToolCalls.has(callId)) return;
  handledToolCalls.add(callId);

  let args: Record<string, unknown>;
  try {
    const parsed = JSON.parse(typeof event.arguments === "string" ? event.arguments : "{}");
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      throw new Error("tool arguments must be an object");
    }
    args = parsed as Record<string, unknown>;
  } catch {
    sendEvent?.({
      type: "conversation.item.create",
      item: {
        type: "function_call_output",
        call_id: callId,
        output: JSON.stringify({ ok: false, error: "invalid tool arguments" })
      }
    });
    sendEvent?.({ type: "response.create" });
    return;
  }

  let output: unknown;
  try {
    output = callbacks.onToolCall
      ? await callbacks.onToolCall(name, args, callId)
      : { ok: false, error: "tool routing is unavailable" };
  } catch (error) {
    output = { ok: false, error: error instanceof Error ? error.message : String(error) };
  }
  sendEvent?.({
    type: "conversation.item.create",
    item: {
      type: "function_call_output",
      call_id: callId,
      output: serializeToolOutput(output)
    }
  });
  sendEvent?.({ type: "response.create" });
}
