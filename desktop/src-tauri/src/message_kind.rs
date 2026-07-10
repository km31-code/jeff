#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    UserDirectQuestion,
    UserStatement,
    AssistantAnswer,
    AssistantNudge,
    AssistantRevisionProposal,
    AssistantRevisionStatus,
    SystemStatusEvent,
    // phase 12: streaming turn states. partial is transient (row exists while
    // streaming); interrupted means the user or jeff cut the turn before
    // completion. list_messages returns both to the frontend.
    AssistantPartial,
    AssistantInterrupted,
    // phase 28: proactive messages are stored in the chat thread as normal
    // assistant turns, with the reason preserved in the message kind.
    AssistantProactiveReorientation,
    AssistantProactiveDrift,
    AssistantProactiveBlocker,
    AssistantProactiveDeadline,
    AssistantProactiveSpeculativeSubtask,
    // apex c3: daily judgment rituals delivered as conversation-shaped bubbles.
    AssistantProactiveBriefing,
    AssistantProactiveDebrief,
}

impl MessageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserDirectQuestion => "user_direct_question",
            Self::UserStatement => "user_statement",
            Self::AssistantAnswer => "assistant_answer",
            Self::AssistantNudge => "assistant_nudge",
            Self::AssistantRevisionProposal => "assistant_revision_proposal",
            Self::AssistantRevisionStatus => "assistant_revision_status",
            Self::SystemStatusEvent => "system_status_event",
            Self::AssistantPartial => "assistant_partial",
            Self::AssistantInterrupted => "assistant_interrupted",
            Self::AssistantProactiveReorientation => "proactive_reorientation",
            Self::AssistantProactiveDrift => "proactive_drift",
            Self::AssistantProactiveBlocker => "proactive_blocker",
            Self::AssistantProactiveDeadline => "proactive_deadline",
            Self::AssistantProactiveSpeculativeSubtask => "proactive_speculative_subtask",
            Self::AssistantProactiveBriefing => "proactive_briefing",
            Self::AssistantProactiveDebrief => "proactive_debrief",
        }
    }

    pub fn from_db(value: &str) -> Self {
        match value {
            "user_direct_question" => Self::UserDirectQuestion,
            "assistant_answer" => Self::AssistantAnswer,
            "assistant_nudge" => Self::AssistantNudge,
            "assistant_revision_proposal" => Self::AssistantRevisionProposal,
            "assistant_revision_status" => Self::AssistantRevisionStatus,
            "system_status_event" => Self::SystemStatusEvent,
            "assistant_partial" => Self::AssistantPartial,
            "assistant_interrupted" => Self::AssistantInterrupted,
            "proactive_reorientation" | "assistant_proactive" => {
                Self::AssistantProactiveReorientation
            }
            "proactive_drift" => Self::AssistantProactiveDrift,
            "proactive_blocker" => Self::AssistantProactiveBlocker,
            "proactive_deadline" => Self::AssistantProactiveDeadline,
            "proactive_speculative_subtask" => Self::AssistantProactiveSpeculativeSubtask,
            "proactive_briefing" => Self::AssistantProactiveBriefing,
            "proactive_debrief" => Self::AssistantProactiveDebrief,
            _ => Self::UserStatement,
        }
    }

    pub fn from_proactive_kind(kind: &str) -> Option<Self> {
        match kind {
            "proactive_reorientation" => Some(Self::AssistantProactiveReorientation),
            "proactive_drift" => Some(Self::AssistantProactiveDrift),
            "proactive_blocker" => Some(Self::AssistantProactiveBlocker),
            "proactive_deadline" => Some(Self::AssistantProactiveDeadline),
            "proactive_speculative_subtask" => Some(Self::AssistantProactiveSpeculativeSubtask),
            "proactive_briefing" => Some(Self::AssistantProactiveBriefing),
            "proactive_debrief" => Some(Self::AssistantProactiveDebrief),
            _ => None,
        }
    }
}

pub fn classify_user_message_kind(message: &str) -> MessageKind {
    let trimmed = message.trim();
    let lower = trimmed.to_ascii_lowercase();

    if trimmed.ends_with('?') {
        return MessageKind::UserDirectQuestion;
    }

    let question_starts = [
        "what ",
        "why ",
        "how ",
        "when ",
        "where ",
        "who ",
        "can you",
        "could you",
        "would you",
        "should i",
        "do i",
    ];

    if question_starts
        .iter()
        .any(|prefix| lower.starts_with(prefix))
    {
        MessageKind::UserDirectQuestion
    } else {
        MessageKind::UserStatement
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_user_message_kind, MessageKind};

    #[test]
    fn classify_user_message_kind_detects_questions() {
        assert_eq!(
            classify_user_message_kind("What are the requirements?"),
            MessageKind::UserDirectQuestion
        );
        assert_eq!(
            classify_user_message_kind("how should i structure this"),
            MessageKind::UserDirectQuestion
        );
    }

    #[test]
    fn classify_user_message_kind_detects_statements() {
        assert_eq!(
            classify_user_message_kind("I drafted the intro paragraph"),
            MessageKind::UserStatement
        );
    }

    #[test]
    fn proactive_message_kinds_round_trip_from_db() {
        for kind in [
            "proactive_reorientation",
            "proactive_drift",
            "proactive_blocker",
            "proactive_deadline",
            "proactive_speculative_subtask",
            "proactive_briefing",
            "proactive_debrief",
        ] {
            assert_eq!(MessageKind::from_db(kind).as_str(), kind);
        }

        assert_eq!(
            MessageKind::from_db("assistant_proactive").as_str(),
            "proactive_reorientation"
        );
    }
}
