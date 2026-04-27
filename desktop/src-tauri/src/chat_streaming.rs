// phase 12: streaming send-message pipeline.
// coordinates llm token streaming, db placeholder lifecycle, and tts
// phrase-chunked synthesis. all async; cancelled via InteractionToken.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Instant;

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio_util::sync::CancellationToken;

use crate::{
    ambient,
    chat::{build_system_prompt, build_user_prompt},
    coworking::unix_now_seconds,
    embedding::EmbeddingProvider,
    message_kind::MessageKind,
    reasoning::OpenAiStreamingReasoningProvider,
    retrieval::build_task_context_pack,
    state::JeffState,
    store::TaskStore,
    streaming::{
        InteractionToken, LlmCompletePayload, LlmTokenPayload, SharedRegistry, TtsChunkPayload,
        TurnCancelledPayload, TurnCompletePayload, EVENT_LLM_COMPLETE, EVENT_LLM_TOKEN,
        EVENT_TTS_CHUNK, EVENT_TURN_CANCELLED, EVENT_TURN_COMPLETE,
    },
};

// ---- tts phrase synthesis helpers ------------------------------------------

// synthesize one phrase via openai tts, return base64-encoded mp3.
// non-fatal: caller should ignore errors and continue.
async fn synthesize_phrase_async(api_key: &str, text: &str, voice: &str) -> Result<String> {
    let client = reqwest::Client::new();
    let voice = crate::voice_naturalness::normalize_tts_voice(voice);
    let response = client
        .post("https://api.openai.com/v1/audio/speech")
        .bearer_auth(api_key)
        .json(&serde_json::json!({
            "model": "gpt-4o-mini-tts",
            "voice": voice,
            "input": text,
            "format": "mp3"
        }))
        .send()
        .await
        .context("tts request failed")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("tts status {status}: {body}"));
    }

    let bytes = response.bytes().await.context("tts bytes read failed")?;
    Ok(BASE64.encode(bytes))
}

// true when the phrase buffer has accumulated enough text at a sentence boundary
// to be worth synthesizing independently. minimum length avoids tts calls for
// very short fragments that produce audible latency spikes.
fn phrase_needs_synthesis(buf: &str) -> bool {
    let has_newline_boundary = buf.ends_with('\n');
    let trimmed = buf.trim_end();
    if trimmed.len() < 20 {
        return false;
    }
    if has_newline_boundary {
        return true;
    }
    matches!(trimmed.chars().last(), Some('.' | '?' | '!'))
}

