use crate::model_router::{join_system_blocks, CacheHint, SystemBlock};

pub const ASSESSMENT_INSTRUCTION: &str = "Before presenting a result, write one first-person sentence naming the judgment you made: the tradeoff, what got stronger, or what got softer. No hedging. Example: 'I moved the argument to the front - loses the setup but lands faster.' Then give the result.";
pub const SOFT_ASSESSMENT_INSTRUCTION: &str = "Note the key tradeoff you made in one short first-person clause. Example: 'I tried a shorter path here.' Do not lead with a strong opinion.";

const CHAT_BEHAVIOR_PROMPT: &str = "When context chunks are provided in the user prompt, prioritize them and cite relevant material. When the user asks what they are currently doing, working on, or has open, the Active Window section is the primary signal. If no retrieved context is relevant, still help: answer, give next steps, or ask one necessary clarifying question. One to three sentences unless asked for more.";

const REORIENTATION_BEHAVIOR_PROMPT: &str = "The user returned to this task. Write one short sentence, maximum 25 words, that starts a conversation from what you have been watching. Be specific. Do not sound like a notification.";

pub fn base_character_prompt() -> &'static str {
    "You are Jeff, a coworker who works beside the user. Be terse, direct, and specific. Start with the point. Use first person when giving a judgment. Do not flatter, confirm receipt, or use filler phrases like Certainly, Absolutely, Of course, Great question, Sure thing, or Happy to help. Hedge only for real uncertainty, in one clause, then keep moving. If you disagree, state it directly once and then defer to the user's call. Do not narrate your process. Before presenting a result, write one first-person sentence naming the judgment you made: the tradeoff, what got stronger, or what got softer. No hedging. Example: 'I moved the argument to the front - loses the setup but lands faster.' Then give the result. Do not add a summary or recap after giving the result. Do not ask for permission when the user has given a clear instruction. Do not open with a statement about what you are about to do."
}

