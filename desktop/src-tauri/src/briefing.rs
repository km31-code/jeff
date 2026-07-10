// apex c3: briefing and debrief rituals. the day opens and closes with Jeff's
// judgment products, delivered as reply-able conversation bubbles (phase 28
// contract), never as reports.

use anyhow::Result;
use chrono::Timelike;
use tauri::{AppHandle, Manager, Runtime};

use crate::{
    ambient::AmbientState,
    consolidation,
    model_router::ModelRouter,
    proactive::deliver_proactive_as_chat_message,
    state::{CalendarState, JeffState},
    store::TaskStore,
    workload,
};
#[cfg(not(test))]
use crate::model_router::{GenerateOptions, Tier};

pub const BRIEFING_MESSAGE_KIND: &str = "proactive_briefing";
pub const DEBRIEF_MESSAGE_KIND: &str = "proactive_debrief";
pub const DEBRIEF_ENABLED_KEY: &str = "debrief_enabled";

pub const BRIEFING_AWAY_SECONDS: i64 = 6 * 3600;
pub const DEBRIEF_IDLE_SECONDS: i64 = 45 * 60;
pub const DEBRIEF_EVENING_HOUR: u32 = 17;

const BRIEFING_LAST_FIRED_KEY: &str = "ritual:briefing_last_fired_date";
const DEBRIEF_LAST_FIRED_KEY: &str = "ritual:debrief_last_fired_date";
const WRAP_REQUESTED_KEY: &str = "ritual:wrap_requested_date";

pub const BRIEFING_SYSTEM_PROMPT: &str = "You are Jeff opening the user's day. \
Compose a briefing as a coworker who has already looked at the calendar, workload, and what mattered yesterday — not a report or a status dump. \
At most four sentences. Reference the specific items given. Make at most one concrete offer to help. \
No greetings-cliche, no filler, no bullet points. Start a conversation.";

pub const DEBRIEF_SYSTEM_PROMPT: &str = "You are Jeff closing the user's day. \
Compose a short debrief as a coworker wrapping up together — what got done, what is still waiting on them, and the first thing tomorrow needs. \
At most four sentences. Reference the specific items given. No filler, no bullet points.";

#[derive(Debug, Clone, Default)]
pub struct BriefingInputs {
    pub calendar: Option<String>,
    pub workload: String,
    pub facts: Vec<String>,
    pub pending_approvals: usize,
}

#[derive(Debug, Clone, Default)]
pub struct DebriefInputs {
    pub done_today: Vec<String>,
    pub pending_approvals: usize,
    pub tomorrow_first: Option<String>,
}

// ---- trigger predicates (pure, testable) ------------------------------------

pub fn should_fire_briefing(
    last_activity_unix: Option<i64>,
    last_fired_date: Option<&str>,
    now: i64,
) -> bool {
    if last_fired_date == Some(date_of(now).as_str()) {
        return false;
    }
    match last_activity_unix {
        Some(last) => now.saturating_sub(last) >= BRIEFING_AWAY_SECONDS,
        None => true,
    }
}

pub fn should_fire_debrief(
    enabled: bool,
    hour: u32,
    idle_seconds: i64,
    explicit_wrap: bool,
    last_fired_date: Option<&str>,
    now: i64,
) -> bool {
    if !enabled {
        return false;
    }
    if last_fired_date == Some(date_of(now).as_str()) {
        return false;
    }
    explicit_wrap || (hour >= DEBRIEF_EVENING_HOUR && idle_seconds >= DEBRIEF_IDLE_SECONDS)
}

