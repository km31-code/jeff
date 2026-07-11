use anyhow::{anyhow, Result};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::{
    ambient::{self, AmbientState, NotificationPayload},
    crisis_core::{self, CrisisCandidate},
    models::{CalendarEventDto, CrisisCardDto, CrisisClassControlDto},
    store::TaskStore,
};

pub use crate::crisis_core::CrisisClass;

pub const CRISIS_FIRED_EVENT: &str = "crisis://fired";
pub const CRISIS_LOG_EVENT_TYPE: &str = "crisis_fired";
pub const CRISIS_FEEDBACK_EVENT_TYPE: &str = "crisis_feedback";

pub fn class_controls(store: &TaskStore) -> Vec<CrisisClassControlDto> {
    crisis_core::CrisisClass::all()
        .iter()
        .map(|class| CrisisClassControlDto {
            class: class.as_str().to_string(),
            label: class.label().to_string(),
            enabled: class_enabled(store, *class),
        })
        .collect()
}

pub fn set_class_enabled(store: &TaskStore, class_name: &str, enabled: bool) -> Result<()> {
    let class = CrisisClass::parse(class_name)
        .ok_or_else(|| anyhow!("unknown crisis class '{class_name}'"))?;
    store.set_app_setting(
        &class_setting_key(class),
        if enabled { "true" } else { "false" },
    )
}

pub fn class_enabled(store: &TaskStore, class: CrisisClass) -> bool {
    store
        .get_app_setting_bool(&class_setting_key(class))
        .ok()
        .flatten()
        .unwrap_or(true)
}

pub fn record_feedback(
    store: &TaskStore,
    task_id: i64,
    class_name: &str,
    evidence: &str,
) -> Result<()> {
    let class = CrisisClass::parse(class_name)
        .ok_or_else(|| anyhow!("unknown crisis class '{class_name}'"))?;
    store.record_event(
        task_id,
        CRISIS_FEEDBACK_EVENT_TYPE,
        &serde_json::json!({
            "class": class.as_str(),
            "evidence": evidence,
            "feedback": "not_urgent"
        })
        .to_string(),
    )
}

pub fn maybe_fire_meeting_imminent<R: Runtime>(
    app: &AppHandle<R>,
    task_id: i64,
    event: &CalendarEventDto,
    movement_toward_event: bool,
) {
    if let Some(candidate) =
        crisis_core::detect_meeting_imminent(event.minutes_until, false, movement_toward_event)
    {
        fire_crisis(app, task_id, candidate);
    }
}

pub fn maybe_fire_deadline_collision<R: Runtime>(
    app: &AppHandle<R>,
    task_id: i64,
    minutes_until: i64,
    far_from_done: bool,
) {
    if let Some(candidate) = crisis_core::detect_deadline_collision(minutes_until, far_from_done) {
        fire_crisis(app, task_id, candidate);
    }
}

pub fn fire_data_loss_risk<R: Runtime>(
    app: &AppHandle<R>,
    task_id: i64,
    removed_count: usize,
    known_file_count: usize,
) {
    if let Some(candidate) =
        crisis_core::detect_data_loss_risk(removed_count, known_file_count, None)
    {
        fire_crisis(app, task_id, candidate);
    }
}

pub fn fire_crisis<R: Runtime>(app: &AppHandle<R>, task_id: i64, candidate: CrisisCandidate) {
    let Some(state) = app.try_state::<crate::state::JeffState>() else {
        return;
    };
    if !class_enabled(&state.store, candidate.class) {
        return;
    }

    let quiet = app
        .try_state::<AmbientState>()
        .map(|state| state.is_quiet_mode())
        .unwrap_or(false);
    let card = build_card(task_id, &candidate, quiet);
    let payload = serde_json::to_string(&card).unwrap_or_else(|_| "{}".to_string());
    let _ = state
        .store
        .record_event(task_id, CRISIS_LOG_EVENT_TYPE, &payload);
    let _ = app.emit(CRISIS_FIRED_EVENT, &card);

    if !quiet {
        let _ = ambient::dispatch_notification(
            app,
            NotificationPayload {
                title: card.title.clone(),
                body: card.message.clone(),
                context_kind: Some(CRISIS_LOG_EVENT_TYPE.to_string()),
                context_id: Some(task_id),
            },
        );
    }
}

pub fn build_card(task_id: i64, candidate: &CrisisCandidate, quiet: bool) -> CrisisCardDto {
    CrisisCardDto {
        task_id,
        class: candidate.class.as_str().to_string(),
        title: crisis_title(candidate.class).to_string(),
        message: crisis_message(candidate),
        evidence: candidate.evidence.clone(),
        delivery_channel: if quiet {
            "persistent_card".to_string()
        } else {
            "notification".to_string()
        },
        quiet_downgraded: quiet,
        voice_if_session_open: !quiet,
    }
}

fn class_setting_key(class: CrisisClass) -> String {
    format!("crisis:{}:enabled", class.as_str())
}

fn crisis_title(class: CrisisClass) -> &'static str {
    match class {
        CrisisClass::DeadlineCollision => "Deadline collision",
        CrisisClass::MeetingImminent => "Meeting imminent",
        CrisisClass::DataLossRisk => "Data loss risk",
        CrisisClass::AwaitedReplyLanded => "Awaited reply landed",
        CrisisClass::StandingJobCritical => "Standing job critical",
    }
}

fn crisis_message(candidate: &CrisisCandidate) -> String {
    match candidate.class {
        CrisisClass::DeadlineCollision => {
            format!(
                "This deadline is close and the work still looks unfinished: {}.",
                candidate.evidence
            )
        }
        CrisisClass::MeetingImminent => {
            format!(
                "You have a meeting about to start, and I do not see movement toward it: {}.",
                candidate.evidence
            )
        }
        CrisisClass::DataLossRisk => {
            format!(
                "I am seeing a possible data-loss event in this workspace: {}.",
                candidate.evidence
            )
        }
        CrisisClass::AwaitedReplyLanded => {
            format!("The watched reply landed: {}.", candidate.evidence)
        }
        CrisisClass::StandingJobCritical => {
            format!(
                "A standing guard found a critical condition: {}.",
                candidate.evidence
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, TaskStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn c7_class_toggles_default_on_and_disable() {
        let (_dir, store) = store();
        assert!(class_enabled(&store, CrisisClass::MeetingImminent));
        set_class_enabled(&store, "meeting_imminent", false).unwrap();
        assert!(!class_enabled(&store, CrisisClass::MeetingImminent));
    }

    #[test]
    fn c7_quiet_mode_downgrades_to_persistent_card() {
        let candidate = CrisisCandidate {
            class: CrisisClass::MeetingImminent,
            evidence: "meeting starts in 5 minutes".to_string(),
        };
        let card = build_card(7, &candidate, true);
        assert_eq!(card.delivery_channel, "persistent_card");
        assert!(card.quiet_downgraded);
    }
}
