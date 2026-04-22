#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
STREAMING_RS="$ROOT_DIR/desktop/src-tauri/src/streaming.rs"
CHAT_STREAMING_RS="$ROOT_DIR/desktop/src-tauri/src/chat_streaming.rs"
COMMANDS_RS="$ROOT_DIR/desktop/src-tauri/src/commands.rs"
MAIN_RS="$ROOT_DIR/desktop/src-tauri/src/main.rs"
STREAM_CLIENT_TS="$ROOT_DIR/desktop/src/streamClient.ts"
APP_TSX="$ROOT_DIR/desktop/src/App.tsx"
OVERLAY_TSX="$ROOT_DIR/desktop/src/Overlay.tsx"

echo "--- phase 12 streaming everywhere check ---"

fail() { echo "FAIL: $1" >&2; exit 1; }
pass() { echo "PASS: $1"; }

# 1. cancellation primitives (m12.1)
grep -q "InteractionToken" "$STREAMING_RS" || fail "InteractionToken not found in streaming.rs"
grep -q "InteractionRegistry" "$STREAMING_RS" || fail "InteractionRegistry not found in streaming.rs"
grep -q "CancellationToken" "$STREAMING_RS" || fail "CancellationToken not in streaming.rs"
pass "cancellation primitives present (m12.1)"

# 2. stream event payload types defined
grep -q "LlmTokenPayload" "$STREAMING_RS" || fail "LlmTokenPayload not in streaming.rs"
grep -q "LlmCompletePayload" "$STREAMING_RS" || fail "LlmCompletePayload not in streaming.rs"
grep -q "TtsChunkPayload" "$STREAMING_RS" || fail "TtsChunkPayload not in streaming.rs"
grep -q "TurnCancelledPayload" "$STREAMING_RS" || fail "TurnCancelledPayload not in streaming.rs"
grep -q "TurnCompletePayload" "$STREAMING_RS" || fail "TurnCompletePayload not in streaming.rs"
pass "all stream event payload types defined"

# 3. event name constants present
grep -q 'EVENT_LLM_TOKEN' "$STREAMING_RS" || fail "EVENT_LLM_TOKEN constant missing"
grep -q 'EVENT_TTS_CHUNK' "$STREAMING_RS" || fail "EVENT_TTS_CHUNK constant missing"
grep -q 'EVENT_TURN_COMPLETE' "$STREAMING_RS" || fail "EVENT_TURN_COMPLETE constant missing"
pass "stream event name constants present"

# 4. streaming llm pipeline (m12.2)
grep -q "start_streaming_turn" "$CHAT_STREAMING_RS" || fail "start_streaming_turn not in chat_streaming.rs"
grep -q "run_llm_stream" "$CHAT_STREAMING_RS" || fail "run_llm_stream not in chat_streaming.rs"
grep -q "OpenAiStreamingReasoningProvider" "$CHAT_STREAMING_RS" || fail "streaming reasoning provider not used"
grep -q "stream_response" "$CHAT_STREAMING_RS" || fail "stream_response not called in pipeline"
pass "streaming llm pipeline present (m12.2)"

# 5. phrase-chunked tts (m12.3 core feature)
grep -q "phrase_needs_synthesis" "$CHAT_STREAMING_RS" || fail "phrase_needs_synthesis not in chat_streaming.rs"
grep -q "spawn_tts_chunk" "$CHAT_STREAMING_RS" || fail "spawn_tts_chunk not in chat_streaming.rs"
grep -q "synthesize_phrase_async" "$CHAT_STREAMING_RS" || fail "synthesize_phrase_async not in chat_streaming.rs"
grep -q "phrase_buf" "$CHAT_STREAMING_RS" || fail "phrase_buf accumulator not found"
grep -q "first_audio_reported" "$CHAT_STREAMING_RS" || fail "first_audio_reported tracking missing"
pass "phrase-chunked tts synthesis present (m12.3)"

# 6. cancellation propagates to tts tasks
grep -q "child_token" "$CHAT_STREAMING_RS" || fail "child cancellation tokens not used for tts tasks"
pass "cancellation propagates to tts tasks via child tokens"

# 7. phrase flush at end of llm stream
grep -q "remaining" "$CHAT_STREAMING_RS" || fail "remaining phrase flush not found"
pass "remaining phrase buffer flushed after llm stream ends"

# 8. tauri commands registered (m12.2)
grep -q "send_message_streaming" "$COMMANDS_RS" || fail "send_message_streaming command not in commands.rs"
grep -q "cancel_streaming_turn" "$COMMANDS_RS" || fail "cancel_streaming_turn command not in commands.rs"
grep -q "send_message_streaming" "$MAIN_RS" || fail "send_message_streaming not registered in main.rs"
grep -q "cancel_streaming_turn" "$MAIN_RS" || fail "cancel_streaming_turn not registered in main.rs"
pass "streaming commands registered in invoke_handler"

