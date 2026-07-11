// apex e4: calendar full-day and meeting awareness. Full-day (and next-day)
// awareness replaces next-event-only; meeting-aware judgment composes a
// pre-meeting prep offer from the document delta since the last meeting with
// overlapping attendees (the 1:25pm scene). Event creation goes through the
// action bus as calendar.propose (L1). MeetingImminent (crisis_core) now has
// full-day data behind it.
//
// Live calendar (EventKit / Calendar MCP) is env-gated and stays transient in
// memory (Phase 23 contract); the full-day filtering, attendee overlap,
// pre-meeting prep composer, and calendar.propose action are deterministic and
// tested over seeded events.

#![cfg_attr(test, allow(dead_code))]

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{models::ActionReceiptDto, store::TaskStore};

pub const PRE_MEETING_PREP_WINDOW_MINUTES: i64 = 10;
pub const FULL_DAY_HORIZON_MINUTES: i64 = 36 * 60; // today + next day

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FullCalendarEvent {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub start: String,
    #[serde(default)]
    pub minutes_until: i64,
    #[serde(default)]
    pub attendees: Vec<String>,
    #[serde(default)]
    pub acknowledged: bool,
}

// full-day (and next-day) awareness: upcoming events within the horizon, sorted
// by time. Past events are dropped.
pub fn full_day_events(events: &[FullCalendarEvent]) -> Vec<FullCalendarEvent> {
    let mut upcoming = events
        .iter()
        .filter(|event| event.minutes_until >= 0 && event.minutes_until <= FULL_DAY_HORIZON_MINUTES)
        .cloned()
        .collect::<Vec<_>>();
    upcoming.sort_by_key(|event| event.minutes_until);
    upcoming
}

pub fn next_meeting(events: &[FullCalendarEvent]) -> Option<FullCalendarEvent> {
    full_day_events(events).into_iter().next()
}

// meeting-aware judgment candidate. When the next meeting is within the prep
// window and the document changed since the last meeting (with overlapping
// attendees), offer a one-paragraph summary of what changed -- the 1:25pm scene.
pub fn pre_meeting_prep_offer(
    events: &[FullCalendarEvent],
    document_delta_summary: Option<&str>,
    last_meeting_attendees: &[String],
) -> Option<String> {
    let meeting = next_meeting(events)?;
    if meeting.minutes_until > PRE_MEETING_PREP_WINDOW_MINUTES {
        return None;
    }
    let delta = document_delta_summary
        .map(str::trim)
        .filter(|summary| !summary.is_empty())?;
    let overlapping = attendee_overlap(&meeting.attendees, last_meeting_attendees);
    let with = if overlapping.is_empty() {
        String::new()
    } else {
        format!(" with {}", overlapping.join(", "))
    };
    Some(format!(
        "{} in {} minutes{}. Since the last meeting you changed: {}. Want a one-paragraph summary to open with?",
        meeting.title, meeting.minutes_until, with, delta
    ))
}

// minimal attendee-name overlap (full entity model is G3).
pub fn attendee_overlap(current: &[String], previous: &[String]) -> Vec<String> {
    current
        .iter()
        .filter(|name| {
            let lower = name.to_ascii_lowercase();
            previous
                .iter()
                .any(|other| other.to_ascii_lowercase() == lower)
        })
        .cloned()
        .collect()
}

// event creation through the action bus as calendar.propose (L1 proposal).
pub fn propose_event(
    store: &TaskStore,
    task_id: i64,
    title: &str,
    start: &str,
    end: &str,
) -> Result<ActionReceiptDto> {
    let class = crate::action_bus::ActionClass::CalendarPropose.as_str();
    crate::trust::assert_runtime_level_allowed(&class, crate::trust::TRUST_LEVEL_L1)?;
    let payload = serde_json::json!({
        "title": title,
        "start": start,
        "end": end,
    })
    .to_string();
    let receipt = store.create_action_receipt(
        task_id,
        &class,
        "calendar",
        crate::trust::TRUST_LEVEL_L1,
        &format!("Propose event: {title}"),
        &payload,
        "pending_approval",
        None,
        None,
    )?;
    crate::trust::record_receipt_outcome(store, &receipt)?;
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_store() -> (TempDir, TaskStore, i64) {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        let task = store.create_task("calendar").unwrap();
        (dir, store, task.id)
    }

    fn event(id: &str, title: &str, minutes: i64, attendees: &[&str]) -> FullCalendarEvent {
        FullCalendarEvent {
            id: id.to_string(),
            title: title.to_string(),
            start: String::new(),
            minutes_until: minutes,
            attendees: attendees.iter().map(|a| a.to_string()).collect(),
            acknowledged: false,
        }
    }

    #[test]
    fn e4_full_day_awareness_sorts_upcoming_and_drops_past() {
        let events = vec![
            event("1", "Standup", 30, &[]),
            event("2", "Past", -20, &[]),
            event("3", "Review", 5, &["Sarah"]),
            event("4", "Way out", 40 * 60, &[]),
        ];
        let upcoming = full_day_events(&events);
        assert_eq!(upcoming.len(), 2, "past and beyond-horizon events dropped");
        assert_eq!(upcoming[0].title, "Review");
        assert_eq!(upcoming[1].title, "Standup");
    }

    #[test]
    fn e4_pre_meeting_prep_reproduces_the_125pm_scene() {
        // review in 5 minutes, doc changed since last meeting, attendee overlap.
        let events = vec![event("r", "Draft review", 5, &["Sarah", "You"])];
        let offer = pre_meeting_prep_offer(
            &events,
            Some("added 900 words to chapter 3 and restructured the methods section"),
            &["Sarah".to_string()],
        )
        .expect("expected a pre-meeting prep offer");
        assert!(offer.contains("Draft review in 5 minutes"));
        assert!(offer.contains("with Sarah"));
        assert!(offer.contains("chapter 3"));
        assert!(offer.contains("summary"));

        // no offer when the meeting is far off.
        let far = vec![event("r", "Draft review", 40, &["Sarah"])];
        assert!(pre_meeting_prep_offer(&far, Some("changed something"), &["Sarah".to_string()]).is_none());
        // no offer when nothing changed.
        assert!(pre_meeting_prep_offer(&events, None, &["Sarah".to_string()]).is_none());
    }

    #[test]
    fn e4_event_proposal_round_trips_as_calendar_propose() {
        let (_dir, store, task_id) = test_store();
        let receipt = propose_event(&store, task_id, "Sync", "2026-07-12T14:00", "2026-07-12T14:30")
            .unwrap();
        assert_eq!(receipt.class, "calendar.propose");
        assert_eq!(receipt.level, "L1");
        assert_eq!(receipt.status, "pending_approval");
        let receipts = store.list_action_receipts(Some(task_id), 10).unwrap();
        assert!(receipts.iter().any(|r| r.class == "calendar.propose"));
    }
}
