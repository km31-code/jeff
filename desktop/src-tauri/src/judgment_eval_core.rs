use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JudgmentStage2Input {
    pub attention_state: String,
    pub focus_score: f32,
    pub content_idle_seconds: Option<u32>,
    pub snapshot_confidence: f32,
    pub quiet_mode: bool,
    pub natural_boundary: bool,
    pub reason_type: String,
    pub candidate_confidence: f32,
    pub candidate_importance: f32,
    pub deadline_minutes: Option<i64>,
    pub ignored_count: u32,
    pub engaged_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JudgmentStage2Output {
    pub decision: String,
    pub channel: String,
    pub reason: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JudgmentEvalScenario {
    pub id: String,
    pub category: String,
    pub snapshot: JudgmentEvalSnapshot,
    pub candidate: JudgmentEvalCandidate,
    pub ledger: JudgmentEvalLedger,
    pub expected: JudgmentEvalExpected,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JudgmentEvalSnapshot {
    pub attention_state: String,
    pub focus_score: f32,
    pub content_idle_seconds: Option<u32>,
    pub snapshot_confidence: f32,
    pub quiet_mode: bool,
    pub natural_boundary: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JudgmentEvalCandidate {
    pub reason_type: String,
    pub detail: String,
    pub confidence: f32,
    pub importance: f32,
    pub deadline_minutes: Option<i64>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JudgmentEvalLedger {
    pub ignored_count: u32,
    pub engaged_count: u32,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JudgmentEvalExpected {
    pub decision: String,
    pub channels: Vec<String>,
}

impl JudgmentEvalScenario {
    #[allow(dead_code)]
    pub fn stage2_input(&self) -> JudgmentStage2Input {
        JudgmentStage2Input {
            attention_state: self.snapshot.attention_state.clone(),
            focus_score: self.snapshot.focus_score,
            content_idle_seconds: self.snapshot.content_idle_seconds,
            snapshot_confidence: self.snapshot.snapshot_confidence,
            quiet_mode: self.snapshot.quiet_mode,
            natural_boundary: self.snapshot.natural_boundary,
            reason_type: self.candidate.reason_type.clone(),
            candidate_confidence: self.candidate.confidence,
            candidate_importance: self.candidate.importance,
            deadline_minutes: self.candidate.deadline_minutes,
            ignored_count: self.ledger.ignored_count,
            engaged_count: self.ledger.engaged_count,
        }
    }
}

#[allow(dead_code)]
pub fn evaluate_stage2_fixture(scenario: &JudgmentEvalScenario) -> JudgmentStage2Output {
    evaluate_stage2_economics(&scenario.stage2_input())
}

pub fn evaluate_stage2_economics(input: &JudgmentStage2Input) -> JudgmentStage2Output {
    if input.snapshot_confidence < 0.30 || input.candidate_confidence < 0.35 {
        return output("drop", "silent_card", "low_confidence");
    }

    if input.quiet_mode {
        return output("hold", "silent_card", "quiet_mode");
    }

    let deadline_soon = input
        .deadline_minutes
        .map(|minutes| minutes <= 60)
        .unwrap_or(false);
    let deadline_imminent = input
        .deadline_minutes
        .map(|minutes| minutes <= 20)
        .unwrap_or(false);
    let urgent = deadline_soon || input.candidate_importance >= 0.85;
    if urgent {
        let channel = if deadline_imminent {
            "notification"
        } else {
            "bubble"
        };
        return output("speak", channel, "urgent_enough_to_interrupt");
    }

    if input.candidate_importance < 0.25 {
        return output("drop", "silent_card", "low_value_candidate");
    }

    let ignored_pattern = input.ignored_count >= 3 && input.engaged_count == 0;
    if ignored_pattern && !is_natural_boundary(input) {
        return output("hold", "silent_card", "repeated_ignore_pattern");
    }

    if is_deep_focus(input) && !is_natural_boundary(input) {
        return output("hold", "silent_card", "deep_focus");
    }

    output("speak", "bubble", "acceptable_interruption")
}

fn is_deep_focus(input: &JudgmentStage2Input) -> bool {
    input.attention_state.eq_ignore_ascii_case("focused")
        && input.focus_score >= 0.65
        && input
            .content_idle_seconds
            .map(|seconds| seconds < 60)
            .unwrap_or(true)
}

fn is_natural_boundary(input: &JudgmentStage2Input) -> bool {
    input.natural_boundary
        || input.attention_state.eq_ignore_ascii_case("idle")
        || input
            .content_idle_seconds
            .map(|seconds| seconds >= 90)
            .unwrap_or(false)
}

fn output(decision: &str, channel: &str, reason: &str) -> JudgmentStage2Output {
    JudgmentStage2Output {
        decision: decision.to_string(),
        channel: channel.to_string(),
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> JudgmentStage2Input {
        JudgmentStage2Input {
            attention_state: "focused".to_string(),
            focus_score: 0.8,
            content_idle_seconds: Some(5),
            snapshot_confidence: 0.8,
            quiet_mode: false,
            natural_boundary: false,
            reason_type: "task_return".to_string(),
            candidate_confidence: 0.8,
            candidate_importance: 0.6,
            deadline_minutes: None,
            ignored_count: 0,
            engaged_count: 0,
        }
    }

    #[test]
    fn c6_deep_focus_holds_unless_boundary() {
        let held = evaluate_stage2_economics(&input());
        assert_eq!(held.decision, "hold");

        let mut boundary = input();
        boundary.natural_boundary = true;
        let spoken = evaluate_stage2_economics(&boundary);
        assert_eq!(spoken.decision, "speak");
    }

    #[test]
    fn c6_deadline_escalates_even_in_deep_focus() {
        let mut urgent = input();
        urgent.reason_type = "deadline_pressure".to_string();
        urgent.deadline_minutes = Some(12);
        urgent.candidate_importance = 0.95;
        let result = evaluate_stage2_economics(&urgent);
        assert_eq!(result.decision, "speak");
        assert_eq!(result.channel, "notification");
    }

    #[test]
    fn c6_repeated_ignore_and_quiet_mode_suppress() {
        let mut ignored = input();
        ignored.attention_state = "returning".to_string();
        ignored.focus_score = 0.4;
        ignored.ignored_count = 3;
        let result = evaluate_stage2_economics(&ignored);
        assert_eq!(result.reason, "repeated_ignore_pattern");
        assert_eq!(result.decision, "hold");

        let mut quiet = input();
        quiet.quiet_mode = true;
        let quiet_result = evaluate_stage2_economics(&quiet);
        assert_eq!(quiet_result.decision, "hold");
        assert_eq!(quiet_result.channel, "silent_card");
    }

    #[test]
    fn c6_low_confidence_drops() {
        let mut low = input();
        low.candidate_confidence = 0.2;
        let result = evaluate_stage2_economics(&low);
        assert_eq!(result.decision, "drop");
    }
}
