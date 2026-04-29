use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Manager, Runtime};

use crate::{
    ambient::{AmbientState, NotificationPayload, OVERLAY_WINDOW_LABEL},
    awareness_core::{
        self, ProactiveSpeechReason, SituationalSnapshot, SnapshotTrigger, UserProfile,
    },
    state::{CalendarState, ContextState, JeffState},
};

/// Runs the single Phase 27 proactive judgment. This replaces the old ambient
/// monitor's independent resume, drift, and stuck checks.
pub async fn run_synthesis_check<R: Runtime>(handle: &AppHandle<R>) {
    let quiet = handle
        .try_state::<AmbientState>()
        .map(|state: tauri::State<'_, AmbientState>| state.is_quiet_mode())
        .unwrap_or(false);

    let Some(jeff_ref) = handle.try_state::<JeffState>() else {
        return;
    };
    let jeff = jeff_ref.inner().clone();

    let Some(task) = jeff.store.get_active_task().ok().flatten() else {
        return;
    };

    let active_window = active_window_context(handle, &jeff);
    let calendar_event = calendar_context(handle, &jeff);
    let snapshot = jeff
        .awareness_core
        .update_with_context(
            SnapshotTrigger::TimeTick,
            task.id,
            &jeff,
            active_window,
            calendar_event,
        )
        .await;

    let proactive_enabled = jeff
        .store
        .get_privacy_proactive_triggers_enabled()
        .unwrap_or(false);
    if !proactive_enabled {
        log_synthesis_decision(
            &jeff,
            task.id,
            None,
            &snapshot,
            Some("privacy_proactive_triggers_disabled"),
            None,
            false,
        );
        return;
    }

    let profile = if jeff
        .store
        .get_privacy_user_profile_memory_enabled()
        .unwrap_or(false)
    {
        UserProfile::from_store(&jeff.store)
    } else {
        UserProfile::default()
    };
    let last_synthesis_at = jeff.store.get_last_synthesis_at(task.id).unwrap_or(None);
    let reason = awareness_core::should_speak_proactively(
        &snapshot,
        &profile,
        last_synthesis_at,
        unix_now(),
    );

    let Some(reason) = reason else {
        log_synthesis_decision(
            &jeff,
            task.id,
            None,
            &snapshot,
            Some("no_reason"),
            None,
            false,
        );
        return;
    };

    if quiet {
        log_synthesis_decision(
            &jeff,
            task.id,
            Some(&reason),
            &snapshot,
            Some("quiet_mode"),
            None,
            false,
        );
        return;
    }

    let Some(api_key) = crate::secrets::resolve_openai_api_key().api_key else {
        log_synthesis_decision(
            &jeff,
            task.id,
            Some(&reason),
            &snapshot,
            Some("missing_openai_api_key"),
            None,
            false,
        );
        return;
    };

    let message =
        match awareness_core::synthesize_proactive_message(&reason, &snapshot, &api_key).await {
            Ok(message) => message.trim().to_string(),
            Err(error) => {
                log_synthesis_decision(
                    &jeff,
                    task.id,
                    Some(&reason),
                    &snapshot,
                    Some(&format!("synthesis_failed={error}")),
                    None,
                    false,
                );
                return;
            }
        };

    if message.is_empty() {
        log_synthesis_decision(
            &jeff,
            task.id,
            Some(&reason),
            &snapshot,
            Some("empty_synthesis"),
            None,
            false,
        );
        return;
    }

    let delivered = deliver_synthesis_message(handle, &jeff, task.id, &reason, &message).await;
    log_synthesis_decision(
        &jeff,
        task.id,
        Some(&reason),
        &snapshot,
        None,
        Some(&message),
        delivered,
    );
}

fn active_window_context<R: Runtime>(handle: &AppHandle<R>, jeff: &JeffState) -> Option<String> {
    if !jeff
        .store
        .get_privacy_active_window_context_enabled()
        .unwrap_or(true)
    {
        return None;
    }

    handle
        .try_state::<ContextState>()
        .and_then(|state: tauri::State<'_, ContextState>| state.current())
        .map(|ctx| {
            format!(
                "The user currently has {} open with {}.",
                ctx.app_name, ctx.document_title
            )
        })
}

fn calendar_context<R: Runtime>(
    handle: &AppHandle<R>,
    jeff: &JeffState,
) -> Option<crate::models::CalendarEventDto> {
    if !jeff
        .store
        .get_privacy_calendar_context_enabled()
        .unwrap_or(false)
    {
        return None;
    }

    handle
        .try_state::<CalendarState>()
        .and_then(|calendar: tauri::State<'_, CalendarState>| calendar.current())
}

async fn deliver_synthesis_message<R: Runtime>(
    handle: &AppHandle<R>,
    jeff: &JeffState,
    task_id: i64,
    reason: &ProactiveSpeechReason,
    message: &str,
) -> bool {
    let kind = proactive_message_kind_for_reason(reason);
    if crate::proactive::deliver_proactive_as_chat_message(
        &jeff.store,
        handle,
        task_id,
        message,
        kind,
    )
    .await
    .is_err()
    {
        return false;
    }

    let overlay_visible = handle
        .get_webview_window(OVERLAY_WINDOW_LABEL)
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false);

    if !overlay_visible {
        return crate::ambient::dispatch_notification(
            handle,
            NotificationPayload {
                title: "jeff".to_string(),
                body: message.to_string(),
                context_kind: Some(kind.to_string()),
                context_id: Some(task_id),
            },
        )
        .is_ok();
    }

    true
}

fn proactive_message_kind_for_reason(reason: &ProactiveSpeechReason) -> &'static str {
    match reason {
        ProactiveSpeechReason::TaskReturn { .. } => "proactive_reorientation",
        ProactiveSpeechReason::DeadlinePressure { .. } => "proactive_deadline",
        ProactiveSpeechReason::BlockerDetected { .. } => "proactive_blocker",
        ProactiveSpeechReason::WorkQualityObservation { .. } => "proactive_drift",
    }
}

fn log_synthesis_decision(
    jeff: &JeffState,
    task_id: i64,
    reason: Option<&ProactiveSpeechReason>,
    snapshot: &SituationalSnapshot,
    suppression: Option<&str>,
    message: Option<&str>,
    delivered: bool,
) {
    let (reason_type, reason_detail) = reason_fields(reason, suppression);
    let _ = jeff.store.log_synthesis_decision(
        Some(task_id),
        &reason_type,
        reason_detail.as_deref(),
        snapshot.snapshot_confidence,
        snapshot.attention_state.as_str(),
        message,
        delivered,
    );
}

fn reason_fields(
    reason: Option<&ProactiveSpeechReason>,
    suppression: Option<&str>,
) -> (String, Option<String>) {
    match reason {
        Some(reason) => {
            let detail = match suppression {
                Some(suppression) => format!("{}; suppressed={suppression}", reason.detail()),
                None => reason.detail(),
            };
            (reason.reason_type().to_string(), Some(detail))
        }
        None => (
            "suppressed".to_string(),
            Some(suppression.unwrap_or("no_reason").to_string()),
        ),
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_fields_records_suppressed_none() {
        assert_eq!(
            reason_fields(None, Some("no_reason")),
            ("suppressed".to_string(), Some("no_reason".to_string()))
        );
    }

    #[test]
    fn reason_fields_records_quiet_mode_for_real_reason() {
        assert_eq!(
            reason_fields(
                Some(&ProactiveSpeechReason::TaskReturn { idle_minutes: 8 }),
                Some("quiet_mode")
            ),
            (
                "task_return".to_string(),
                Some("idle_minutes=8; suppressed=quiet_mode".to_string())
            )
        );
    }

    #[test]
    fn synthesis_reasons_map_to_proactive_message_kinds() {
        assert_eq!(
            proactive_message_kind_for_reason(&ProactiveSpeechReason::TaskReturn {
                idle_minutes: 6
            }),
            "proactive_reorientation"
        );
        assert_eq!(
            proactive_message_kind_for_reason(&ProactiveSpeechReason::DeadlinePressure {
                event: "standup".to_string(),
                minutes_until: 12,
            }),
            "proactive_deadline"
        );
        assert_eq!(
            proactive_message_kind_for_reason(&ProactiveSpeechReason::BlockerDetected {
                blocker: "waiting on outline".to_string(),
            }),
            "proactive_blocker"
        );
        assert_eq!(
            proactive_message_kind_for_reason(&ProactiveSpeechReason::WorkQualityObservation {
                observation: "draft is drifting from prompt".to_string(),
            }),
            "proactive_drift"
        );
    }
}