#[derive(Debug, Clone, Default)]
pub struct ChatContext {
    pub task_summary: String,
    pub active_window: Option<String>,
    pub profile_injection: Option<String>,
    pub relational_context: Option<String>,
    pub memory_recall: Option<String>,
    pub recent_transcript: Vec<String>,
    pub is_first_message: bool,
    pub snapshot_summary: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RevisionContext {
    pub task_summary: String,
    pub target_description: String,
    pub instruction: String,
    pub profile_injection: Option<String>,
    pub memory_recall: Option<String>,
    pub prefers_opinions: Option<f32>,
    pub snapshot_summary: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ReorientationContext {
    pub task_summary: String,
    pub last_active: String,
    pub profile_injection: Option<String>,
    pub active_window: Option<String>,
    pub calendar_context: Option<String>,
    pub memory_recall: Option<String>,
    pub snapshot_summary: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SubtaskContext {
    pub task_summary: String,
    pub subtask_title: String,
    pub execution_type: String,
    pub profile_injection: Option<String>,
    pub prefers_opinions: Option<f32>,
}

pub fn build_chat_system_blocks(ctx: &ChatContext) -> Vec<SystemBlock> {
    let mut blocks = vec![stable_block(base_character_prompt())];
    let mut session = Vec::new();
    push_optional(&mut session, ctx.profile_injection.as_deref());
    push_optional(&mut session, ctx.relational_context.as_deref());
    push_optional(&mut session, ctx.memory_recall.as_deref());

    if !ctx.task_summary.trim().is_empty() {
        session.push(format!("Task summary:\n{}", ctx.task_summary.trim()));
    }
    session.push(CHAT_BEHAVIOR_PROMPT.to_string());
    push_block(&mut blocks, CacheHint::Session, session.join("\n\n"));

    let mut volatile = Vec::new();
    push_optional(&mut volatile, ctx.snapshot_summary.as_deref());
    if ctx.is_first_message {
        let joining = match ctx.active_window.as_deref().filter(|value| !value.trim().is_empty()) {
            Some(active) => format!(
                "The user just told you what they're working on for the first time. Their current document is: {active}. Orient yourself as a coworker joining this work."
            ),
            None => "The user just told you what they're working on for the first time. Orient yourself as a coworker joining this work.".to_string(),
        };
        volatile.push(joining);
    } else {
        push_optional(&mut volatile, ctx.active_window.as_deref());
    }

    if !ctx.recent_transcript.is_empty() {
        volatile.push(format!(
            "Recent transcript:\n{}",
            ctx.recent_transcript.join("\n")
        ));
    }
    push_block(&mut blocks, CacheHint::Volatile, volatile.join("\n\n"));

    blocks
}

#[allow(dead_code)]
pub fn build_chat_system_prompt(ctx: &ChatContext) -> String {
    join_system_blocks(&build_chat_system_blocks(ctx))
}

pub fn build_revision_system_blocks(ctx: &RevisionContext) -> Vec<SystemBlock> {
    let mut blocks = vec![stable_block(base_character_prompt())];
    let mut session = Vec::new();
    push_optional(&mut session, ctx.profile_injection.as_deref());
    push_optional(&mut session, ctx.memory_recall.as_deref());
    push_labeled(&mut session, "Task summary", &ctx.task_summary);
    push_block(&mut blocks, CacheHint::Session, session.join("\n\n"));

    let mut volatile = Vec::new();
    push_optional(&mut volatile, ctx.snapshot_summary.as_deref());
    push_labeled(&mut volatile, "Target", &ctx.target_description);
    push_labeled(&mut volatile, "Instruction", &ctx.instruction);
    let assessment_instruction = assessment_instruction_for_preference(ctx.prefers_opinions);
    volatile.push(format!(
        "{assessment_instruction}\nRewrite only the target text while preserving intent and factual grounding from provided context. Satisfy the user's instruction literally: remove the weakness the instruction names, keep concrete useful consequences from the source, and do not keep phrasing the instruction is targeting. Do not invent names, counts, metrics, dates, timelines, environments, commitments, mechanisms, legal facts, product features, user actions, implementation details, or causal triggers that are not in the target text or provided context. If facts are missing, make the revision more precise through structure, scope, contrast, and wording rather than false specificity. When the user asks for specificity but the source lacks specifics, name the missing concrete category only if useful; do not fabricate examples. Before returning, compare every concrete detail in proposed_text against the target/context and remove unsupported details. Do not use placeholders, brackets, TODO text, or instructions to fill in missing facts. Avoid hype, meta framing, filler transitions, absolute claims, and repeated weak phrasing from the original. Return strict JSON with keys: proposed_text (string), rationale (string), confidence (number 0-1), grounding_notes (string). Put the assessment sentence in rationale. If context is weak, keep edits conservative and clearly mark weak grounding_notes."
    ));
    push_block(&mut blocks, CacheHint::Volatile, volatile.join("\n\n"));

    blocks
}

#[allow(dead_code)]
pub fn build_revision_system_prompt(ctx: &RevisionContext) -> String {
    join_system_blocks(&build_revision_system_blocks(ctx))
}

pub fn build_reorientation_system_blocks(ctx: &ReorientationContext) -> Vec<SystemBlock> {
    let mut blocks = vec![stable_block(base_character_prompt())];
    let mut session = Vec::new();
    push_optional(&mut session, ctx.profile_injection.as_deref());
    push_optional(&mut session, ctx.memory_recall.as_deref());
    push_labeled(&mut session, "Task summary", &ctx.task_summary);
    push_block(&mut blocks, CacheHint::Session, session.join("\n\n"));

    let mut volatile = Vec::new();
    push_optional(&mut volatile, ctx.snapshot_summary.as_deref());
    push_labeled(&mut volatile, "Last active", &ctx.last_active);
    push_optional(&mut volatile, ctx.active_window.as_deref());
    push_optional(&mut volatile, ctx.calendar_context.as_deref());
    volatile.push(REORIENTATION_BEHAVIOR_PROMPT.to_string());
    push_block(&mut blocks, CacheHint::Volatile, volatile.join("\n\n"));

    blocks
}

#[allow(dead_code)]
pub fn build_reorientation_system_prompt(ctx: &ReorientationContext) -> String {
    join_system_blocks(&build_reorientation_system_blocks(ctx))
}

pub fn build_subtask_system_blocks(ctx: &SubtaskContext) -> Vec<SystemBlock> {
    let mut blocks = vec![stable_block(base_character_prompt())];
    let mut session = Vec::new();
    push_optional(&mut session, ctx.profile_injection.as_deref());
    push_labeled(&mut session, "Task summary", &ctx.task_summary);
    push_labeled(&mut session, "Subtask", &ctx.subtask_title);
    push_labeled(&mut session, "Execution type", &ctx.execution_type);
    push_block(&mut blocks, CacheHint::Session, session.join("\n\n"));

    let assessment_instruction = assessment_instruction_for_preference(ctx.prefers_opinions);

    let instruction = match ctx.execution_type.as_str() {
        "subtask_suggestion" => {
            "Suggest one small, useful subtask in the current task materials. Return strict JSON with keys: title, description, execution_type, reason. Execution type must be one of: draft_generation, expansion, synthesis, targeted_research_synthesis.".to_string()
        }
        "chain_planning" => {
            "You are Jeff's subtask chain planner. Produce a step-by-step execution plan. Return strict JSON: {\"steps\": [{\"step_type\": \"retrieval|llm_call|file_write_proposal\", \"description\": \"...\", \"proposed_path\": \"relative/path.md\"}]}. Maximum 5 steps. Only include proposed_path for file_write_proposal steps. Use relative paths only.".to_string()
        }
        "step_llm" => {
            "Complete the described step using only the provided context and prior step outputs. Be concise and grounded.".to_string()
        }
        "file_write_proposal" => {
            "You are Jeff's bounded file writer. Draft the requested file content grounded in the provided context and prior step outputs. Output raw file content only: no JSON wrapper and no explanation outside the content.".to_string()
        }
        _ => {
            format!("{assessment_instruction}\nComplete exactly one controlled subtask using only provided context. Never claim external research. Return strict JSON with keys: result_summary (string), result_payload (string), grounding_notes (string), confidence (number 0..1). Put Jeff's assessment sentence in result_summary.")
        }
    };
    push_block(&mut blocks, CacheHint::Volatile, instruction);
    blocks
}

#[allow(dead_code)]
pub fn build_subtask_system_prompt(ctx: &SubtaskContext) -> String {
    join_system_blocks(&build_subtask_system_blocks(ctx))
}

pub fn assessment_instruction_for_preference(prefers_opinions: Option<f32>) -> &'static str {
    match prefers_opinions {
        Some(score) if score < 0.3 => SOFT_ASSESSMENT_INSTRUCTION,
        _ => ASSESSMENT_INSTRUCTION,
    }
}

pub fn strip_filler_phrases(text: &str) -> String {
    let mut cleaned = text.to_string();
    for phrase in [
        "Certainly,",
        "Certainly!",
        "Absolutely,",
        "Absolutely!",
        "Of course,",
        "Of course!",
        "Great question!",
        "Sure thing,",
        "Sure thing!",
        "Happy to help!",
        "I'd be happy to",
        "I'll go ahead and",
        "I've gone ahead and",
    ] {
        cleaned = cleaned.replace(phrase, "");
    }

    cleaned
        .lines()
        .map(|line| line.trim_start())
        .collect::<Vec<&str>>()
        .join("\n")
        .trim_start()
        .to_string()
}

fn push_optional(parts: &mut Vec<String>, value: Option<&str>) {
    if let Some(value) = value {
        if !value.trim().is_empty() {
            parts.push(value.trim().to_string());
        }
    }
}

fn push_labeled(parts: &mut Vec<String>, label: &str, value: &str) {
    if !value.trim().is_empty() {
        parts.push(format!("{label}:\n{}", value.trim()));
    }
}

fn stable_block(text: &str) -> SystemBlock {
    SystemBlock {
        text: text.to_string(),
        cache_hint: CacheHint::Stable,
    }
}

fn push_block(blocks: &mut Vec<SystemBlock>, cache_hint: CacheHint, text: String) {
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        blocks.push(SystemBlock {
            text: trimmed.to_string(),
            cache_hint,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_character_prompt_under_300_tokens() {
        assert!(base_character_prompt().chars().count() <= 1200);
    }

    #[test]
    fn strip_filler_phrases_removes_all_patterns() {
        let text = "Certainly, alpha. Absolutely, beta. Of course, gamma. Great question! delta. Sure thing, epsilon. Happy to help! I'd be happy to revise. I'll go ahead and draft. I've gone ahead and summarized.";
        let cleaned = strip_filler_phrases(text);
        for phrase in [
            "Certainly",
            "Absolutely",
            "Of course",
            "Great question",
            "Sure thing",
            "Happy to help",
            "I'd be happy to",
            "I'll go ahead and",
            "I've gone ahead and",
        ] {
            assert!(!cleaned.contains(phrase));
        }
        assert!(cleaned.contains("alpha"));
        assert!(cleaned.contains("summarized"));
    }

    #[test]
    fn chat_system_prompt_contains_character_block() {
        let prompt = build_chat_system_prompt(&ChatContext {
            task_summary: "finish the essay".to_string(),
            active_window: None,
            profile_injection: None,
            relational_context: None,
            memory_recall: None,
            recent_transcript: Vec::new(),
            is_first_message: false,
            snapshot_summary: None,
        });
        assert!(prompt.contains(base_character_prompt()));
        assert!(prompt.contains("finish the essay"));
    }

    #[test]
    fn a2_block_one_is_byte_stable_across_chat_builds() {
        let first = build_chat_system_blocks(&ChatContext {
            task_summary: "finish the essay".to_string(),
            active_window: Some("Draft.md".to_string()),
            profile_injection: Some("User prefers direct edits.".to_string()),
            relational_context: Some("Long-running writing task.".to_string()),
            memory_recall: Some(
                "Memory recall:\n- preference: User likes concise critique.".to_string(),
            ),
            recent_transcript: vec!["user: tighten this".to_string()],
            is_first_message: false,
            snapshot_summary: Some("Draft is open.".to_string()),
        });
        let second = build_chat_system_blocks(&ChatContext {
            task_summary: "finish the essay".to_string(),
            active_window: Some("Different.md".to_string()),
            profile_injection: Some("User prefers direct edits.".to_string()),
            relational_context: Some("Long-running writing task.".to_string()),
            memory_recall: Some(
                "Memory recall:\n- preference: User likes concise critique.".to_string(),
            ),
            recent_transcript: vec!["user: continue".to_string()],
            is_first_message: false,
            snapshot_summary: Some("Different snapshot.".to_string()),
        });

        assert_eq!(first[0].cache_hint, CacheHint::Stable);
        assert_eq!(first[0].text.as_bytes(), second[0].text.as_bytes());
        assert_eq!(first[0].text, base_character_prompt());
    }

    #[test]
    fn a2_chat_blocks_are_ordered_by_cache_stability() {
        let blocks = build_chat_system_blocks(&ChatContext {
            task_summary: "task".to_string(),
            active_window: Some("active".to_string()),
            profile_injection: Some("profile".to_string()),
            relational_context: Some("relational".to_string()),
            memory_recall: Some("memory".to_string()),
            recent_transcript: vec!["recent".to_string()],
            is_first_message: false,
            snapshot_summary: Some("snapshot".to_string()),
        });

        assert_eq!(blocks[0].cache_hint, CacheHint::Stable);
        assert_eq!(blocks[1].cache_hint, CacheHint::Session);
        assert_eq!(blocks[2].cache_hint, CacheHint::Volatile);
        assert!(blocks[1].text.contains("profile"));
        assert!(blocks[1].text.contains("memory"));
        assert!(blocks[2].text.contains("snapshot"));
    }

    #[test]
    fn b5_chat_recall_sits_after_relational_context_in_session_block() {
        let blocks = build_chat_system_blocks(&ChatContext {
            task_summary: "task".to_string(),
            active_window: None,
            profile_injection: Some("profile".to_string()),
            relational_context: Some("relational context".to_string()),
            memory_recall: Some("Memory recall:\n- preference: direct assessments".to_string()),
            recent_transcript: Vec::new(),
            is_first_message: false,
            snapshot_summary: None,
        });
        let session = &blocks[1].text;
        let relational_index = session.find("relational context").unwrap();
        let recall_index = session.find("Memory recall").unwrap();
        let task_index = session.find("Task summary").unwrap();
        assert!(relational_index < recall_index);
        assert!(recall_index < task_index);
        assert_eq!(blocks[1].cache_hint, CacheHint::Session);
    }

    #[test]
    fn a2_scripted_conversation_cacheable_ratio_exceeds_seventy_percent() {
        let mut total_chars = 0usize;
        let mut cacheable_after_first_chars = 0usize;
        for turn in 0..20 {
            let blocks = build_chat_system_blocks(&ChatContext {
                task_summary: "Write the history essay with a precise thesis.".to_string(),
                active_window: Some(format!("Draft turn {turn}")),
                profile_injection: Some("User prefers direct critique.".to_string()),
                relational_context: Some(
                    "The user has been revising this essay for an hour.".to_string(),
                ),
                memory_recall: Some(
                    "Memory recall:\n- preference: User prefers direct critique.".to_string(),
                ),
                recent_transcript: vec![format!("user turn {turn}")],
                is_first_message: false,
                snapshot_summary: Some(format!("Volatile snapshot {turn}")),
            });
            for block in blocks {
                total_chars += block.text.len();
                if turn > 0 && matches!(block.cache_hint, CacheHint::Stable | CacheHint::Session) {
                    cacheable_after_first_chars += block.text.len();
                }
            }
        }
        let ratio = cacheable_after_first_chars as f64 / total_chars as f64;
        assert!(
            ratio > 0.70,
            "expected scripted cacheable ratio > 70%, got {:.1}%",
            ratio * 100.0
        );
    }
}