// spawn an async task to synthesize one phrase and emit a tts_chunk event.
// cancel token is a child of the turn token so cancellation propagates cleanly.
// first_audio_reported is updated atomically on the first successful synthesis.
fn spawn_tts_chunk<R: Runtime + 'static>(
    api_key: String,
    text: String,
    voice: String,
    phrase_id: u32,
    app: AppHandle<R>,
    turn_id: String,
    cancel: CancellationToken,
    first_audio_reported: Arc<AtomicU64>,
    turn_start: Instant,
) {
    tauri::async_runtime::spawn(async move {
        if cancel.is_cancelled() {
            return;
        }
        let spoken_text =
            crate::voice_naturalness::prepare_tts_text(&text, &format!("{turn_id}:{phrase_id}"));
        if spoken_text.trim().is_empty() {
            return;
        }
        match synthesize_phrase_async(&api_key, &spoken_text, &voice).await {
            Ok(audio_b64) => {
                if cancel.is_cancelled() {
                    return;
                }
                let ms = turn_start.elapsed().as_millis() as u64;
                // only the first synthesis records the latency.
                let _ = first_audio_reported.compare_exchange(
                    0,
                    ms,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
                let _ = app.emit(
                    EVENT_TTS_CHUNK,
                    &TtsChunkPayload {
                        turn_id,
                        phrase_id,
                        audio_b64,
                        first_audio_ms: ms,
                    },
                );
            }
            Err(_) => {
                // tts failure is non-fatal: text still renders in chat.
            }
        }
    });
}

// entry point called from the send_message_streaming tauri command.
// this function is async because the tauri command is async; it spawns
// the streaming work and returns immediately with the turn_id.
pub async fn start_streaming_turn(
    state: &JeffState,
    app: AppHandle<impl Runtime + 'static>,
    task_id: i64,
    message: String,
    message_source: String,
    token: InteractionToken,
    registry: SharedRegistry,
    // phase 20: optional context prefix for the system prompt.
    active_context: Option<String>,
) -> Result<()> {
    use crate::message_kind::classify_user_message_kind;

    let clean = message.trim().to_string();
    if clean.is_empty() {
        return Err(anyhow::anyhow!("message cannot be empty"));
    }

    // 1. append user message synchronously (fast sqlite write).
    let user_kind = classify_user_message_kind(&clean);
    state
        .store
        .append_chat_message(task_id, "user", &message_source, user_kind, &clean)?;

    // let the coworking runtime know the user sent a message. this mirrors
    // what commands::send_message does when it calls set_user_typing(false).
    let now = unix_now_seconds().unwrap_or(0);
    {
        let mut coworking = state
            .coworking
            .lock()
            .map_err(|_| anyhow::anyhow!("coworking lock poisoned"))?;
        // note_interruption resets the nudge cooldown clock on a new turn,
        // matching the behavior of the non-streaming send path.
        coworking.note_interruption(now);
    }

    // 2. insert placeholder for assistant response.
    let placeholder_id = state.store.insert_streaming_placeholder(task_id)?;

    // 3. build context pack (db reads + embedding — blocking but fast).
    let store = state.store.clone();
    let embeddings = state.embeddings.clone();
    let reasoning = OpenAiStreamingReasoningProvider::from_env();
    let tts_voice = state.store.get_tts_voice()?;

    // capture everything needed to run the async pipeline.
    tauri::async_runtime::spawn(run_llm_stream(
        store,
        embeddings,
        reasoning,
        app,
        task_id,
        clean,
        placeholder_id,
        token,
        registry,
        active_context,
        tts_voice,
    ));

    Ok(())
}

async fn run_llm_stream<R: Runtime + 'static>(
    store: TaskStore,
    embeddings: Arc<dyn EmbeddingProvider>,
    reasoning: OpenAiStreamingReasoningProvider,
    app: AppHandle<R>,
    task_id: i64,
    message: String,
    placeholder_id: i64,
    token: InteractionToken,
    registry: SharedRegistry,
    active_context: Option<String>,
    tts_voice: String,
) {
    let turn_start = Instant::now();
    let turn_id = token.turn_id.clone();
    let _active_turn = ActiveTurnGuard::new(registry, turn_id.clone());

    // read api key once via resolver; tts synthesis is skipped silently if absent.
    let tts_api_key = crate::secrets::resolve_openai_api_key().api_key;
    let first_audio_reported: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
    let mut phrase_buf = String::new();
    let mut phrase_id: u32 = 0;

    // Build the context pack outside the hot loop. The embedding provider uses
    // reqwest::blocking today, so keep retrieval work off the async worker.
    let context_store = store.clone();
    let context_embeddings = embeddings.clone();
    let context_message = message.clone();
    let context_pack_result = tauri::async_runtime::spawn_blocking(move || {
        build_task_context_pack(
            &context_store,
            context_embeddings.as_ref(),
            task_id,
            &context_message,
        )
    })
    .await;

    let context_pack = match context_pack_result {
        Ok(Ok(pack)) => pack,
        Ok(Err(err)) => {
            finalize_and_emit_cancelled(
                &store,
                &app,
                placeholder_id,
                &turn_id,
                String::new(),
                &format!("context_pack_error: {err}"),
                turn_start,
            );
            return;
        }
        Err(err) => {
            finalize_and_emit_cancelled(
                &store,
                &app,
                placeholder_id,
                &turn_id,
                String::new(),
                &format!("context_pack_join_error: {err}"),
                turn_start,
            );
            return;
        }
    };

    if token.is_cancelled() {
        let reason = token.cancellation_reason();
        finalize_and_emit_cancelled(
            &store,
            &app,
            placeholder_id,
            &turn_id,
            String::new(),
            &reason,
            turn_start,
        );
        return;
    }

    let user_prompt = build_user_prompt(&message, &context_pack, active_context.as_deref());

    // phase 20/23: prepend active window context and user profile to system prompt.
    let effective_system_prompt = build_system_prompt(&store, active_context.as_deref());

    // open the streaming LLM channel.
    let mut rx = match reasoning.stream_response(
        &effective_system_prompt,
        &user_prompt,
        token.cancel.clone(),
    ) {
        Ok(rx) => rx,
        Err(err) => {
            finalize_and_emit_cancelled(
                &store,
                &app,
                placeholder_id,
                &turn_id,
                String::new(),
                &format!("llm_stream_open_error: {err}"),
                turn_start,
            );
            return;
        }
    };

    let mut full_text = String::new();
    let mut token_index: u32 = 0;
    let mut ttft_ms: Option<u64> = None;

    loop {
        tokio::select! {
            _ = token.cancel.cancelled() => {
                let reason = token.cancellation_reason();
                finalize_and_emit_cancelled(
                    &store,
                    &app,
                    placeholder_id,
                    &turn_id,
                    full_text,
                    &reason,
                    turn_start,
                );
                return;
            }

            msg = rx.recv() => {
                match msg {
                    None => {
                        // channel closed = stream complete.
                        break;
                    }
                    Some(Err(err)) => {
                        finalize_and_emit_cancelled(
                            &store,
                            &app,
                            placeholder_id,
                            &turn_id,
                            full_text,
                            &format!("stream_error: {err}"),
                            turn_start,
                        );
                        return;
                    }
                    Some(Ok(delta)) => {
                        if ttft_ms.is_none() {
                            ttft_ms = Some(turn_start.elapsed().as_millis() as u64);
                        }

                        full_text.push_str(&delta);
                        phrase_buf.push_str(&delta);

                        let _ = app.emit(
                            EVENT_LLM_TOKEN,
                            &LlmTokenPayload {
                                turn_id: turn_id.clone(),
                                delta,
                                index: token_index,
                            },
                        );
                        token_index += 1;

                        // synthesize completed phrases as they arrive so tts
                        // playback starts before the full llm response is done.
                        if phrase_needs_synthesis(&phrase_buf) {
                            let phrase = std::mem::take(&mut phrase_buf)
                                .trim()
                                .to_string();
                            if !phrase.is_empty() {
                                phrase_id += 1;
                                if let Some(ref key) = tts_api_key {
                                    spawn_tts_chunk(
                                        key.clone(),
                                        phrase,
                                        tts_voice.clone(),
                                        phrase_id,
                                        app.clone(),
                                        turn_id.clone(),
                                        token.cancel.child_token(),
                                        first_audio_reported.clone(),
                                        turn_start,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // flush any remaining phrase that did not end at a sentence boundary.
    let remaining = phrase_buf.trim().to_string();
    if !remaining.is_empty() {
        phrase_id += 1;
        if let Some(ref key) = tts_api_key {
            spawn_tts_chunk(
                key.clone(),
                remaining,
                tts_voice.clone(),
                phrase_id,
                app.clone(),
                turn_id.clone(),
                token.cancel.child_token(),
                first_audio_reported.clone(),
                turn_start,
            );
        }
    }

    // stream ended normally — finalize db record.
    let total_ms = turn_start.elapsed().as_millis() as u64;
    let final_kind = MessageKind::AssistantAnswer;
    let first_audio_ms = match first_audio_reported.load(Ordering::SeqCst) {
        0 => None,
        ms => Some(ms),
    };

    if let Err(err) = store.finalize_streaming_message(placeholder_id, &full_text, final_kind) {
        let _ = app.emit(
            EVENT_TURN_CANCELLED,
            &TurnCancelledPayload {
                turn_id: turn_id.clone(),
                reason: format!("finalize_error: {err}"),
                partial_text: full_text,
                elapsed_ms: total_ms,
            },
        );
        return;
    }

    if store
        .get_privacy_user_profile_memory_enabled()
        .unwrap_or(false)
    {
        let _ =
            crate::user_model::record_response_length(&store, full_text.split_whitespace().count());
    }

    let _ = app.emit(
        EVENT_LLM_COMPLETE,
        &LlmCompletePayload {
            turn_id: turn_id.clone(),
            full_text: full_text.clone(),
            cancelled: false,
            ttft_ms,
            total_ms,
        },
    );

    let _ = app.emit(
        EVENT_TURN_COMPLETE,
        &TurnCompletePayload {
            turn_id: turn_id.clone(),
            duration_ms: total_ms,
            ttft_ms,
            first_audio_ms,
        },
    );

    notify_turn_completion_if_backgrounded(
        &app,
        placeholder_id,
        &full_text,
        "Jeff finished a response",
    );
}

fn finalize_and_emit_cancelled<R: Runtime>(
    store: &TaskStore,
    app: &AppHandle<R>,
    placeholder_id: i64,
    turn_id: &str,
    partial_text: String,
    reason: &str,
    turn_start: Instant,
) {
    let elapsed_ms = turn_start.elapsed().as_millis() as u64;
    let _ = store.finalize_streaming_message(
        placeholder_id,
        &partial_text,
        MessageKind::AssistantInterrupted,
    );
    let _ = app.emit(
        EVENT_TURN_CANCELLED,
        &TurnCancelledPayload {
            turn_id: turn_id.to_string(),
            reason: reason.to_string(),
            partial_text,
            elapsed_ms,
        },
    );
}

struct ActiveTurnGuard {
    registry: SharedRegistry,
    turn_id: String,
}

impl ActiveTurnGuard {
    fn new(registry: SharedRegistry, turn_id: String) -> Self {
        Self { registry, turn_id }
    }
}

impl Drop for ActiveTurnGuard {
    fn drop(&mut self) {
        self.registry.remove(&self.turn_id);
    }
}

fn is_window_visible<R: Runtime>(app: &AppHandle<R>, label: &str) -> bool {
    app.get_webview_window(label)
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false)
}

fn should_notify_when_backgrounded<R: Runtime>(app: &AppHandle<R>) -> bool {
    let overlay_visible = is_window_visible(app, ambient::OVERLAY_WINDOW_LABEL);
    !overlay_visible
}

fn compact_notification_body(content: &str, max_chars: usize) -> String {
    let compact = content.split_whitespace().collect::<Vec<&str>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }

    let mut trimmed: String = compact.chars().take(max_chars).collect();
    while trimmed.ends_with(' ') {
        trimmed.pop();
    }
    format!("{trimmed}...")
}

fn notify_turn_completion_if_backgrounded<R: Runtime>(
    app: &AppHandle<R>,
    message_id: i64,
    content: &str,
    title: &str,
) {
    if !should_notify_when_backgrounded(app) {
        return;
    }

    let body = compact_notification_body(content, 160);
    if body.trim().is_empty() {
        return;
    }

    let _ = ambient::dispatch_notification(
        app,
        ambient::NotificationPayload {
            title: title.to_string(),
            body,
            context_kind: Some("assistant_answer".to_string()),
            context_id: Some(message_id),
        },
    );
}

// ---- tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use crate::{
        message_kind::MessageKind,
        store::TaskStore,
        streaming::{new_shared_registry, new_turn_id, InteractionToken},
    };

    fn init_store(base: &Path) -> (TaskStore, i64) {
        let store = TaskStore::initialize(base).unwrap();
        let task = store.create_task("stream test").unwrap();
        store.set_active_task(task.id).unwrap();
        (store, task.id)
    }

    // directly test store methods used by the streaming pipeline.
    #[test]
    fn insert_and_finalize_streaming_placeholder() {
        let temp = tempfile::tempdir().unwrap();
        let (store, task_id) = init_store(&temp.path().join("data"));

        let msg_id = store.insert_streaming_placeholder(task_id).unwrap();
        assert!(msg_id > 0);

        // the placeholder row exists with kind=assistant_partial
        let messages = store.list_chat_messages(task_id).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].message_kind, "assistant_partial");
        assert_eq!(messages[0].role, "assistant");

        // finalize to assistant_answer
        store
            .finalize_streaming_message(msg_id, "Hello there.", MessageKind::AssistantAnswer)
            .unwrap();

        let messages = store.list_chat_messages(task_id).unwrap();
        assert_eq!(messages[0].message_kind, "assistant_answer");
        assert_eq!(messages[0].content, "Hello there.");
    }

    #[test]
    fn finalize_with_empty_content_stores_interrupted_marker() {
        let temp = tempfile::tempdir().unwrap();
        let (store, task_id) = init_store(&temp.path().join("data"));

        let msg_id = store.insert_streaming_placeholder(task_id).unwrap();
        store
            .finalize_streaming_message(msg_id, "", MessageKind::AssistantInterrupted)
            .unwrap();

        let messages = store.list_chat_messages(task_id).unwrap();
        assert_eq!(messages[0].message_kind, "assistant_interrupted");
        assert_eq!(messages[0].content, "(interrupted)");
    }

    #[tokio::test]
    async fn cancellation_token_stops_channel_reader() {
        let cancel = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel::<anyhow::Result<String>>(4);

        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            // simulate a slow producer
            for i in 0..100u32 {
                if cancel_clone.is_cancelled() {
                    break;
                }
                let _ = tx.send(Ok(format!("token_{i}"))).await;
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        });

        // read 2 tokens then cancel
        let mut received = 0u32;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                msg = rx.recv() => {
                    if msg.is_none() { break; }
                    received += 1;
                    if received >= 2 {
                        cancel.cancel();
                    }
                }
            }
        }

        assert!(received >= 2);
        // after cancel, no more tokens should be processed
        assert!(received < 100);
    }

    #[test]
    fn new_turn_id_produces_unique_ids() {
        let a = new_turn_id();
        let b = new_turn_id();
        assert_ne!(a, b);
    }

    #[test]
    fn phrase_needs_synthesis_requires_sentence_boundary() {
        assert!(!super::phrase_needs_synthesis("short."));
        assert!(!super::phrase_needs_synthesis(
            "This is long enough but has no boundary"
        ));
        assert!(super::phrase_needs_synthesis(
            "This is long enough to synthesize and ends with punctuation."
        ));
        assert!(super::phrase_needs_synthesis(
            "This is also long enough to synthesize and ends with a newline\n"
        ));
    }

    #[test]
    fn compact_notification_body_collapses_whitespace_and_truncates() {
        let compact =
            super::compact_notification_body("  multiple\nlines   with\tmixed   spacing ", 80);
        assert_eq!(compact, "multiple lines with mixed spacing");

        let truncated = super::compact_notification_body("abcdefghijklmnopqrstuvwxyz", 10);
        assert_eq!(truncated, "abcdefghij...");
    }

    #[test]
    fn active_turn_guard_removes_registry_entry_on_drop() {
        let registry = new_shared_registry();
        let token = InteractionToken::new("turn_guard_test".to_string());
        registry.register(&token);
        assert_eq!(registry.active_count(), 1);
        {
            let _guard = super::ActiveTurnGuard::new(registry.clone(), token.turn_id.clone());
        }
        assert_eq!(registry.active_count(), 0);
    }
}
