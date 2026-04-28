pub const ASSESSMENT_INSTRUCTION: &str = "Before presenting a result, write one first-person sentence naming the judgment you made: the tradeoff, what got stronger, or what got softer. No hedging. Example: 'I moved the argument to the front - loses the setup but lands faster.' Then give the result.";

pub fn base_character_prompt() -> &'static str {
    "You are Jeff, a coworker who works beside the user. Be terse, direct, and specific. Start with the point. Use first person when giving a judgment. Do not flatter, confirm receipt, or use filler phrases like Certainly, Absolutely, Of course, Great question, Sure thing, or Happy to help. Hedge only for real uncertainty, in one clause, then keep moving. If you disagree, state it directly once and then defer to the user's call. Do not narrate your process. Before presenting a result, write one first-person sentence naming the judgment you made: the tradeoff, what got stronger, or what got softer. No hedging. Example: 'I moved the argument to the front - loses the setup but lands faster.' Then give the result."
}

#[derive(Debug, Clone, Default)]
pub struct ChatContext {
    pub task_summary: String,
    pub active_window: Option<String>,
    pub profile_injection: Option<String>,
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
}

#[derive(Debug, Clone, Default)]
pub struct ReorientationContext {
    pub task_summary: String,
    pub last_active: String,
    pub profile_injection: Option<String>,
    pub active_window: Option<String>,
    pub calendar_context: Option<String>,
    pub snapshot_summary: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SubtaskContext {
    pub task_summary: String,
    pub subtask_title: String,
    pub execution_type: String,
    pub profile_injection: Option<String>,
}

pub fn build_chat_system_prompt(ctx: &ChatContext) -> String {
    let mut parts = vec![base_character_prompt().to_string()];
    push_optional(&mut parts, ctx.profile_injection.as_deref());
    push_optional(&mut parts, ctx.snapshot_summary.as_deref());

    if ctx.is_first_message {
        let joining = match ctx.active_window.as_deref().filter(|value| !value.trim().is_empty()) {
            Some(active) => format!(
                "The user just told you what they're working on for the first time. Their current document is: {active}. Orient yourself as a coworker joining this work."
            ),
            None => "The user just told you what they're working on for the first time. Orient yourself as a coworker joining this work.".to_string(),
        };
        parts.push(joining);
    } else {
        push_optional(&mut parts, ctx.active_window.as_deref());
    }

    if !ctx.task_summary.trim().is_empty() {
        parts.push(format!("Task summary:\n{}", ctx.task_summary.trim()));
    }

    if !ctx.recent_transcript.is_empty() {
        parts.push(format!(
            "Recent transcript:\n{}",
            ctx.recent_transcript.join("\n")
        ));
    }

    parts.push(
        "When context chunks are provided in the user prompt, prioritize them and cite relevant material. When the user asks what they are currently doing, working on, or has open, the Active Window section is the primary signal. If no retrieved context is relevant, still help: answer, give next steps, or ask one necessary clarifying question. One to three sentences unless asked for more."
            .to_string(),
    );

    parts.join("\n\n")
}

pub fn build_revision_system_prompt(ctx: &RevisionContext) -> String {
    let mut parts = vec![base_character_prompt().to_string()];
    push_optional(&mut parts, ctx.profile_injection.as_deref());
    push_labeled(&mut parts, "Task summary", &ctx.task_summary);
    push_labeled(&mut parts, "Target", &ctx.target_description);
    push_labeled(&mut parts, "Instruction", &ctx.instruction);
    parts.push(format!(
        "{ASSESSMENT_INSTRUCTION}\nRewrite only the target text while preserving intent and factual grounding from provided context. Return strict JSON with keys: proposed_text (string), rationale (string), confidence (number 0-1), grounding_notes (string). Put the assessment sentence in rationale. If context is weak, keep edits conservative and clearly mark weak grounding_notes."
    ));
    parts.join("\n\n")
}

pub fn build_reorientation_system_prompt(ctx: &ReorientationContext) -> String {
    let mut parts = vec![base_character_prompt().to_string()];
    push_optional(&mut parts, ctx.profile_injection.as_deref());
    push_optional(&mut parts, ctx.snapshot_summary.as_deref());
    push_labeled(&mut parts, "Task summary", &ctx.task_summary);
    push_labeled(&mut parts, "Last active", &ctx.last_active);
    push_optional(&mut parts, ctx.active_window.as_deref());
    push_optional(&mut parts, ctx.calendar_context.as_deref());
    parts.push(
        "The user returned to this task. Write one short sentence, maximum 25 words, that starts a conversation from what you have been watching. Be specific. Do not sound like a notification."
            .to_string(),
    );
    parts.join("\n\n")
}

pub fn build_subtask_system_prompt(ctx: &SubtaskContext) -> String {
    let mut parts = vec![base_character_prompt().to_string()];
    push_optional(&mut parts, ctx.profile_injection.as_deref());
    push_labeled(&mut parts, "Task summary", &ctx.task_summary);
    push_labeled(&mut parts, "Subtask", &ctx.subtask_title);
    push_labeled(&mut parts, "Execution type", &ctx.execution_type);

    let instruction = match ctx.execution_type.as_str() {
        "subtask_suggestion" => {
            "Suggest one small, useful subtask in the current task materials. Return strict JSON with keys: title, description, execution_type, reason. Execution type must be one of: draft_generation, expansion, synthesis, targeted_research_synthesis."
        }
        "chain_planning" => {
            "You are Jeff's subtask chain planner. Produce a step-by-step execution plan. Return strict JSON: {\"steps\": [{\"step_type\": \"retrieval|llm_call|file_write_proposal\", \"description\": \"...\", \"proposed_path\": \"relative/path.md\"}]}. Maximum 5 steps. Only include proposed_path for file_write_proposal steps. Use relative paths only."
        }
        "step_llm" => {
            "Complete the described step using only the provided context and prior step outputs. Be concise and grounded."
        }
        "file_write_proposal" => {
            "You are Jeff's bounded file writer. Draft the requested file content grounded in the provided context and prior step outputs. Output raw file content only: no JSON wrapper and no explanation outside the content."
        }
        _ => {
            "Complete exactly one controlled subtask using only provided context. Never claim external research. Return strict JSON with keys: result_summary (string), result_payload (string), grounding_notes (string), confidence (number 0..1). Put Jeff's assessment sentence in result_summary."
        }
    };
    parts.push(instruction.to_string());
    parts.join("\n\n")
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
            recent_transcript: Vec::new(),
            is_first_message: false,
            snapshot_summary: None,
        });
        assert!(prompt.contains(base_character_prompt()));
        assert!(prompt.contains("finish the essay"));
    }
}
