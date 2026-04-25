use anyhow::{anyhow, Result};

use crate::{
    embedding::EmbeddingProvider,
    message_kind::{classify_user_message_kind, MessageKind},
    models::{SendMessageResponseDto, TaskContextPackDto},
    reasoning::ReasoningProvider,
    retrieval::build_task_context_pack,
    store::TaskStore,
    user_model,
};

const GROUNDING_SYSTEM_PROMPT: &str = "You are Jeff, a task-focused assistant. Use only the provided context chunks, active-window title context, and explicitly selected-text context to answer. If the answer is not in context, explicitly say you don't know based on available materials. Be concise. One to three sentences unless the user asks for more. No filler phrases.";

pub fn send_message_for_task(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    reasoning: &dyn ReasoningProvider,
    task_id: i64,
    message: &str,
    message_source: &str,
    // phase 20: optional active-window context injected as a system-prompt prefix.
    // format: "User's active app: X. Document: Y." — under 30 tokens.
    active_context: Option<&str>,
    is_cancelled: impl Fn() -> bool,
) -> Result<SendMessageResponseDto> {
    let clean_message = message.trim();
    if clean_message.is_empty() {
        return Err(anyhow!("message cannot be empty"));
    }

    let user_message_kind = classify_user_message_kind(clean_message);
    store.append_chat_message(
        task_id,
        "user",
        message_source,
        user_message_kind,
        clean_message,
    )?;

    if is_cancelled() {
        return Ok(SendMessageResponseDto {
            assistant_response: String::new(),
            retrieved_chunks: Vec::new(),
            cancelled: true,
        });
    }

    let context_pack = build_task_context_pack(store, embeddings, task_id, clean_message)?;
    let user_prompt = build_user_prompt(clean_message, &context_pack);

    let effective_system_prompt = build_system_prompt(store, active_context);
    let assistant_response = reasoning.generate_response(&effective_system_prompt, &user_prompt)?;
    if is_cancelled() {
        return Ok(SendMessageResponseDto {
            assistant_response: String::new(),
            retrieved_chunks: context_pack.retrieved_chunks,
            cancelled: true,
        });
    }

    store.append_chat_message(
        task_id,
        "assistant",
        "assistant",
        MessageKind::AssistantAnswer,
        &assistant_response,
    )?;

    Ok(SendMessageResponseDto {
        assistant_response,
        retrieved_chunks: context_pack.retrieved_chunks,
        cancelled: false,
    })
}

/// builds the effective system prompt, prepending active window context and user
/// profile injection when present. profile injection is gated on the privacy setting.
pub fn build_system_prompt(store: &TaskStore, active_context: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();

    // phase 23: profile injection (gated on privacy setting)
    if store
        .get_privacy_user_profile_memory_enabled()
        .unwrap_or(false)
    {
        if let Some(injection) = user_model::build_profile_injection(store) {
            parts.push(injection);
        }
    }

    if let Some(ctx) = active_context {
        if !ctx.is_empty() {
            parts.push(ctx.to_string());
        }
    }

    parts.push(GROUNDING_SYSTEM_PROMPT.to_string());
    parts.join("\n\n")
}