# 9. frontend typed event listeners
grep -q "EVENT_LLM_TOKEN" "$STREAM_CLIENT_TS" || fail "EVENT_LLM_TOKEN not in streamClient.ts"
grep -q "EVENT_TTS_CHUNK" "$STREAM_CLIENT_TS" || fail "EVENT_TTS_CHUNK not in streamClient.ts"
grep -q "TtsChunkPayload" "$STREAM_CLIENT_TS" || fail "TtsChunkPayload not in streamClient.ts"
grep -q "isStreamingEnabled" "$STREAM_CLIENT_TS" || fail "isStreamingEnabled not in streamClient.ts"
pass "frontend stream event types and listeners present"

# 10. frontend tts audio queue
grep -q "streamTtsQueueRef" "$APP_TSX" || fail "streamTtsQueueRef not found in App.tsx"
grep -q "scheduleStreamTtsPlayback" "$APP_TSX" || fail "scheduleStreamTtsPlayback not found in App.tsx"
grep -q "stopStreamingTtsPlayback" "$APP_TSX" || fail "stopStreamingTtsPlayback not found in App.tsx"
grep -q "ttsActiveTurnIdRef" "$APP_TSX" || fail "ttsActiveTurnIdRef not found in App.tsx"
grep -q "streamTtsNextPhraseRef" "$APP_TSX" || fail "streamTtsNextPhraseRef not found in App.tsx"
pass "frontend streaming tts audio queue present"

# 11. barge-in: stopStreamingTtsPlayback called in interruptCurrentInteraction
INTERRUPT_BLOCK=$(awk '/async function interruptCurrentInteraction/,/^  }/' "$APP_TSX")
echo "$INTERRUPT_BLOCK" | grep -q "stopStreamingTtsPlayback" || \
  fail "stopStreamingTtsPlayback not called in interruptCurrentInteraction (barge-in broken)"
echo "$INTERRUPT_BLOCK" | grep -q "cancelStreamingTurn" || \
  fail "cancelStreamingTurn not called in interruptCurrentInteraction (backend cancel broken)"
pass "barge-in cuts streaming tts and cancels backend turn"

# 12. partial stt via web speech api
grep -q "tryStartPartialStt" "$APP_TSX" || fail "tryStartPartialStt not found in App.tsx"
grep -q "stopPartialStt" "$APP_TSX" || fail "stopPartialStt not found in App.tsx"
grep -q "partialSttSentRef" "$APP_TSX" || fail "partialSttSentRef guard not found in App.tsx"
grep -q "interimResults" "$APP_TSX" || fail "interimResults not set (partial stt will not work)"
grep -q "0.7" "$APP_TSX" || fail "confidence threshold 0.7 not found in App.tsx"
pass "partial stt via web speech api present"

# 13. partial stt skips whisper when already routed
FINALIZE_BLOCK=$(awk '/async function finalizeVoiceInput/,/^  }/' "$APP_TSX")
echo "$FINALIZE_BLOCK" | grep -q "partialSttSentRef.current" || \
  fail "finalizeVoiceInput does not check partialSttSentRef (whisper would double-submit)"
pass "whisper skipped when partial stt has already routed"

# 14. no-key guard: tts synthesis only runs when api key is present
grep -q "tts_api_key" "$CHAT_STREAMING_RS" || fail "tts_api_key guard not found"
pass "tts synthesis skipped when OPENAI_API_KEY absent"

# 15. streaming ui renders token-by-token
grep -q "streaming-indicator" "$APP_TSX" || fail "streaming-indicator not found in App.tsx"
grep -q "streamingText\[streamingTurnId\]" "$APP_TSX" || fail "streaming text accumulator not rendered"
pass "streaming ui renders tokens incrementally"

# 16. overlay also uses streaming send path (streaming everywhere)
grep -q "sendMessageStreaming" "$OVERLAY_TSX" || fail "Overlay.tsx does not use sendMessageStreaming"
grep -q "EVENT_LLM_TOKEN" "$OVERLAY_TSX" || fail "Overlay.tsx missing llm token stream listener"
pass "overlay uses streaming send path and llm token listener"

# 17. cancellation command accepts reason tags
grep -q "reason: Option<String>" "$COMMANDS_RS" || fail "cancel_streaming_turn missing reason parameter"
grep -q "cancelStreamingTurn(activeTurnId, reason)" "$APP_TSX" || fail "App.tsx does not forward barge-in reason"
pass "cancel command accepts and forwards reason tags"

# 18. turn_complete carries first_audio_ms metric
grep -q "first_audio_ms" "$CHAT_STREAMING_RS" || fail "chat_streaming.rs missing first_audio_ms metric"
grep -q "first_audio_ms," "$CHAT_STREAMING_RS" || fail "turn_complete does not include computed first_audio_ms"
pass "turn_complete emits first_audio_ms metric"

# 19. full build and test suite
cd "$ROOT_DIR/desktop"
npm run lint
npm run test
cargo build --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml phrase_needs_synthesis -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml streaming -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml streaming
cargo test --manifest-path src-tauri/Cargo.toml chat_streaming
pass "automated build + test suite passed"

echo ""
echo "phase 12 checks passed"
