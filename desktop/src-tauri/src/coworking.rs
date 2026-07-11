use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};

use crate::{
    embedding::EmbeddingProvider,
    message_kind::MessageKind,
    models::{CoworkingStatusDto, ProactiveEvaluationDto, ProactiveNudgeDto},
    reasoning::ReasoningProvider,
    retrieval::retrieve_relevant_chunks_with_top_k,
    store::TaskStore,
};

const NUDGE_SYSTEM_PROMPT: &str = "You are Jeff, a restrained coworker. Decide whether to issue a proactive nudge. If context is weak or uncertain, output exactly NO_NUDGE. If context is strong, output exactly one short advisory sentence grounded in the provided chunks. Keep it under 25 words and do not issue commands.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoworkingState {
    Idle,
    Listening,
    Thinking,
    Speaking,
    SilentObserving,
    AwaitingUser,
    Suppressed,
}

impl CoworkingState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Listening => "listening",
            Self::Thinking => "thinking",
            Self::Speaking => "speaking",
            Self::SilentObserving => "silent_observing",
            Self::AwaitingUser => "awaiting_user",
            Self::Suppressed => "suppressed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    Discussion,
    Quiet,
}

impl SessionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discussion => "discussion",
            Self::Quiet => "quiet",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CoworkingConfig {
    pub proactive_mode: bool,
    pub pause_threshold_seconds: u64,
    pub nudge_cooldown_seconds: u64,
    pub interruption_suppression_seconds: u64,
    pub low_confidence_suppression_seconds: u64,
    pub min_retrieval_confidence: f32,
}

