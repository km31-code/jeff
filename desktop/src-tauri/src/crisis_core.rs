use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CrisisClass {
    DeadlineCollision,
    MeetingImminent,
    DataLossRisk,
    AwaitedReplyLanded,
    StandingJobCritical,
}

impl CrisisClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeadlineCollision => "deadline_collision",
            Self::MeetingImminent => "meeting_imminent",
            Self::DataLossRisk => "data_loss_risk",
            Self::AwaitedReplyLanded => "awaited_reply_landed",
            Self::StandingJobCritical => "standing_job_critical",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::DeadlineCollision => "Deadline collision",
            Self::MeetingImminent => "Meeting imminent",
            Self::DataLossRisk => "Data loss risk",
            Self::AwaitedReplyLanded => "Awaited reply landed",
            Self::StandingJobCritical => "Standing job critical",
        }
    }

    pub fn all() -> [Self; 5] {
        [
            Self::DeadlineCollision,
            Self::MeetingImminent,
            Self::DataLossRisk,
            Self::AwaitedReplyLanded,
            Self::StandingJobCritical,
        ]
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "deadline_collision" => Some(Self::DeadlineCollision),
            "meeting_imminent" => Some(Self::MeetingImminent),
            "data_loss_risk" => Some(Self::DataLossRisk),
            "awaited_reply_landed" => Some(Self::AwaitedReplyLanded),
            "standing_job_critical" => Some(Self::StandingJobCritical),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CrisisCandidate {
    pub class: CrisisClass,
    pub evidence: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CrisisEvalCase {
    pub id: String,
    pub class: CrisisClass,
    pub minutes_until: Option<i64>,
    pub far_from_done: bool,
    pub acknowledged: bool,
    pub movement_toward: bool,
    pub removed_count: usize,
    pub known_file_count: usize,
    pub disk_available_bytes: Option<u64>,
    pub expected_fire: bool,
}

pub const MEETING_IMMINENT_MINUTES: i64 = 10;
pub const DEADLINE_COLLISION_MINUTES: i64 = 120;
pub const MASS_DELETION_MIN_COUNT: usize = 20;
pub const MASS_DELETION_MIN_RATIO: f32 = 0.25;
pub const CRITICAL_DISK_AVAILABLE_BYTES: u64 = 500 * 1024 * 1024;

pub fn detect_meeting_imminent(
    minutes_until: i64,
    acknowledged: bool,
    movement_toward: bool,
) -> Option<CrisisCandidate> {
    if minutes_until <= MEETING_IMMINENT_MINUTES && !acknowledged && !movement_toward {
        Some(CrisisCandidate {
            class: CrisisClass::MeetingImminent,
            evidence: format!("meeting starts in {minutes_until} minutes; no movement toward it"),
        })
    } else {
        None
    }
}

pub fn detect_deadline_collision(
    minutes_until: i64,
    far_from_done: bool,
) -> Option<CrisisCandidate> {
    if minutes_until <= DEADLINE_COLLISION_MINUTES && far_from_done {
        Some(CrisisCandidate {
            class: CrisisClass::DeadlineCollision,
            evidence: format!("deadline in {minutes_until} minutes while work is far from done"),
        })
    } else {
        None
    }
}

pub fn is_mass_deletion_signal(removed_count: usize, known_file_count: usize) -> bool {
    if removed_count < MASS_DELETION_MIN_COUNT {
        return false;
    }
    if known_file_count == 0 {
        return true;
    }
    (removed_count as f32 / known_file_count as f32) >= MASS_DELETION_MIN_RATIO
}

pub fn detect_data_loss_risk(
    removed_count: usize,
    known_file_count: usize,
    disk_available_bytes: Option<u64>,
) -> Option<CrisisCandidate> {
    if is_mass_deletion_signal(removed_count, known_file_count) {
        return Some(CrisisCandidate {
            class: CrisisClass::DataLossRisk,
            evidence: format!(
                "mass deletion signal: {removed_count} removed out of {known_file_count} known files"
            ),
        });
    }
    if disk_available_bytes
        .map(|bytes| bytes <= CRITICAL_DISK_AVAILABLE_BYTES)
        .unwrap_or(false)
    {
        return Some(CrisisCandidate {
            class: CrisisClass::DataLossRisk,
            evidence: "disk is critically low".to_string(),
        });
    }
    None
}

#[allow(dead_code)]
pub fn evaluate_crisis_case(case: &CrisisEvalCase) -> bool {
    match case.class {
        CrisisClass::DeadlineCollision => case
            .minutes_until
            .and_then(|minutes| detect_deadline_collision(minutes, case.far_from_done))
            .is_some(),
        CrisisClass::MeetingImminent => case
            .minutes_until
            .and_then(|minutes| {
                detect_meeting_imminent(minutes, case.acknowledged, case.movement_toward)
            })
            .is_some(),
        CrisisClass::DataLossRisk => detect_data_loss_risk(
            case.removed_count,
            case.known_file_count,
            case.disk_available_bytes,
        )
        .is_some(),
        CrisisClass::AwaitedReplyLanded => false,
        CrisisClass::StandingJobCritical => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c7_meeting_imminent_fires_at_ten_not_forty() {
        assert!(detect_meeting_imminent(10, false, false).is_some());
        assert!(detect_meeting_imminent(40, false, false).is_none());
    }

    #[test]
    fn c7_meeting_imminent_respects_acknowledged_and_movement() {
        assert!(detect_meeting_imminent(5, true, false).is_none());
        assert!(detect_meeting_imminent(5, false, true).is_none());
    }

    #[test]
    fn c7_deadline_collision_requires_far_from_done() {
        assert!(detect_deadline_collision(90, true).is_some());
        assert!(detect_deadline_collision(90, false).is_none());
        assert!(detect_deadline_collision(180, true).is_none());
    }

    #[test]
    fn c7_mass_deletion_signal_needs_count_and_ratio() {
        assert!(is_mass_deletion_signal(30, 100));
        assert!(!is_mass_deletion_signal(19, 20));
        assert!(!is_mass_deletion_signal(20, 200));
    }
}