// a "wrapping up" cue in a user message (reflex-tagged; heuristic fallback here).
pub fn is_wrapping_up(message: &str) -> bool {
    let lower = message.trim().to_ascii_lowercase();
    [
        "wrapping up",
        "wrap up for",
        "calling it a day",
        "done for the day",
        "signing off",
        "heading out",
        "that's it for today",
        "thats it for today",
        "logging off",
        "call it a night",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
}

// record that the user signaled wrapping up, so the next debrief tick fires.
pub fn note_wrapping_up(store: &TaskStore, now: i64) {
    let _ = store.set_app_setting(WRAP_REQUESTED_KEY, &date_of(now));
}

// ---- firing ------------------------------------------------------------------

pub async fn maybe_fire_rituals<R: Runtime>(app: &AppHandle<R>) {
    maybe_fire_briefing(app).await;
    maybe_fire_debrief(app).await;
}

pub async fn maybe_fire_briefing<R: Runtime>(app: &AppHandle<R>) {
    if is_quiet(app) {
        return;
    }
    let Some(jeff) = app.try_state::<JeffState>() else {
        return;
    };
    if !jeff
        .store
        .get_privacy_proactive_triggers_enabled()
        .unwrap_or(false)
    {
        return;
    }
    let Some(task) = jeff.store.get_active_task().ok().flatten() else {
        return;
    };
    let now = now_unix();
    let last_activity = last_activity_unix(&jeff.store, task.id);
    let last_fired = jeff
        .store
        .get_app_setting(BRIEFING_LAST_FIRED_KEY)
        .ok()
        .flatten();
    if !should_fire_briefing(last_activity, last_fired.as_deref(), now) {
        return;
    }

    let inputs = gather_briefing_inputs(app, &jeff, task.id);
    let message = compose_briefing(&jeff.model_router, &inputs);
    if message.trim().is_empty() {
        return;
    }
    if deliver_proactive_as_chat_message(
        &jeff.store,
        app,
        task.id,
        &message,
        BRIEFING_MESSAGE_KIND,
    )
    .await
    .is_ok()
    {
        let _ = jeff
            .store
            .set_app_setting(BRIEFING_LAST_FIRED_KEY, &date_of(now));
    }
}

pub async fn maybe_fire_debrief<R: Runtime>(app: &AppHandle<R>) {
    if is_quiet(app) {
        return;
    }
    let Some(jeff) = app.try_state::<JeffState>() else {
        return;
    };
    let enabled = jeff
        .store
        .get_app_setting(DEBRIEF_ENABLED_KEY)
        .ok()
        .flatten()
        .map(|value| value == "true" || value == "1")
        .unwrap_or(false);
    if !enabled {
        return;
    }
    let Some(task) = jeff.store.get_active_task().ok().flatten() else {
        return;
    };
    let now = now_unix();
    let hour = chrono::Local::now().hour();
    let idle_seconds = last_activity_unix(&jeff.store, task.id)
        .map(|last| now.saturating_sub(last))
        .unwrap_or(i64::MAX);
    let last_fired = jeff
        .store
        .get_app_setting(DEBRIEF_LAST_FIRED_KEY)
        .ok()
        .flatten();
    let explicit_wrap = jeff
        .store
        .get_app_setting(WRAP_REQUESTED_KEY)
        .ok()
        .flatten()
        .map(|value| value == date_of(now))
        .unwrap_or(false);
    if !should_fire_debrief(enabled, hour, idle_seconds, explicit_wrap, last_fired.as_deref(), now)
    {
        return;
    }

    let inputs = gather_debrief_inputs(&jeff, task.id);
    let message = compose_debrief(&jeff.model_router, &inputs);
    if message.trim().is_empty() {
        return;
    }
    if deliver_proactive_as_chat_message(&jeff.store, app, task.id, &message, DEBRIEF_MESSAGE_KIND)
        .await
        .is_ok()
    {
        let _ = jeff
            .store
            .set_app_setting(DEBRIEF_LAST_FIRED_KEY, &date_of(now));
        let _ = jeff.store.set_app_setting(WRAP_REQUESTED_KEY, "");
    }
}

// ---- inputs ------------------------------------------------------------------

fn gather_briefing_inputs<R: Runtime>(
    app: &AppHandle<R>,
    jeff: &JeffState,
    task_id: i64,
) -> BriefingInputs {
    let calendar = app
        .try_state::<CalendarState>()
        .and_then(|state: tauri::State<'_, CalendarState>| state.current())
        .map(|event| format!("{} in {} minutes", event.title, event.minutes_until));
    let workload = workload::compute_workload_summary(&jeff.store)
        .map(|summary| {
            format!(
                "{} active task(s), {} stale",
                summary.active_tasks.len(),
                summary.stale_tasks.len()
            )
        })
        .unwrap_or_else(|_| "workload unavailable".to_string());
    let facts = memory_facts(&jeff.store);
    let pending_approvals = pending_approvals_count(&jeff.store, task_id);
    BriefingInputs {
        calendar,
        workload,
        facts,
        pending_approvals,
    }
}

fn gather_debrief_inputs(jeff: &JeffState, task_id: i64) -> DebriefInputs {
    let done_today = crate::memory::list_episodes(&jeff.store, task_id, 8)
        .map(|episodes| {
            episodes
                .into_iter()
                .filter(|episode| {
                    episode.kind == crate::memory::KIND_DECISION
                        || episode.kind == crate::memory::KIND_PROPOSAL_OUTCOME
                        || episode.kind == crate::memory::KIND_SESSION_SUMMARY
                })
                .map(|episode| episode.text)
                .take(3)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let pending_approvals = pending_approvals_count(&jeff.store, task_id);
    let tomorrow_first = memory_facts(&jeff.store).into_iter().next();
    DebriefInputs {
        done_today,
        pending_approvals,
        tomorrow_first,
    }
}

fn memory_facts(store: &TaskStore) -> Vec<String> {
    consolidation::list_facts(store, 3)
        .map(|facts| facts.into_iter().map(|fact| fact.text).collect())
        .unwrap_or_default()
}

fn pending_approvals_count(store: &TaskStore, task_id: i64) -> usize {
    store
        .list_pending_file_write_proposals(task_id)
        .map(|proposals| proposals.len())
        .unwrap_or(0)
}

fn last_activity_unix(store: &TaskStore, task_id: i64) -> Option<i64> {
    let messages = store.list_recent_chat_messages(task_id, 1).ok()?;
    let latest = messages.first()?;
    crate::awareness_core::parse_sqlite_datetime_to_unix(&latest.created_at)
}

// ---- composition -------------------------------------------------------------

pub fn compose_briefing(router: &ModelRouter, inputs: &BriefingInputs) -> String {
    match compose_briefing_model(router, inputs) {
        Ok(message) if !message.trim().is_empty() => message.trim().to_string(),
        _ => deterministic_briefing(inputs),
    }
}

pub fn compose_debrief(router: &ModelRouter, inputs: &DebriefInputs) -> String {
    match compose_debrief_model(router, inputs) {
        Ok(message) if !message.trim().is_empty() => message.trim().to_string(),
        _ => deterministic_debrief(inputs),
    }
}

#[cfg(test)]
fn compose_briefing_model(_router: &ModelRouter, _inputs: &BriefingInputs) -> Result<String> {
    Err(anyhow::anyhow!("test fallback"))
}

#[cfg(not(test))]
fn compose_briefing_model(router: &ModelRouter, inputs: &BriefingInputs) -> Result<String> {
    router.generate_with(
        Tier::Craft,
        BRIEFING_SYSTEM_PROMPT,
        &build_briefing_prompt(inputs),
        GenerateOptions {
            temperature: 0.3,
            max_tokens: Some(220),
            json_object: false,
            timeout_ms: Some(8000),
        },
    )
}

#[cfg(test)]
fn compose_debrief_model(_router: &ModelRouter, _inputs: &DebriefInputs) -> Result<String> {
    Err(anyhow::anyhow!("test fallback"))
}

#[cfg(not(test))]
fn compose_debrief_model(router: &ModelRouter, inputs: &DebriefInputs) -> Result<String> {
    router.generate_with(
        Tier::Craft,
        DEBRIEF_SYSTEM_PROMPT,
        &build_debrief_prompt(inputs),
        GenerateOptions {
            temperature: 0.3,
            max_tokens: Some(220),
            json_object: false,
            timeout_ms: Some(8000),
        },
    )
}

pub fn build_briefing_prompt(inputs: &BriefingInputs) -> String {
    format!(
        "Calendar: {}\nWorkload: {}\nYesterday's takeaways: {}\nPending approvals: {}",
        inputs.calendar.as_deref().unwrap_or("nothing scheduled"),
        inputs.workload,
        if inputs.facts.is_empty() {
            "none".to_string()
        } else {
            inputs.facts.join("; ")
        },
        inputs.pending_approvals,
    )
}

pub fn build_debrief_prompt(inputs: &DebriefInputs) -> String {
    format!(
        "Done today: {}\nPending approvals: {}\nTomorrow's first constraint: {}",
        if inputs.done_today.is_empty() {
            "nothing recorded".to_string()
        } else {
            inputs.done_today.join("; ")
        },
        inputs.pending_approvals,
        inputs.tomorrow_first.as_deref().unwrap_or("unknown"),
    )
}

fn deterministic_briefing(inputs: &BriefingInputs) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(calendar) = inputs.calendar.as_deref() {
        parts.push(format!("You have {calendar}."));
    }
    if let Some(fact) = inputs.facts.first() {
        parts.push(format!("From yesterday: {fact}."));
    }
    if inputs.pending_approvals > 0 {
        parts.push(format!(
            "{} approval{} still waiting on you.",
            inputs.pending_approvals,
            if inputs.pending_approvals == 1 { "" } else { "s" }
        ));
    }
    if parts.is_empty() {
        parts.push("Nothing urgent on the calendar and no loose ends from yesterday.".to_string());
    }
    parts.push("Want me to take the first thing while you settle in?".to_string());
    parts.join(" ")
}

fn deterministic_debrief(inputs: &DebriefInputs) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(done) = inputs.done_today.first() {
        parts.push(format!("Today: {done}."));
    } else {
        parts.push("Quiet day on the record.".to_string());
    }
    if inputs.pending_approvals > 0 {
        parts.push(format!(
            "{} approval{} still waiting.",
            inputs.pending_approvals,
            if inputs.pending_approvals == 1 { "" } else { "s" }
        ));
    }
    if let Some(tomorrow) = inputs.tomorrow_first.as_deref() {
        parts.push(format!("Tomorrow starts with: {tomorrow}."));
    }
    parts.join(" ")
}

// ---- helpers -----------------------------------------------------------------

fn is_quiet<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<AmbientState>()
        .map(|state: tauri::State<'_, AmbientState>| state.is_quiet_mode())
        .unwrap_or(false)
}

fn date_of(now: i64) -> String {
    chrono::DateTime::from_timestamp(now, 0)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    #[test]
    fn c3_briefing_fires_after_6h_away_and_not_twice_a_day() {
        let now = 10 * DAY;
        // last activity 7h ago, never fired today -> fire.
        assert!(should_fire_briefing(Some(now - 7 * 3600), None, now));
        // last activity 2h ago -> not away enough.
        assert!(!should_fire_briefing(Some(now - 2 * 3600), None, now));
        // already fired today -> no re-fire even after a long gap.
        assert!(!should_fire_briefing(
            Some(now - 7 * 3600),
            Some(date_of(now).as_str()),
            now
        ));
        // no prior activity (fresh) -> fire.
        assert!(should_fire_briefing(None, None, now));
    }

    #[test]
    fn c3_debrief_requires_opt_in() {
        let now = 10 * DAY;
        // opted out: never fires, even in the evening after long idle.
        assert!(!should_fire_debrief(false, 20, DEBRIEF_IDLE_SECONDS + 1, false, None, now));
        // opted in, evening, idle -> fires.
        assert!(should_fire_debrief(true, 20, DEBRIEF_IDLE_SECONDS + 1, false, None, now));
        // opted in but morning, not idle -> no.
        assert!(!should_fire_debrief(true, 9, 10, false, None, now));
        // opted in, explicit wrap -> fires regardless of hour/idle.
        assert!(should_fire_debrief(true, 9, 10, true, None, now));
        // already fired today -> no.
        assert!(!should_fire_debrief(
            true,
            20,
            DEBRIEF_IDLE_SECONDS + 1,
            true,
            Some(date_of(now).as_str()),
            now
        ));
    }

    #[test]
    fn c3_is_wrapping_up_detects_end_of_day_cues() {
        assert!(is_wrapping_up("ok I'm wrapping up for today"));
        assert!(is_wrapping_up("calling it a day"));
        assert!(!is_wrapping_up("let's wrap this paragraph tighter"));
        assert!(!is_wrapping_up("what should I do next?"));
    }

    #[test]
    fn c3_deterministic_briefing_is_an_opener_with_one_offer() {
        let inputs = BriefingInputs {
            calendar: Some("Design review in 120 minutes".to_string()),
            workload: "2 active task(s), 1 stale".to_string(),
            facts: vec!["advisor pushes back on passive voice".to_string()],
            pending_approvals: 1,
        };
        let message = deterministic_briefing(&inputs);
        assert!(message.contains("Design review"));
        assert!(message.contains("advisor"));
        assert!(message.contains("approval"));
        // exactly one offer ("want me to ...").
        assert_eq!(message.to_lowercase().matches("want me to").count(), 1);
        assert!(message.split(['.', '?']).filter(|s| !s.trim().is_empty()).count() <= 4);
    }

    #[test]
    fn c3_deterministic_debrief_summarizes_done_and_tomorrow() {
        let inputs = DebriefInputs {
            done_today: vec!["cut the intro anecdote".to_string()],
            pending_approvals: 2,
            tomorrow_first: Some("abstract due Friday".to_string()),
        };
        let message = deterministic_debrief(&inputs);
        assert!(message.contains("cut the intro"));
        assert!(message.contains("2 approvals"));
        assert!(message.contains("abstract due Friday"));
    }
}