impl Default for CoworkingConfig {
    fn default() -> Self {
        Self {
            proactive_mode: true,
            pause_threshold_seconds: 12,
            nudge_cooldown_seconds: 45,
            interruption_suppression_seconds: 25,
            low_confidence_suppression_seconds: 20,
            min_retrieval_confidence: 0.2,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CoworkingRuntime {
    state: CoworkingState,
    session_mode: SessionMode,
    config: CoworkingConfig,
    user_typing: bool,
    user_speaking: bool,
    last_user_activity_at: Option<u64>,
    last_assistant_spoke_at: Option<u64>,
    last_nudge_at: Option<u64>,
    suppressed_until: Option<u64>,
    last_decision_reason: String,
}

impl Default for CoworkingRuntime {
    fn default() -> Self {
        Self {
            state: CoworkingState::Idle,
            session_mode: SessionMode::Quiet,
            config: CoworkingConfig::default(),
            user_typing: false,
            user_speaking: false,
            last_user_activity_at: None,
            last_assistant_spoke_at: None,
            last_nudge_at: None,
            suppressed_until: None,
            last_decision_reason: "initialized".to_string(),
        }
    }
}

impl CoworkingRuntime {
    pub fn with_proactive_mode(proactive_mode: bool) -> Self {
        let mut runtime = Self::default();
        runtime.config.proactive_mode = proactive_mode;
        runtime.last_decision_reason = if proactive_mode {
            "initialized_proactive_mode_enabled".to_string()
        } else {
            "initialized_proactive_mode_disabled".to_string()
        };
        runtime
    }

    pub fn status(&mut self, now_seconds: u64) -> CoworkingStatusDto {
        self.refresh_state(now_seconds);
        CoworkingStatusDto {
            state: self.state.as_str().to_string(),
            proactive_mode: self.config.proactive_mode,
            user_typing: self.user_typing,
            user_speaking: self.user_speaking,
            session_mode: self.session_mode.as_str().to_string(),
            pause_threshold_seconds: self.config.pause_threshold_seconds,
            nudge_cooldown_seconds: self.config.nudge_cooldown_seconds,
            interruption_suppression_seconds: self.config.interruption_suppression_seconds,
            low_confidence_suppression_seconds: self.config.low_confidence_suppression_seconds,
            cooldown_remaining_seconds: self.cooldown_remaining(now_seconds),
            last_decision_reason: self.last_decision_reason.clone(),
        }
    }

    pub fn set_proactive_mode(&mut self, enabled: bool, now_seconds: u64) -> CoworkingStatusDto {
        self.config.proactive_mode = enabled;
        self.last_decision_reason = if enabled {
            "proactive_mode_enabled".to_string()
        } else {
            "proactive_mode_disabled".to_string()
        };

        if !enabled {
            self.state = CoworkingState::Idle;
        }

        self.status(now_seconds)
    }

    pub fn set_user_typing(&mut self, is_typing: bool, now_seconds: u64) -> CoworkingStatusDto {
        self.user_typing = is_typing;
        if is_typing {
            self.last_user_activity_at = Some(now_seconds);
            self.state = CoworkingState::SilentObserving;
            self.last_decision_reason = "typing_activity".to_string();
        } else {
            self.last_user_activity_at = Some(now_seconds);
            if self.state == CoworkingState::SilentObserving {
                self.state = CoworkingState::AwaitingUser;
            }
            self.last_decision_reason = "typing_stopped".to_string();
        }

        self.status(now_seconds)
    }

    pub fn set_user_speaking(&mut self, is_speaking: bool, now_seconds: u64) -> CoworkingStatusDto {
        self.user_speaking = is_speaking;
        self.last_user_activity_at = Some(now_seconds);

        if is_speaking {
            self.state = CoworkingState::Listening;
            self.last_decision_reason = "user_speaking".to_string();
        } else {
            self.state = CoworkingState::SilentObserving;
            self.last_decision_reason = "user_stopped_speaking".to_string();
        }

        self.status(now_seconds)
    }

    pub fn set_assistant_speaking(
        &mut self,
        is_speaking: bool,
        now_seconds: u64,
    ) -> CoworkingStatusDto {
        if is_speaking {
            self.state = CoworkingState::Speaking;
            self.last_assistant_spoke_at = Some(now_seconds);
            self.last_decision_reason = "assistant_speaking".to_string();
        } else if self.state == CoworkingState::Speaking {
            self.state = CoworkingState::AwaitingUser;
            self.last_decision_reason = "assistant_finished_speaking".to_string();
        }

        self.status(now_seconds)
    }

    pub fn note_user_message(&mut self, message_kind: MessageKind, now_seconds: u64) {
        self.last_user_activity_at = Some(now_seconds);
        self.user_typing = false;
        self.user_speaking = false;
        self.state = CoworkingState::Thinking;
        self.last_decision_reason = "processing_user_message".to_string();

        self.session_mode = if message_kind == MessageKind::UserDirectQuestion {
            SessionMode::Discussion
        } else {
            SessionMode::Quiet
        };
    }

    pub fn note_assistant_answer(&mut self, now_seconds: u64) {
        self.last_assistant_spoke_at = Some(now_seconds);
        self.state = CoworkingState::AwaitingUser;
        self.last_decision_reason = "assistant_answer_sent".to_string();
    }

    pub fn note_assistant_nudge(&mut self, now_seconds: u64) {
        self.last_assistant_spoke_at = Some(now_seconds);
        self.last_nudge_at = Some(now_seconds);
        self.state = CoworkingState::Speaking;
        self.last_decision_reason = "proactive_nudge_generated".to_string();
    }

    pub fn note_interruption(&mut self, now_seconds: u64) -> CoworkingStatusDto {
        if self.state == CoworkingState::Speaking || self.state == CoworkingState::Thinking {
            self.suppressed_until =
                Some(now_seconds + self.config.interruption_suppression_seconds);
            self.state = CoworkingState::Suppressed;
            self.last_decision_reason = "suppressed_after_user_interruption".to_string();
        } else {
            self.last_decision_reason = "interruption_without_active_speech".to_string();
        }

        self.status(now_seconds)
    }

    pub fn note_low_confidence_context(&mut self, now_seconds: u64) {
        self.suppressed_until = Some(now_seconds + self.config.low_confidence_suppression_seconds);
        self.state = CoworkingState::Suppressed;
        self.last_decision_reason = "suppressed_low_confidence_context".to_string();
    }

    pub fn evaluate_gate(
        &mut self,
        now_seconds: u64,
        has_recent_direct_question: bool,
    ) -> ProactiveDecision {
        self.refresh_state(now_seconds);

        if !self.config.proactive_mode {
            self.last_decision_reason = "proactive_mode_disabled".to_string();
            return ProactiveDecision::Skip("proactive_mode_disabled");
        }

        if self.user_speaking {
            self.state = CoworkingState::Listening;
            self.last_decision_reason = "user_speaking".to_string();
            return ProactiveDecision::Skip("user_speaking");
        }

        if self.user_typing {
            self.state = CoworkingState::SilentObserving;
            self.last_decision_reason = "user_typing".to_string();
            return ProactiveDecision::Skip("user_typing");
        }

        if let Some(until) = self.suppressed_until {
            if now_seconds < until {
                self.state = CoworkingState::Suppressed;
                self.last_decision_reason = "suppressed_cooldown".to_string();
                return ProactiveDecision::Skip("suppressed_cooldown");
            }
        }

        if has_recent_direct_question {
            self.state = CoworkingState::AwaitingUser;
            self.last_decision_reason = "direct_question_in_flight".to_string();
            return ProactiveDecision::Skip("direct_question_in_flight");
        }

        if let Some(last_nudge_at) = self.last_nudge_at {
            if now_seconds.saturating_sub(last_nudge_at) < self.config.nudge_cooldown_seconds {
                self.state = CoworkingState::Suppressed;
                self.last_decision_reason = "nudge_cooldown_active".to_string();
                return ProactiveDecision::Skip("nudge_cooldown_active");
            }
        }

        let pause_elapsed = match self.last_user_activity_at {
            Some(last_user_activity_at) => now_seconds.saturating_sub(last_user_activity_at),
            None => {
                self.last_decision_reason = "no_user_activity".to_string();
                return ProactiveDecision::Skip("no_user_activity");
            }
        };

        if pause_elapsed < self.config.pause_threshold_seconds {
            self.state = CoworkingState::SilentObserving;
            self.last_decision_reason = "pause_not_long_enough".to_string();
            return ProactiveDecision::Skip("pause_not_long_enough");
        }

        self.state = CoworkingState::AwaitingUser;
        self.last_decision_reason = "pause_threshold_reached_evaluate_nudge".to_string();
        ProactiveDecision::Evaluate
    }

    fn refresh_state(&mut self, now_seconds: u64) {
        if let Some(until) = self.suppressed_until {
            if now_seconds >= until {
                self.suppressed_until = None;
                if self.user_speaking {
                    self.state = CoworkingState::Listening;
                } else if self.user_typing {
                    self.state = CoworkingState::SilentObserving;
                } else if self.state == CoworkingState::Suppressed {
                    self.state = CoworkingState::AwaitingUser;
                }
                self.last_decision_reason = "suppression_expired".to_string();
            }
        }
    }

    fn cooldown_remaining(&self, now_seconds: u64) -> u64 {
        let mut remaining = 0;

        if let Some(until) = self.suppressed_until {
            if until > now_seconds {
                remaining = remaining.max(until - now_seconds);
            }
        }

        if let Some(last_nudge_at) = self.last_nudge_at {
            let nudge_until = last_nudge_at + self.config.nudge_cooldown_seconds;
            if nudge_until > now_seconds {
                remaining = remaining.max(nudge_until - now_seconds);
            }
        }

        remaining
    }
}

pub enum ProactiveDecision {
    Evaluate,
    Skip(&'static str),
}

pub fn evaluate_proactive_nudge_for_task(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    reasoning: &dyn ReasoningProvider,
    runtime: &mut CoworkingRuntime,
    task_id: i64,
    now_seconds: u64,
) -> Result<ProactiveEvaluationDto> {
    let recent_messages = store.list_recent_chat_messages(task_id, 10)?;
    let has_recent_direct_question = has_unanswered_direct_question(&recent_messages);

    match runtime.evaluate_gate(now_seconds, has_recent_direct_question) {
        ProactiveDecision::Skip(reason) => {
            let status = runtime.status(now_seconds);
            return Ok(ProactiveEvaluationDto {
                status,
                decision_event_type: MessageKind::SystemStatusEvent.as_str().to_string(),
                decision_reason: reason.to_string(),
                nudge: None,
            });
        }
        ProactiveDecision::Evaluate => {}
    }

    let task_summary = store.get_task_summary(task_id)?;
    let retrieval_query = build_proactive_query(&task_summary.summary_text, &recent_messages);
    let retrieved_chunks =
        retrieve_relevant_chunks_with_top_k(store, embeddings, task_id, &retrieval_query, 4)?;

    if retrieved_chunks.is_empty() {
        runtime.note_low_confidence_context(now_seconds);
        let status = runtime.status(now_seconds);
        return Ok(ProactiveEvaluationDto {
            status,
            decision_event_type: MessageKind::SystemStatusEvent.as_str().to_string(),
            decision_reason: "no_retrieved_context".to_string(),
            nudge: None,
        });
    }

    let confidence = retrieved_chunks
        .first()
        .map(|chunk| chunk.similarity_score)
        .unwrap_or(0.0);

    if confidence < runtime.config.min_retrieval_confidence {
        runtime.note_low_confidence_context(now_seconds);
        let status = runtime.status(now_seconds);
        return Ok(ProactiveEvaluationDto {
            status,
            decision_event_type: MessageKind::SystemStatusEvent.as_str().to_string(),
            decision_reason: "low_retrieval_confidence".to_string(),
            nudge: None,
        });
    }

    let prompt = build_nudge_prompt(
        &task_summary.summary_text,
        runtime.session_mode,
        &recent_messages,
        &retrieved_chunks,
    );

    let candidate = reasoning.generate_response(NUDGE_SYSTEM_PROMPT, &prompt)?;
    let Some(nudge_text) = normalize_nudge_response(&candidate) else {
        let status = runtime.status(now_seconds);
        return Ok(ProactiveEvaluationDto {
            status,
            decision_event_type: MessageKind::SystemStatusEvent.as_str().to_string(),
            decision_reason: "model_declined_nudge".to_string(),
            nudge: None,
        });
    };

    store.append_chat_message(
        task_id,
        "assistant",
        "assistant",
        MessageKind::AssistantNudge,
        &nudge_text,
    )?;

    runtime.note_assistant_nudge(now_seconds);
    let status = runtime.status(now_seconds);

    Ok(ProactiveEvaluationDto {
        status,
        decision_event_type: MessageKind::AssistantNudge.as_str().to_string(),
        decision_reason: "grounded_nudge_generated".to_string(),
        nudge: Some(ProactiveNudgeDto {
            message: nudge_text,
            retrieved_chunks,
            confidence,
        }),
    })
}

fn has_unanswered_direct_question(messages: &[crate::models::ChatMessageDto]) -> bool {
    let latest_user_index = messages.iter().rposition(|message| message.role == "user");
    let Some(index) = latest_user_index else {
        return false;
    };

    let user_message = &messages[index];
    if MessageKind::from_db(&user_message.message_kind) != MessageKind::UserDirectQuestion {
        return false;
    }

    !messages[index + 1..].iter().any(|message| {
        message.role == "assistant" && message.message_kind == MessageKind::AssistantAnswer.as_str()
    })
}

fn build_proactive_query(
    summary_text: &str,
    recent_messages: &[crate::models::ChatMessageDto],
) -> String {
    let mut parts = vec![summary_text.trim().to_string()];

    let recent_user_text = recent_messages
        .iter()
        .rev()
        .filter(|message| message.role == "user")
        .take(3)
        .map(|message| message.content.trim())
        .collect::<Vec<&str>>()
        .join(" ");

    if !recent_user_text.is_empty() {
        parts.push(recent_user_text);
    }

    parts.push(
        "rubric requirements primary source evidence course readings section structure thesis"
            .to_string(),
    );

    parts
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<String>>()
        .join(" ")
}

fn build_nudge_prompt(
    task_summary: &str,
    session_mode: SessionMode,
    recent_messages: &[crate::models::ChatMessageDto],
    retrieved_chunks: &[crate::models::RetrievedChunkDto],
) -> String {
    let transcript = recent_messages
        .iter()
        .rev()
        .take(6)
        .map(|message| {
            format!(
                "{} ({}) [{}]: {}",
                message.role, message.message_source, message.message_kind, message.content
            )
        })
        .collect::<Vec<String>>()
        .join("\n");

    let chunks = retrieved_chunks
        .iter()
        .enumerate()
        .map(|(index, chunk)| {
            format!(
                "Chunk {} | {} | score {:.3} | {}",
                index + 1,
                chunk.artifact_file_name,
                chunk.similarity_score,
                chunk.chunk_text
            )
        })
        .collect::<Vec<String>>()
        .join("\n\n");

    format!(
        "Session mode: {}\n\nTask summary:\n{}\n\nRecent messages:\n{}\n\nRetrieved context:\n{}\n\nReturn NO_NUDGE if context is weak.",
        session_mode.as_str(),
        task_summary,
        if transcript.is_empty() {
            "<none>"
        } else {
            &transcript
        },
        chunks
    )
}

fn normalize_nudge_response(response: &str) -> Option<String> {
    let trimmed = response.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.eq_ignore_ascii_case("NO_NUDGE")
        || trimmed.to_ascii_uppercase().starts_with("NO_NUDGE")
    {
        return None;
    }

    let first_sentence = trimmed
        .split_terminator(['.', '!', '?'])
        .next()
        .unwrap_or(trimmed)
        .trim();

    if first_sentence.is_empty() {
        return None;
    }

    let mut normalized = first_sentence.to_string();
    if normalized.len() > 200 {
        normalized.truncate(200);
    }

    if !normalized.ends_with('.') {
        normalized.push('.');
    }

    Some(normalized)
}

pub fn unix_now_seconds() -> Result<u64> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| anyhow!("system clock is before UNIX_EPOCH: {error}"))?;

    Ok(now.as_secs())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use anyhow::Result;

    use crate::{
        embedding::EmbeddingProvider,
        message_kind::{classify_user_message_kind, MessageKind},
        reasoning::ReasoningProvider,
        retrieval::import_artifact_for_task,
        store::TaskStore,
    };

    use super::{
        evaluate_proactive_nudge_for_task, CoworkingRuntime, CoworkingState, ProactiveDecision,
    };

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
                score(&["reading", "readings", "course"]),
                score(&["section", "sections", "structure"]),
                score(&["thesis", "intro", "introduction"]),
                (lower.len() as f32) / 1000.0,
            ])
        }
    }

    #[derive(Clone)]
    struct NudgeReasoningProvider;

    impl ReasoningProvider for NudgeReasoningProvider {
        fn generate_response(&self, _system_prompt: &str, user_prompt: &str) -> Result<String> {
            let lower = user_prompt.to_lowercase();
            if lower.contains("primary source")
                || lower.contains("evidence")
                || lower.contains("course readings")
            {
                Ok("Your intro still needs a primary source and linked course-reading evidence to satisfy the rubric.".to_string())
            } else {
                Ok("NO_NUDGE".to_string())
            }
        }
    }

    fn write_file(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("failed to create parent directory");
        }

        fs::write(path, body).expect("failed to write file");
    }

    fn setup_storymap_store() -> (tempfile::TempDir, TaskStore, i64) {
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let base = temp.path().join("app_local_data");
        let store = TaskStore::initialize(&base).expect("failed to initialize store");
        let task = store
            .create_task("history storymap")
            .expect("failed to create task");
        store
            .set_active_task(task.id)
            .expect("failed to set active task");

        let notes = temp.path().join("fixtures").join("notes.md");
        write_file(
            &notes,
            "Draft notes: intro thesis is broad. Rubric says each section needs a primary source and evidence from course readings.",
        );

        let rubric = temp.path().join("fixtures").join("rubric.txt");
        write_file(
            &rubric,
            "StoryMap rubric: use six sections, include primary source analysis, and tie each claim to course readings with evidence.",
        );

        import_artifact_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task.id,
            &notes.to_string_lossy(),
        )
        .expect("failed to import notes");

        import_artifact_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task.id,
            &rubric.to_string_lossy(),
        )
        .expect("failed to import rubric");

        (temp, store, task.id)
    }

    #[test]
    fn state_transitions_cover_listening_thinking_and_interruption_suppression() {
        let mut runtime = CoworkingRuntime::default();

        runtime.set_user_speaking(true, 10);
        assert_eq!(runtime.status(10).state, CoworkingState::Listening.as_str());

        runtime.note_user_message(MessageKind::UserStatement, 11);
        assert_eq!(runtime.status(11).state, CoworkingState::Thinking.as_str());

        runtime.set_assistant_speaking(true, 12);
        assert_eq!(runtime.status(12).state, CoworkingState::Speaking.as_str());

        runtime.note_interruption(13);
        assert_eq!(
            runtime.status(13).state,
            CoworkingState::Suppressed.as_str()
        );
    }

    #[test]
    fn pause_detection_requires_meaningful_delay_and_resets_on_typing() {
        let mut runtime = CoworkingRuntime::default();
        runtime.note_user_message(MessageKind::UserStatement, 100);

        let decision_short = runtime.evaluate_gate(105, false);
        assert!(matches!(
            decision_short,
            ProactiveDecision::Skip("pause_not_long_enough")
        ));

        runtime.set_user_typing(true, 106);
        runtime.set_user_typing(false, 110);

        let decision_after_reset = runtime.evaluate_gate(118, false);
        assert!(matches!(
            decision_after_reset,
            ProactiveDecision::Skip("pause_not_long_enough")
        ));

        let decision_after_pause = runtime.evaluate_gate(123, false);
        assert!(matches!(decision_after_pause, ProactiveDecision::Evaluate));
    }

    #[test]
    fn cooldown_logic_blocks_immediate_follow_up_nudges() {
        let mut runtime = CoworkingRuntime::default();
        runtime.note_user_message(MessageKind::UserStatement, 0);

        runtime.note_assistant_nudge(20);

        let during_cooldown = runtime.evaluate_gate(30, false);
        assert!(matches!(
            during_cooldown,
            ProactiveDecision::Skip("nudge_cooldown_active")
        ));

        let after_cooldown = runtime.evaluate_gate(80, false);
        assert!(matches!(after_cooldown, ProactiveDecision::Evaluate));
    }

    #[test]
    fn low_confidence_context_triggers_suppression() {
        let mut runtime = CoworkingRuntime::default();
        runtime.note_low_confidence_context(40);

        let decision = runtime.evaluate_gate(45, false);
        assert!(matches!(
            decision,
            ProactiveDecision::Skip("suppressed_cooldown")
        ));
    }

    #[test]
    fn nudge_eligibility_rejects_weak_retrieval_context() {
        let (_temp, store, task_id) = setup_storymap_store();
        let mut runtime = CoworkingRuntime::default();
        runtime.note_user_message(MessageKind::UserStatement, 0);

        struct NoopReasoning;
        impl ReasoningProvider for NoopReasoning {
            fn generate_response(
                &self,
                _system_prompt: &str,
                _user_prompt: &str,
            ) -> Result<String> {
                Ok("NO_NUDGE".to_string())
            }
        }

        #[derive(Clone)]
        struct WeakEmbeddingProvider;
        impl EmbeddingProvider for WeakEmbeddingProvider {
            fn embed_text(&self, _input: &str) -> Result<Vec<f32>> {
                Ok(vec![0.0, 0.0, 0.0, 0.0])
            }
        }

        let evaluation = evaluate_proactive_nudge_for_task(
            &store,
            &WeakEmbeddingProvider,
            &NoopReasoning,
            &mut runtime,
            task_id,
            20,
        )
        .expect("failed proactive evaluation");

        assert!(evaluation.nudge.is_none());
        assert!(
            evaluation.decision_reason == "low_retrieval_confidence"
                || evaluation.decision_reason == "no_retrieved_context"
        );
    }

    #[test]
    fn proactive_mode_off_never_generates_nudge() {
        let (_temp, store, task_id) = setup_storymap_store();
        let mut runtime = CoworkingRuntime::default();
        runtime.set_proactive_mode(false, 0);
        runtime.note_user_message(MessageKind::UserStatement, 0);

        let evaluation = evaluate_proactive_nudge_for_task(
            &store,
            &KeywordEmbeddingProvider,
            &NudgeReasoningProvider,
            &mut runtime,
            task_id,
            20,
        )
        .expect("failed proactive evaluation");

        assert!(evaluation.nudge.is_none());
        assert_eq!(evaluation.decision_reason, "proactive_mode_disabled");
    }

    #[test]
    fn proactive_mode_on_generates_grounded_nudge_after_pause() {
        let (_temp, store, task_id) = setup_storymap_store();
        let mut runtime = CoworkingRuntime::default();

        store
            .append_chat_message(
                task_id,
                "user",
                "text",
                MessageKind::UserStatement,
                "I drafted the intro and main points.",
            )
            .expect("failed to append user draft message");

        runtime.note_user_message(MessageKind::UserStatement, 0);

        let evaluation = evaluate_proactive_nudge_for_task(
            &store,
            &KeywordEmbeddingProvider,
            &NudgeReasoningProvider,
            &mut runtime,
            task_id,
            20,
        )
        .expect("failed proactive evaluation");

        let nudge = evaluation.nudge.expect("expected proactive nudge");
        assert!(nudge.message.to_lowercase().contains("primary source"));
        assert!(!nudge.retrieved_chunks.is_empty());
    }

    #[test]
    fn typing_activity_suppresses_proactive_nudges() {
        let (_temp, store, task_id) = setup_storymap_store();
        let mut runtime = CoworkingRuntime::default();

        runtime.note_user_message(MessageKind::UserStatement, 0);
        runtime.set_user_typing(true, 20);

        let evaluation = evaluate_proactive_nudge_for_task(
            &store,
            &KeywordEmbeddingProvider,
            &NudgeReasoningProvider,
            &mut runtime,
            task_id,
            35,
        )
        .expect("failed proactive evaluation");

        assert!(evaluation.nudge.is_none());
        assert_eq!(evaluation.decision_reason, "user_typing");
    }

    #[test]
    fn interruption_suppression_blocks_immediate_proactive_nudge() {
        let (_temp, store, task_id) = setup_storymap_store();
        let mut runtime = CoworkingRuntime::default();

        runtime.note_user_message(MessageKind::UserStatement, 0);
        runtime.set_assistant_speaking(true, 10);
        runtime.note_interruption(11);

        let evaluation = evaluate_proactive_nudge_for_task(
            &store,
            &KeywordEmbeddingProvider,
            &NudgeReasoningProvider,
            &mut runtime,
            task_id,
            20,
        )
        .expect("failed proactive evaluation");

        assert!(evaluation.nudge.is_none());
        assert_eq!(evaluation.decision_reason, "suppressed_cooldown");
    }

    #[test]
    fn history_storymap_scenario_pause_can_generate_grounded_nudge() {
        let (_temp, store, task_id) = setup_storymap_store();
        let mut runtime = CoworkingRuntime::default();

        store
            .append_chat_message(
                task_id,
                "user",
                "text",
                MessageKind::UserStatement,
                "I drafted my intro and maybe one section.",
            )
            .expect("failed to append scenario user message");
        runtime.note_user_message(MessageKind::UserStatement, 0);

        let evaluation = evaluate_proactive_nudge_for_task(
            &store,
            &KeywordEmbeddingProvider,
            &NudgeReasoningProvider,
            &mut runtime,
            task_id,
            25,
        )
        .expect("failed proactive evaluation");

        let nudge = evaluation.nudge.expect("expected nudge for scenario");
        let lower = nudge.message.to_lowercase();
        assert!(lower.contains("primary source") || lower.contains("evidence"));
    }

    #[test]
    fn history_storymap_scenario_typing_means_do_nothing() {
        let (_temp, store, task_id) = setup_storymap_store();
        let mut runtime = CoworkingRuntime::default();

        runtime.note_user_message(MessageKind::UserStatement, 0);
        runtime.set_user_typing(true, 20);

        let evaluation = evaluate_proactive_nudge_for_task(
            &store,
            &KeywordEmbeddingProvider,
            &NudgeReasoningProvider,
            &mut runtime,
            task_id,
            40,
        )
        .expect("failed proactive evaluation");

        assert!(evaluation.nudge.is_none());
        assert_eq!(evaluation.decision_reason, "user_typing");
    }

    #[test]
    fn history_storymap_scenario_direct_question_uses_answer_path_not_nudge() {
        let (_temp, store, task_id) = setup_storymap_store();
        let mut runtime = CoworkingRuntime::default();

        store
            .append_chat_message(
                task_id,
                "user",
                "text",
                classify_user_message_kind("What are the primary source requirements?"),
                "What are the primary source requirements?",
            )
            .expect("failed to append direct question");
        runtime.note_user_message(MessageKind::UserDirectQuestion, 0);

        let evaluation = evaluate_proactive_nudge_for_task(
            &store,
            &KeywordEmbeddingProvider,
            &NudgeReasoningProvider,
            &mut runtime,
            task_id,
            20,
        )
        .expect("failed proactive evaluation");

        assert!(evaluation.nudge.is_none());
        assert_eq!(evaluation.decision_reason, "direct_question_in_flight");
    }
}
