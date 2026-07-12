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

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
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

pub fn connected_full_day_events(store: &TaskStore) -> Result<Vec<FullCalendarEvent>> {
    let now = Utc::now();
    let end = now + Duration::hours(36);
    let result = crate::tool_bus::invoke_first_enabled_tool(
        store,
        &["calendar.list_events", "calendar.get_events"],
        serde_json::json!({
            "start": now.to_rfc3339(),
            "end": end.to_rfc3339(),
            "include_attendees": true,
        }),
    )?;
    let payload = crate::tool_bus::tool_result_payload(&result.output)?;
    let events = payload
        .get("events")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("calendar tool result omitted events"))?;
    let mut parsed = events
        .iter()
        .take(500)
        .map(|event| {
            let mut event: FullCalendarEvent =
                serde_json::from_value(event.clone()).context("invalid calendar event")?;
            let start = DateTime::parse_from_rfc3339(&event.start)
                .context("calendar event start must be RFC 3339")?
                .with_timezone(&Utc);
            event.minutes_until = (start - now).num_minutes();
            Ok(event)
        })
        .collect::<Result<Vec<_>>>()?;
    parsed = full_day_events(&parsed);
    Ok(parsed)
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
    let action_class = crate::action_bus::ActionClass::CalendarPropose;
    let class = action_class.as_str();
    crate::trust::assert_runtime_level_allowed(&class, crate::trust::TRUST_LEVEL_L1)?;
    let payload = serde_json::json!({
        "title": title,
        "start": start,
        "end": end,
    });
    let receipt = crate::action_bus::ActionBus::dispatch_proposal(
        store,
        &crate::action_bus::ActionRequest {
            task_id,
            class: action_class,
            surface: "calendar".to_string(),
            description: format!("Propose event: {title}"),
            payload,
            reversibility: crate::action_bus::Reversibility::Guided,
        },
    )?;
    if let Err(error) = crate::tool_bus::persist_connected_action(
        store,
        receipt.id,
        task_id,
        &["calendar.create_event", "calendar.propose_event"],
        serde_json::json!({"title": title, "start": start, "end": end}),
    ) {
        let _ = store.update_action_receipt_status(
            receipt.id,
            "failed",
            Some("failed to persist exact calendar proposal"),
            None,
        );
        return Err(error);
    }
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
        assert!(
            pre_meeting_prep_offer(&far, Some("changed something"), &["Sarah".to_string()])
                .is_none()
        );
        // no offer when nothing changed.
        assert!(pre_meeting_prep_offer(&events, None, &["Sarah".to_string()]).is_none());
    }

    #[test]
    fn e4_event_proposal_round_trips_as_calendar_propose() {
        let (_dir, store, task_id) = test_store();
        let receipt = propose_event(
            &store,
            task_id,
            "Sync",
            "2026-07-12T14:00",
            "2026-07-12T14:30",
        )
        .unwrap();
        assert_eq!(receipt.class, "calendar.propose");
        assert_eq!(receipt.level, "L1");
        assert_eq!(receipt.status, "pending_approval");
        let receipts = store.list_action_receipts(Some(task_id), 10).unwrap();
        assert!(receipts.iter().any(|r| r.class == "calendar.propose"));
        let rejected = crate::tool_bus::reject_connected_action(&store, receipt.id).unwrap();
        assert_eq!(rejected.status, "rejected");
        assert!(crate::tool_bus::approve_connected_action(&store, receipt.id).is_err());
    }

    #[test]
    fn e4_connected_calendar_reads_real_full_day_tool_result() {
        let (_dir, store, _task_id) = test_store();
        let start = (Utc::now() + Duration::minutes(5)).to_rfc3339();
        let server = r#"import json,sys
for line in sys.stdin:
 m=json.loads(line)
 if m.get('method')=='initialize': result={'protocolVersion':'2025-03-26','capabilities':{},'serverInfo':{'name':'calendar-fixture','version':'1'}}
 elif m.get('method')=='tools/list': result={'tools':[{'name':'calendar.list_events','description':'events','inputSchema':{'type':'object'}}]}
 elif m.get('method')=='tools/call': result={'structuredContent':{'events':[{'id':'meeting-1','title':'Review','start':'__START__','attendees':['sarah@example.com'],'acknowledged':False}]}}
 else: continue
 print(json.dumps({'jsonrpc':'2.0','id':m['id'],'result':result}),flush=True)"#
            .replace("__START__", &start);
        let endpoint =
            serde_json::to_string(&vec!["/usr/bin/python3", "-u", "-c", server.as_str()]).unwrap();
        let connection = crate::tool_bus::add_tool_connection(
            &store,
            "calendar-fixture",
            crate::tool_bus::TRANSPORT_STDIO,
            &endpoint,
            &[],
        )
        .unwrap();
        crate::tool_bus::discover_connection_tools(&store, connection.id).unwrap();
        let events = connected_full_day_events(&store).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "Review");
        assert!((4..=5).contains(&events[0].minutes_until));
    }
}