pub fn build_user_prompt(message: &str, context_pack: &TaskContextPackDto) -> String {
    let chunks_text = if context_pack.retrieved_chunks.is_empty() {
        "No retrieved chunks available.".to_string()
    } else {
        context_pack
            .retrieved_chunks
            .iter()
            .enumerate()
            .map(|(index, chunk)| {
                format!(
                    "Chunk {}\nSource: {}\nScore: {:.4}\nText:\n{}",
                    index + 1,
                    chunk.artifact_file_name,
                    chunk.similarity_score,
                    chunk.chunk_text
                )
            })
            .collect::<Vec<String>>()
            .join("\n\n")
    };

    format!(
        "Task Summary:\n{}\n\nUser Query:\n{}\n\nRetrieved Context Chunks:\n{}\n\nAnswer strictly from retrieved context and any active-window title or selected-text context in the system prompt.",
        context_pack.task_summary, message, chunks_text
    )
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use anyhow::Result;

    use crate::{
        embedding::EmbeddingProvider, message_kind::MessageKind, reasoning::ReasoningProvider,
        store::TaskStore,
    };

    use super::send_message_for_task;

    #[derive(Clone)]
    struct KeywordEmbeddingProvider;

    impl EmbeddingProvider for KeywordEmbeddingProvider {
        fn embed_text(&self, input: &str) -> Result<Vec<f32>> {
            let lower = input.to_lowercase();
            let score = |terms: &[&str]| -> f32 {
                terms
                    .iter()
                    .map(|term| lower.matches(term).count() as f32)
                    .sum()
            };

            Ok(vec![
                score(&["primary", "source", "evidence"]),
                score(&["section", "sections", "structure"]),
                score(&["reading", "readings"]),
                (lower.len() as f32) / 1000.0,
            ])
        }
    }

    #[derive(Clone)]
    struct GroundedEchoReasoning;

    impl ReasoningProvider for GroundedEchoReasoning {
        fn generate_response(&self, _system_prompt: &str, user_prompt: &str) -> Result<String> {
            let lower = user_prompt.to_lowercase();
            if lower.contains("primary source")
                || lower.contains("course readings")
                || lower.contains("evidence")
            {
                Ok("Grounded answer: use primary sources, course readings, and evidence requirements from the rubric.".to_string())
            } else {
                Ok("I don't know based on the available context.".to_string())
            }
        }
    }

    fn write_file(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("failed to create parent directory");
        }

        fs::write(path, body).expect("failed to write file");
    }

    #[test]
    fn send_message_returns_grounded_answer_and_debug_chunks() {
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let base_path = temp.path().join("app_local_data");
        let store = TaskStore::initialize(&base_path).expect("failed to initialize store");
        let task = store
            .create_task("History StoryMap")
            .expect("failed to create task");
        store
            .set_active_task(task.id)
            .expect("failed to set active task");

        let notes = temp.path().join("fixtures").join("notes.md");
        write_file(
            &notes,
            "Primary source requirement: include course readings and evidence requirements in each section.",
        );

        crate::retrieval::import_artifact_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task.id,
            &notes.to_string_lossy(),
        )
        .expect("failed to import notes");

        let response = send_message_for_task(
            &store,
            &KeywordEmbeddingProvider,
            &GroundedEchoReasoning,
            task.id,
            "What are the primary source requirements?",
            "text",
            None,
            || false,
        )
        .expect("failed to send message");

        assert!(response
            .assistant_response
            .to_lowercase()
            .contains("primary sources"));
        assert!(!response.retrieved_chunks.is_empty());
        assert!(!response.cancelled);

        let history = store
            .list_chat_messages(task.id)
            .expect("failed to load chat history");
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].role, "user");
        assert_eq!(history[0].message_source, "text");
        assert_eq!(
            history[0].message_kind,
            MessageKind::UserDirectQuestion.as_str()
        );
        assert_eq!(history[1].role, "assistant");
        assert_eq!(history[1].message_source, "assistant");
        assert_eq!(
            history[1].message_kind,
            MessageKind::AssistantAnswer.as_str()
        );
    }

    #[test]
    fn cancelled_send_message_returns_cancelled_flag_without_assistant_append() {
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let base_path = temp.path().join("app_local_data");
        let store = TaskStore::initialize(&base_path).expect("failed to initialize store");
        let task = store
            .create_task("Cancel Test")
            .expect("failed to create task");
        store
            .set_active_task(task.id)
            .expect("failed to set active task");

        let notes = temp.path().join("fixtures").join("notes.md");
        write_file(
            &notes,
            "Primary source requirement and evidence requirement.",
        );
        crate::retrieval::import_artifact_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task.id,
            &notes.to_string_lossy(),
        )
        .expect("failed to import notes");

        let response = send_message_for_task(
            &store,
            &KeywordEmbeddingProvider,
            &GroundedEchoReasoning,
            task.id,
            "primary source requirement",
            "voice",
            None,
            || true,
        )
        .expect("failed to send cancelled message");

        assert!(response.cancelled);
        let messages = store
            .list_chat_messages(task.id)
            .expect("failed to list chat history");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].message_source, "voice");
        assert_eq!(
            messages[0].message_kind,
            MessageKind::UserStatement.as_str()
        );
    }

    #[test]
    fn voice_direct_question_is_classified_as_direct_question_and_answered_normally() {
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let base_path = temp.path().join("app_local_data");
        let store = TaskStore::initialize(&base_path).expect("failed to initialize store");
        let task = store
            .create_task("Voice Question")
            .expect("failed to create task");
        store
            .set_active_task(task.id)
            .expect("failed to set active task");

        let notes = temp.path().join("fixtures").join("notes.md");
        write_file(
            &notes,
            "Primary source requirement: include course readings and evidence requirements.",
        );
        crate::retrieval::import_artifact_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task.id,
            &notes.to_string_lossy(),
        )
        .expect("failed to import notes");

        let response = send_message_for_task(
            &store,
            &KeywordEmbeddingProvider,
            &GroundedEchoReasoning,
            task.id,
            "What are the primary source requirements?",
            "voice",
            None,
            || false,
        )
        .expect("failed to send voice direct question");

        assert!(!response.cancelled);
        assert!(response
            .assistant_response
            .to_lowercase()
            .contains("primary sources"));

        let history = store
            .list_chat_messages(task.id)
            .expect("failed to list chat history");
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].message_source, "voice");
        assert_eq!(
            history[0].message_kind,
            MessageKind::UserDirectQuestion.as_str()
        );
        assert_eq!(
            history[1].message_kind,
            MessageKind::AssistantAnswer.as_str()
        );
    }

    #[test]
    fn real_storymap_retrieval_quality_check() {
        let notes = PathBuf::from(
            "/Users/krishmalik/Desktop/Continuum/jeff_data/tasks/history-storymap/notes.md",
        );
        let rubric = PathBuf::from(
            "/Users/krishmalik/Desktop/Continuum/jeff_data/tasks/history-storymap/rubric.pdf",
        );

        assert!(
            notes.exists() && rubric.exists(),
            "expected real storymap files at {} and {}",
            notes.display(),
            rubric.display()
        );

        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let base_path = temp.path().join("app_local_data");
        let store = TaskStore::initialize(&base_path).expect("failed to initialize store");
        let task = store
            .create_task("History StoryMap Real Retrieval")
            .expect("failed to create task");

        let embeddings = KeywordEmbeddingProvider;
        crate::retrieval::import_artifact_for_task(
            &store,
            &embeddings,
            task.id,
            &notes.to_string_lossy(),
        )
        .expect("failed to import notes.md");
        crate::retrieval::import_artifact_for_task(
            &store,
            &embeddings,
            task.id,
            &rubric.to_string_lossy(),
        )
        .expect("failed to import rubric.pdf");

        let queries = [
            "primary source requirement",
            "how many sections are required",
            "what should each section contain",
        ];

        let mut combined = String::new();
        for query in queries {
            let chunks =
                crate::retrieval::retrieve_relevant_chunks(&store, &embeddings, task.id, query)
                    .expect("failed to retrieve chunks for quality check");

            assert!(
                !chunks.is_empty(),
                "retrieval returned no chunks for query '{query}'"
            );
            for chunk in chunks {
                combined.push_str(&chunk.chunk_text.to_lowercase());
                combined.push('\n');
            }
        }

        assert!(
            combined.contains("primary source") || combined.contains("evidence requirement"),
            "retrieval quality check failed: missing primary-source/evidence chunk"
        );

        assert!(
            combined.contains("sections") || combined.contains("6 sections"),
            "retrieval quality check failed: missing sections chunk"
        );
    }
}
