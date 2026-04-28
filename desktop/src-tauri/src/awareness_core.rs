use std::{
    sync::Mutex as StdMutex,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::Mutex;

use crate::{
    ambient::AmbientState,
    models::CalendarEventDto,
    state::{CalendarState, ContextState, JeffState},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SituationalSnapshot {
    pub current_goal: Option<String>,
    pub recent_progress: Option<String>,
    pub current_blockers: Vec<String>,
    pub attention_state: AttentionState,
    pub pending_work: Vec<PendingItem>,
    pub time_pressure: Option<TimePressure>,
    pub last_meaningful_turn: Option<i64>,
    pub snapshot_confidence: f32,
    pub updated_at: i64,
    pub trigger: String,
}

impl Default for SituationalSnapshot {
    fn default() -> Self {
        Self {
            current_goal: None,
            recent_progress: None,
            current_blockers: Vec::new(),
            attention_state: AttentionState::Idle,
            pending_work: Vec::new(),
            time_pressure: None,
            last_meaningful_turn: None,
            snapshot_confidence: 0.0,
            updated_at: unix_now(),
            trigger: "initial".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AttentionState {
    Focused,
    Drifting,
    Returning,
    Idle,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingItem {
    pub item_type: String,
    pub description: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TimePressure {
    pub source: String,
    pub description: String,
    pub minutes_until: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotTrigger {
    NewTurn,
    FocusEvent,
    WindowSwitch,
    SubtaskCompleted,
    CalendarEvent,
    TimeTick,
}

impl SnapshotTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NewTurn => "new_turn",
            Self::FocusEvent => "focus_event",
            Self::WindowSwitch => "window_switch",
            Self::SubtaskCompleted => "subtask_completed",
            Self::CalendarEvent => "calendar_event",
            Self::TimeTick => "time_tick",
        }
    }
}

pub struct AwarenessCore {
    snapshot: Mutex<SituationalSnapshot>,
    last_active_window: StdMutex<Option<String>>,
    last_calendar_event: StdMutex<Option<CalendarEventDto>>,
}

impl AwarenessCore {
    pub fn new() -> Self {
        Self {
            snapshot: Mutex::new(SituationalSnapshot::default()),
            last_active_window: StdMutex::new(None),
            last_calendar_event: StdMutex::new(None),
        }
    }

    pub async fn update(
        &self,
        trigger: SnapshotTrigger,
        task_id: i64,
        state: &JeffState,
        ambient: &AmbientState,
    ) -> SituationalSnapshot {
        let _quiet = ambient.is_quiet_mode();
        self.update_with_context(trigger, task_id, state, None, None)
            .await
    }

    pub async fn update_with_context(
        &self,
        trigger: SnapshotTrigger,
        task_id: i64,
        state: &JeffState,
        active_window: Option<String>,
        calendar_event: Option<CalendarEventDto>,
    ) -> SituationalSnapshot {
        if let Some(active_window) = active_window.filter(|value| !value.trim().is_empty()) {
            if let Ok(mut slot) = self.last_active_window.lock() {
                *slot = Some(active_window);
            }
        }
        if let Some(calendar_event) = calendar_event {
            if let Ok(mut slot) = self.last_calendar_event.lock() {
                *slot = Some(calendar_event);
            }
        }

        let active_window = self
            .last_active_window
            .lock()
            .ok()
            .and_then(|value| value.clone());
        let calendar_event = self
            .last_calendar_event
            .lock()
            .ok()
            .and_then(|value| value.clone());

        let snapshot = assemble_snapshot(trigger, task_id, state, active_window, calendar_event);
        let mut guard = self.snapshot.lock().await;
        *guard = snapshot.clone();
        snapshot
    }

    pub async fn snapshot(&self) -> SituationalSnapshot {
        self.snapshot.lock().await.clone()
    }

    pub fn snapshot_immediate(&self) -> SituationalSnapshot {
        self.snapshot
            .try_lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_default()
    }
}

pub fn spawn_awareness_update<R: Runtime + 'static>(
    app: &AppHandle<R>,
    trigger: SnapshotTrigger,
    task_id: i64,
) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(state_ref) = handle.try_state::<JeffState>() else {
            return;
        };
        let state = state_ref.inner().clone();
        let active_window = current_active_window_string(&handle);
        let calendar_event = handle
            .try_state::<CalendarState>()
            .and_then(|calendar: tauri::State<'_, CalendarState>| calendar.current());
        state
            .awareness_core
            .update_with_context(trigger, task_id, &state, active_window, calendar_event)
            .await;
    });
}

pub fn snapshot_summary(snapshot: &SituationalSnapshot) -> String {
    if snapshot.snapshot_confidence < 0.3 {
        return String::new();
    }

    let mut lines = Vec::new();
    lines.push(format!(
        "current situation: {}",
        attention_description(&snapshot.attention_state)
    ));
    if let Some(goal) = snapshot.current_goal.as_deref() {
        lines.push(format!("working toward: {goal}"));
    }
    if let Some(progress) = snapshot.recent_progress.as_deref() {
        lines.push(format!("recent progress: {progress}"));
    }
    if !snapshot.current_blockers.is_empty() {
        lines.push(format!(
            "blockers: {}",
            snapshot.current_blockers.join("; ")
        ));
    }
    if !snapshot.pending_work.is_empty() {
        let pending = snapshot
            .pending_work
            .iter()
            .map(|item| item.description.as_str())
            .collect::<Vec<&str>>()
            .join("; ");
        lines.push(format!("pending decisions: {pending}"));
    }
    if let Some(time_pressure) = snapshot.time_pressure.as_ref() {
        lines.push(format!("time pressure: {}", time_pressure.description));
    }

    truncate_chars(&lines.join("\n"), 600)
}

fn assemble_snapshot(
    trigger: SnapshotTrigger,
    task_id: i64,
    state: &JeffState,
    active_window: Option<String>,
    calendar_event: Option<CalendarEventDto>,
) -> SituationalSnapshot {
    let now = unix_now();
    let recent_messages = state
        .store
        .list_recent_chat_messages(task_id, 20)
        .unwrap_or_default();
    let recent_10 = recent_messages
        .iter()
        .rev()
        .take(10)
        .cloned()
        .collect::<Vec<_>>();

    let current_goal = extract_current_goal(&recent_10).or_else(|| {
        state
            .store
            .get_task_summary(task_id)
            .ok()
            .map(|summary| summary.summary_text)
    });

    let recent_progress = recent_messages
        .iter()
        .rev()
        .find(|message| {
            message.role == "assistant"
                && matches!(
                    message.message_kind.as_str(),
                    "subtask_result" | "revision_accepted"
                )
        })
        .map(|message| truncate_chars(&message.content, 80));

    let last_meaningful_turn = recent_messages
        .last()
        .and_then(|message| parse_sqlite_datetime_to_unix(&message.created_at));

    let attention_state = compute_attention_state(task_id, state, last_meaningful_turn, now);
    let pending_work = collect_pending_work(task_id, state);
    let current_blockers = collect_blockers(task_id, state, &attention_state, &pending_work, now);
    let time_pressure = calendar_time_pressure(calendar_event)
        .or_else(|| stated_deadline_pressure(&recent_messages));

    let mut snapshot_confidence = 0.0_f32;
    if state.store.get_active_task().ok().flatten().is_some() {
        snapshot_confidence += 0.20;
    }
    if current_goal.is_some() {
        snapshot_confidence += 0.20;
    }
    if last_meaningful_turn
        .map(|turn| now.saturating_sub(turn) <= 3600)
        .unwrap_or(false)
    {
        snapshot_confidence += 0.20;
    }
    if active_window
        .as_deref()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
    {
        snapshot_confidence += 0.20;
    }
    if recent_progress.is_some() || !pending_work.is_empty() {
        snapshot_confidence += 0.20;
    }

    SituationalSnapshot {
        current_goal: current_goal.map(|value| truncate_chars(value.trim(), 240)),
        recent_progress,
        current_blockers,
        attention_state,
        pending_work,
        time_pressure,
        last_meaningful_turn,
        snapshot_confidence: snapshot_confidence.min(1.0),
        updated_at: now,
        trigger: trigger.as_str().to_string(),
    }
}

fn current_active_window_string<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    app.try_state::<ContextState>()
        .and_then(|context: tauri::State<'_, ContextState>| context.current())
        .map(|ctx| {
            format!(
                "The user currently has {} open with {}.",
                ctx.app_name, ctx.document_title
            )
        })
}

fn extract_current_goal(messages: &[crate::models::ChatMessageDto]) -> Option<String> {
    for message in messages.iter().rev() {
        if message.role != "user" {
            continue;
        }
        if let Some(goal) = extract_goal_from_text(&message.content) {
            return Some(goal);
        }
    }
    None
}

pub fn extract_goal_from_text(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    for pattern in [
        "i'm working on",
        "i need to",
        "i'm trying to",
        "my goal is",
        "i want to",
    ] {
        if let Some(index) = lower.find(pattern) {
            let start = index + pattern.len();
            let goal = text[start..]
                .trim()
                .trim_start_matches([':', '-', ' '])
                .trim_end_matches(['.', '!', '?'])
                .trim();
            if !goal.is_empty() {
                return Some(truncate_chars(goal, 240));
            }
        }
    }
    None
}

fn compute_attention_state(
    task_id: i64,
    state: &JeffState,
    last_meaningful_turn: Option<i64>,
    now: i64,
) -> AttentionState {
    let last_focus_at = state
        .store
        .get_last_task_focus(task_id)
        .ok()
        .flatten()
        .and_then(|value| parse_sqlite_datetime_to_unix(&value));

    let last_drift_at = state
        .store
        .get_last_proactive_trigger(task_id, "drift")
        .ok()
        .flatten()
        .and_then(|value| parse_sqlite_datetime_to_unix(&value));

    classify_attention_state(last_focus_at, last_drift_at, last_meaningful_turn, now)
}

fn classify_attention_state(
    last_focus_at: Option<i64>,
    last_drift_at: Option<i64>,
    last_meaningful_turn: Option<i64>,
    now: i64,
) -> AttentionState {
    if last_focus_at
        .map(|last_focus| now.saturating_sub(last_focus) > 300)
        .unwrap_or(false)
    {
        return AttentionState::Returning;
    }

    if last_drift_at
        .map(|last_drift| now.saturating_sub(last_drift) <= 900)
        .unwrap_or(false)
    {
        return AttentionState::Drifting;
    }

    if last_meaningful_turn
        .map(|turn| now.saturating_sub(turn) <= 120)
        .unwrap_or(false)
    {
        return AttentionState::Focused;
    }

    AttentionState::Idle
}

fn collect_pending_work(task_id: i64, state: &JeffState) -> Vec<PendingItem> {
    let mut pending = Vec::new();

    if let Ok(proposals) = state.store.list_pending_file_write_proposals(task_id) {
        for proposal in proposals {
            pending.push(PendingItem {
                item_type: "file_write_proposal".to_string(),
                description: format!("file write proposal waiting ({})", proposal.proposed_path),
                created_at: parse_sqlite_datetime_to_unix(&proposal.proposed_at).unwrap_or(0),
            });
        }
    }

    if let Ok(subtasks) = state.store.list_subtasks(task_id) {
        for subtask in subtasks {
            if subtask.result_review_status == "unreviewed" && subtask.status == "completed" {
                pending.push(PendingItem {
                    item_type: "subtask_result".to_string(),
                    description: format!("subtask result waiting ({})", subtask.title),
                    created_at: parse_sqlite_datetime_to_unix(&subtask.updated_at).unwrap_or(0),
                });
            }
        }
    }

    pending
}

fn collect_blockers(
    task_id: i64,
    state: &JeffState,
    attention_state: &AttentionState,
    pending_work: &[PendingItem],
    now: i64,
) -> Vec<String> {
    let mut blockers = Vec::new();
    if *attention_state == AttentionState::Drifting {
        blockers.push(
            state
                .store
                .list_proactive_trigger_audit_log(task_id, 5)
                .ok()
                .and_then(|entries| {
                    entries
                        .into_iter()
                        .find(|entry| entry.trigger_type == "drift" && !entry.suppressed)
                })
                .map(|_| "current work appears to be drifting from the task goal".to_string())
                .unwrap_or_else(|| "current work may be drifting from the task goal".to_string()),
        );
    }

    for item in pending_work {
        if item.created_at > 0 && now.saturating_sub(item.created_at) > 300 {
            blockers.push(format!(
                "waiting on your decision about {}",
                item.description
            ));
        }
    }

    blockers
}

fn calendar_time_pressure(event: Option<CalendarEventDto>) -> Option<TimePressure> {
    let event = event?;
    if event.minutes_until > 120 {
        return None;
    }
    Some(TimePressure {
        source: "calendar".to_string(),
        description: format!("{} in {} minutes", event.title, event.minutes_until),
        minutes_until: Some(event.minutes_until),
    })
}

fn stated_deadline_pressure(messages: &[crate::models::ChatMessageDto]) -> Option<TimePressure> {
    for message in messages.iter().rev() {
        let lower = message.content.to_ascii_lowercase();
        for pattern in ["by midnight", "by tomorrow", "deadline is", "due at"] {
            if lower.contains(pattern) {
                return Some(TimePressure {
                    source: "stated_deadline".to_string(),
                    description: truncate_chars(message.content.trim(), 160),
                    minutes_until: None,
                });
            }
        }
    }
    None
}

fn attention_description(state: &AttentionState) -> &'static str {
    match state {
        AttentionState::Focused => "focused on the current task",
        AttentionState::Drifting => "possibly drifting from the task goal",
        AttentionState::Returning => "returning to the task",
        AttentionState::Idle => "idle",
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn parse_sqlite_datetime_to_unix(dt: &str) -> Option<i64> {
    let normalized = dt.trim().replace('T', " ");
    let date_time: Vec<&str> = normalized.splitn(2, ' ').collect();
    if date_time.len() != 2 {
        return None;
    }
    let date_parts: Vec<i64> = date_time[0]
        .split('-')
        .filter_map(|part| part.parse().ok())
        .collect();
    let time_only = date_time[1]
        .split('.')
        .next()
        .unwrap_or("00:00:00")
        .trim_end_matches('Z');
    let time_parts: Vec<i64> = time_only
        .split(':')
        .filter_map(|part| part.parse().ok())
        .collect();
    if date_parts.len() < 3 || time_parts.len() < 3 {
        return None;
    }

    let (year, month, day) = (date_parts[0], date_parts[1], date_parts[2]);
    let (hour, minute, second) = (time_parts[0], time_parts[1], time_parts[2]);
    let leap = |y: i64| -> bool { (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 };
    let mut total_days = 0_i64;
    for y in 1970..year {
        total_days += if leap(y) { 366 } else { 365 };
    }
    let days_per_month = [31_i64, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 0..(month as usize - 1) {
        total_days += days_per_month[m];
        if m == 1 && leap(year) {
            total_days += 1;
        }
    }
    total_days += day - 1;
    Some(total_days * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    input.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_confidence_zero_with_no_signals() {
        assert_eq!(SituationalSnapshot::default().snapshot_confidence, 0.0);
    }

    #[test]
    fn snapshot_summary_empty_when_low_confidence() {
        let mut snapshot = SituationalSnapshot::default();
        snapshot.snapshot_confidence = 0.2;
        assert!(snapshot_summary(&snapshot).is_empty());
    }

    #[test]
    fn snapshot_summary_under_150_tokens() {
        let snapshot = SituationalSnapshot {
            current_goal: Some("finish the introduction before the 10pm deadline".to_string()),
            recent_progress: Some("completed the background section draft".to_string()),
            current_blockers: vec!["waiting on your decision about outline.md".to_string()],
            attention_state: AttentionState::Returning,
            pending_work: vec![PendingItem {
                item_type: "file_write_proposal".to_string(),
                description: "file write proposal waiting (outline.md)".to_string(),
                created_at: 1,
            }],
            time_pressure: Some(TimePressure {
                source: "calendar".to_string(),
                description: "Design review meeting in 42 minutes".to_string(),
                minutes_until: Some(42),
            }),
            last_meaningful_turn: Some(1),
            snapshot_confidence: 1.0,
            updated_at: 2,
            trigger: "test".to_string(),
        };
        assert!(snapshot_summary(&snapshot).chars().count() <= 600);
    }

    #[test]
    fn goal_extracted_from_im_working_on_message() {
        assert_eq!(
            extract_goal_from_text("I'm working on the introduction").as_deref(),
            Some("the introduction")
        );
    }

    #[test]
    fn attention_state_returning_after_five_minutes() {
        assert_eq!(
            classify_attention_state(Some(100), None, Some(350), 461),
            AttentionState::Returning
        );
    }

    #[test]
    fn attention_state_focused_with_recent_message() {
        assert_eq!(
            classify_attention_state(None, None, Some(940), 1_000),
            AttentionState::Focused
        );
    }
}
