use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
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
pub const CRISIS_VOICE_CUE_EVENT: &str = "crisis://voice-cue";
const CRISIS_DEDUPE_SECONDS: i64 = 15 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CrisisDeliveryState {
    fingerprint: String,
    last_fired_at_unix: i64,
    acknowledged: bool,
}

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
    let _delivery_guard = crisis_delivery_lock()
        .lock()
        .map_err(|_| anyhow!("crisis delivery state lock poisoned"))?;
    store.record_event(
        task_id,
        CRISIS_FEEDBACK_EVENT_TYPE,
        &serde_json::json!({
            "class": class.as_str(),
            "evidence": evidence,
            "feedback": "not_urgent"
        })
        .to_string(),
    )?;
    let fingerprint = crisis_fingerprint(class, evidence);
    let key = crisis_delivery_key(task_id, class);
    let last_fired_at_unix = load_delivery_state(store, task_id, class)
        .filter(|state| state.fingerprint == fingerprint)
        .map(|state| state.last_fired_at_unix)
        .unwrap_or_else(unix_now);
    save_delivery_state(
        store,
        &key,
        &CrisisDeliveryState {
            fingerprint,
            last_fired_at_unix,
            acknowledged: true,
        },
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
        let mut candidate = candidate;
        candidate.evidence = format!("{}; {}", event.title.trim(), candidate.evidence);
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

#[allow(dead_code)]
pub fn maybe_fire_deadline_collision_estimated<R: Runtime>(
    app: &AppHandle<R>,
    task_id: i64,
    deadline_identity: &str,
    minutes_until: i64,
    estimated_remaining_minutes: Option<i64>,
) {
    if let Some(mut candidate) = crisis_core::detect_deadline_collision_from_estimate(
        minutes_until,
        estimated_remaining_minutes,
    ) {
        let identity = deadline_identity.trim();
        if !identity.is_empty() {
            candidate.evidence = format!("{identity}; {}", candidate.evidence);
        }
        fire_crisis(app, task_id, candidate);
    }
}

pub fn fire_data_loss_risk<R: Runtime>(
    app: &AppHandle<R>,
    task_id: i64,
    removed_count: usize,
    known_file_count: usize,
) {
    fire_data_loss_risk_with_disk(app, task_id, removed_count, known_file_count, None);
}

pub fn fire_data_loss_risk_with_disk<R: Runtime>(
    app: &AppHandle<R>,
    task_id: i64,
    removed_count: usize,
    known_file_count: usize,
    disk_available_bytes: Option<u64>,
) {
    if let Some(candidate) =
        crisis_core::detect_data_loss_risk(removed_count, known_file_count, disk_available_bytes)
    {
        fire_crisis(app, task_id, candidate);
    }
}

#[allow(dead_code)]
pub fn maybe_fire_awaited_reply_landed<R: Runtime>(
    app: &AppHandle<R>,
    task_id: i64,
    evidence: &str,
    was_watched: bool,
) {
    if let Some(candidate) = crisis_core::detect_awaited_reply_landed(evidence, was_watched) {
        fire_crisis(app, task_id, candidate);
    }
}

#[allow(dead_code)]
pub fn maybe_fire_standing_job_critical<R: Runtime>(
    app: &AppHandle<R>,
    task_id: i64,
    evidence: &str,
    guard_tripped: bool,
) {
    if let Some(candidate) = crisis_core::detect_standing_job_critical(evidence, guard_tripped) {
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
    let Ok(_delivery_guard) = crisis_delivery_lock().lock() else {
        return;
    };
    let now = unix_now();
    let fingerprint = crisis_fingerprint(candidate.class, &candidate.evidence);
    if load_delivery_state(&state.store, task_id, candidate.class)
        .filter(|delivery| delivery.fingerprint == fingerprint)
        .map(|delivery| {
            delivery.acknowledged
                || now.saturating_sub(delivery.last_fired_at_unix) < CRISIS_DEDUPE_SECONDS
        })
        .unwrap_or(false)
    {
        return;
    }

    let quiet = app
        .try_state::<AmbientState>()
        .map(|state| state.is_quiet_mode())
        .unwrap_or(false);
    let card = build_card(task_id, &candidate, quiet);
    let payload = serde_json::to_string(&card).unwrap_or_else(|_| "{}".to_string());
    if state
        .store
        .record_event(task_id, CRISIS_LOG_EVENT_TYPE, &payload)
        .is_err()
    {
        return;
    }
    if save_delivery_state(
        &state.store,
        &crisis_delivery_key(task_id, candidate.class),
        &CrisisDeliveryState {
            fingerprint,
            last_fired_at_unix: now,
            acknowledged: false,
        },
    )
    .is_err()
    {
        return;
    }
    let _ = app.emit(CRISIS_FIRED_EVENT, &card);

    if !quiet {
        let _ = app.emit(CRISIS_VOICE_CUE_EVENT, &card);
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

fn crisis_delivery_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn crisis_delivery_key(task_id: i64, class: CrisisClass) -> String {
    format!("crisis:delivery:{task_id}:{}", class.as_str())
}

fn load_delivery_state(
    store: &TaskStore,
    task_id: i64,
    class: CrisisClass,
) -> Option<CrisisDeliveryState> {
    store
        .get_app_setting(&crisis_delivery_key(task_id, class))
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

fn save_delivery_state(store: &TaskStore, key: &str, state: &CrisisDeliveryState) -> Result<()> {
    let raw = serde_json::to_string(state)?;
    store.set_app_setting(key, &raw)
}

fn crisis_fingerprint(class: CrisisClass, evidence: &str) -> String {
    let mut canonical = String::new();
    let mut in_digits = false;
    for character in evidence.trim().to_ascii_lowercase().chars() {
        if character.is_ascii_digit() {
            if !in_digits {
                canonical.push('#');
            }
            in_digits = true;
        } else {
            in_digits = false;
            if character.is_whitespace() {
                if !canonical.ends_with(' ') {
                    canonical.push(' ');
                }
            } else {
                canonical.push(character);
            }
        }
    }
    let mut hasher = DefaultHasher::new();
    class.hash(&mut hasher);
    canonical.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
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

    #[test]
    fn c7_fingerprint_dedupes_countdown_but_not_distinct_meetings() {
        let first = crisis_fingerprint(
            CrisisClass::MeetingImminent,
            "Design review; meeting starts in 5 minutes",
        );
        let next_tick = crisis_fingerprint(
            CrisisClass::MeetingImminent,
            "Design review; meeting starts in 4 minutes",
        );
        let different = crisis_fingerprint(
            CrisisClass::MeetingImminent,
            "Investor call; meeting starts in 4 minutes",
        );
        assert_eq!(first, next_tick);
        assert_ne!(first, different);
    }

    #[test]
    fn c7_not_urgent_feedback_acknowledges_same_fingerprint() {
        let (_dir, store) = store();
        let task = store.create_task("crisis").unwrap();
        let evidence = "Design review; meeting starts in 5 minutes";
        record_feedback(
            &store,
            task.id,
            CrisisClass::MeetingImminent.as_str(),
            evidence,
        )
        .unwrap();
        let state = load_delivery_state(&store, task.id, CrisisClass::MeetingImminent).unwrap();
        assert!(state.acknowledged);
        assert_eq!(
            state.fingerprint,
            crisis_fingerprint(CrisisClass::MeetingImminent, evidence)
        );
    }
}
