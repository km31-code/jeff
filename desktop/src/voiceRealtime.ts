// apex c4: browser side of the realtime voice session. jeff's backend mints an
// ephemeral client secret (start_voice_session); this opens a WebRTC connection
// directly to OpenAI Realtime with that secret, streams the microphone, plays
// model audio, and reports finalized transcripts + tool-calls back.
//
// this is the env-gated part of c4: it requires a real browser with WebRTC and
// a microphone, and a valid ephemeral secret. when those are unavailable (tests,
// unsupported webview, no secret) it degrades gracefully to null so the caller
// falls back to the STT/TTS pipeline.

const REALTIME_BASE_URL = "https://api.openai.com/v1/realtime";

export interface RealtimeCallbacks {
  onTranscript?: (role: "user" | "assistant", text: string) => void;
  onToolCall?: (name: string, args: Record<string, unknown>) => void;
  onStateChange?: (state: "connecting" | "live" | "closed" | "error") => void;
}

export interface RealtimeConnection {
  close: () => void;
  setMuted: (muted: boolean) => void;
}

// whether this environment can run a realtime WebRTC voice session at all.
export function realtimeVoiceSupported(): boolean {
  return (
    typeof RTCPeerConnection !== "undefined" &&
    typeof navigator !== "undefined" &&
    !!navigator.mediaDevices &&
    typeof navigator.mediaDevices.getUserMedia === "function"
  );
}

// open a realtime voice connection. returns null when unsupported or on failure,
// signalling the caller to fall back to the pipeline.
export async function connectRealtimeVoice(
  clientSecret: string,
  model: string,
  callbacks: RealtimeCallbacks
): Promise<RealtimeConnection | null> {
  if (!realtimeVoiceSupported() || !clientSecret) {
    return null;
  }
  try {
    callbacks.onStateChange?.("connecting");
    const pc = new RTCPeerConnection();

    // remote audio playback.
    const audioEl = new Audio();
    audioEl.autoplay = true;
    pc.ontrack = (event) => {
      audioEl.srcObject = event.streams[0];
    };

    // microphone capture.
    const mic = await navigator.mediaDevices.getUserMedia({ audio: true });
    const micTrack = mic.getAudioTracks()[0];
    pc.addTrack(micTrack, mic);

    // data channel for events (transcripts + tool-calls).
    const channel = pc.createDataChannel("oai-events");
    channel.onmessage = (event) => handleRealtimeEvent(event.data, callbacks);

    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);

    const response = await fetch(`${REALTIME_BASE_URL}?model=${encodeURIComponent(model)}`, {
      method: "POST",
      body: offer.sdp,
      headers: {
        Authorization: `Bearer ${clientSecret}`,
        "Content-Type": "application/sdp"
      }
    });
    if (!response.ok) {
      pc.close();
      callbacks.onStateChange?.("error");
      return null;
    }
    const answer = { type: "answer" as const, sdp: await response.text() };
    await pc.setRemoteDescription(answer);
    callbacks.onStateChange?.("live");

    return {
      close: () => {
        micTrack.stop();
        channel.close();
        pc.close();
        callbacks.onStateChange?.("closed");
      },
      setMuted: (muted: boolean) => {
        micTrack.enabled = !muted;
      }
    };
  } catch {
    callbacks.onStateChange?.("error");
    return null;
  }
}

// parse a realtime server event and surface transcripts / tool-calls. exported
// for testing without a live socket.
export function handleRealtimeEvent(raw: unknown, callbacks: RealtimeCallbacks): void {
  let event: Record<string, unknown>;
  try {
    event = typeof raw === "string" ? JSON.parse(raw) : (raw as Record<string, unknown>);
  } catch {
    return;
  }
  const type = event.type as string | undefined;
  if (!type) return;

  if (type === "conversation.item.input_audio_transcription.completed") {
    const text = (event.transcript as string) ?? "";
    if (text.trim()) callbacks.onTranscript?.("user", text.trim());
    return;
  }
  if (type === "response.audio_transcript.done") {
    const text = (event.transcript as string) ?? "";
    if (text.trim()) callbacks.onTranscript?.("assistant", text.trim());
    return;
  }
  if (type === "response.function_call_arguments.done") {
    const name = (event.name as string) ?? "";
    let args: Record<string, unknown> = {};
    try {
      args = JSON.parse((event.arguments as string) ?? "{}");
    } catch {
      args = {};
    }
    if (name) callbacks.onToolCall?.(name, args);
  }
}
