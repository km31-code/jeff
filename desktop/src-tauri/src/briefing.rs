// apex c3: briefing and debrief rituals. the day opens and closes with Jeff's
// judgment products, delivered as reply-able conversation bubbles (phase 28
// contract), never as reports.

use anyhow::Result;
use chrono::{Duration, Local, TimeZone, Timelike};
use tauri::{AppHandle, Manager, Runtime};

#[cfg(not(test))]
use crate::model_router::{GenerateOptions, Tier};
use crate::{
    ambient::AmbientState,
    model_router::ModelRouter,
    proactive::deliver_proactive_as_chat_message,
    state::{CalendarState, JeffState},
    store::TaskStore,
    workload,
};

pub const BRIEFING_MESSAGE_KIND: &str = "proactive_briefing";
pub const DEBRIEF_MESSAGE_KIND: &str = "proactive_debrief";
pub const DEBRIEF_ENABLED_KEY: &str = "debrief_enabled";

#[allow(dead_code)]
pub const BRIEFING_AWAY_SECONDS: i64 = 6 * 3600;
pub const DEBRIEF_IDLE_SECONDS: i64 = 45 * 60;
pub const DEBRIEF_EVENING_HOUR: u32 = 17;

const BRIEFING_LAST_FIRED_KEY: &str = "ritual:briefing_last_fired_date";
const DEBRIEF_LAST_FIRED_KEY: &str = "ritual:debrief_last_fired_date";
const WRAP_REQUESTED_KEY: &str = "ritual:wrap_requested_date";
const BRIEFING_READY_KEY_PREFIX: &str = "ritual:briefing_ready:";
#[allow(dead_code)]
const LAST_ENGAGEMENT_KEY_PREFIX: &str = "ritual:last_engagement:";

#[cfg_attr(test, allow(dead_code))]
pub const BRIEFING_SYSTEM_PROMPT: &str = "You are Jeff opening the user's day. \
Compose a briefing as a coworker who has already looked at the calendar, workload, and what mattered yesterday — not a report or a status dump. \
At most four sentences. Reference the specific items given. Make at most one concrete offer to help. \
No greetings-cliche, no filler, no bullet points. Start a conversation.";

#[cfg_attr(test, allow(dead_code))]
pub const DEBRIEF_SYSTEM_PROMPT: &str = "You are Jeff closing the user's day. \
Compose a short debrief as a coworker wrapping up together — what got done, what is still waiting on them, and the first thing tomorrow needs. \
At most four sentences. Reference the specific items given. No filler, no bullet points.";

#[derive(Debug, Clone, Default)]
#[cfg_attr(test, allow(dead_code))]
pub struct BriefingInputs {
    pub calendar: Option<String>,
    pub workload: String,
    pub facts: Vec<String>,
    pub pending_approvals: usize,
    // apex f2b: what the daemon finished overnight (completed jobs, standing-job
    // runs). empty for the on-demand path; populated when the briefing is prepared
    // ahead of first engagement so the morning message leads with real progress.
    pub overnight: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DebriefInputs {
    pub done_today: Vec<String>,
    pub pending_approvals: usize,
    pub tomorrow_first: Option<String>,
}

// ---- trigger predicates (pure, testable) ------------------------------------

#[allow(dead_code)]
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
        // A fresh profile has no "return" yet. Firing here produced an empty
        // ambient-timer briefing before the user ever engaged.
        None => false,
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

/// Mark an actual user engagement. Call this before persisting the new turn or
/// focus event so the measured gap is the time the user was truly away. The
/// ambient ritual tick consumes the ready marker; it never invents engagement.
#[allow(dead_code)]
pub fn note_user_engagement(store: &TaskStore, task_id: i64, now: i64) {
    let last_key = format!("{LAST_ENGAGEMENT_KEY_PREFIX}{task_id}");
    let ready_key = format!("{BRIEFING_READY_KEY_PREFIX}{task_id}");
    let last = store
        .get_app_setting(&last_key)
        .ok()
        .flatten()
        .and_then(|value| value.parse::<i64>().ok())
        .or_else(|| last_activity_unix(store, task_id));
    if should_fire_briefing(last, None, now) {
        let _ = store.set_app_setting(&ready_key, &date_of(now));
    }
    let _ = store.set_app_setting(&last_key, &now.to_string());
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
    let last_fired = jeff
        .store
        .get_app_setting(BRIEFING_LAST_FIRED_KEY)
        .ok()
        .flatten();
    let ready_key = format!("{BRIEFING_READY_KEY_PREFIX}{}", task.id);
    let ready = jeff.store.get_app_setting(&ready_key).ok().flatten();
    if ready.as_deref() != Some(date_of(now).as_str())
        || last_fired.as_deref() == Some(date_of(now).as_str())
    {
        return;
    }

    // apex f2b: retrieval first. if the daemon (or the app's own background
    // scheduler) prepared today's briefing ahead of this engagement, deliver that
    // -- it already folded in the overnight work and yesterday's consolidation, and
    // it costs no model call at the moment you sit down. only compose on demand when
    // nothing was prepared (daemon off, or a same-day cold start), preserving the
    // pre-f2b path exactly.
    let today = date_of(now);
    let prepared = jeff
        .store
        .get_prepared_briefing(&today)
        .ok()
        .flatten()
        .filter(|pb| pb.task_id == task.id && !pb.text.trim().is_empty());
    let from_prepared = prepared.is_some();
    let message = match prepared {
        Some(pb) => pb.text,
        None => {
            let inputs = gather_briefing_inputs(app, &jeff, task.id, now);
            compose_briefing(&jeff.model_router, &inputs)
        }
    };
    if message.trim().is_empty() {
        return;
    }
    if deliver_proactive_as_chat_message(&jeff.store, app, task.id, &message, BRIEFING_MESSAGE_KIND)
        .await
        .is_ok()
    {
        let _ = jeff.store.set_app_setting(BRIEFING_LAST_FIRED_KEY, &today);
        let _ = jeff.store.set_app_setting(&ready_key, "");
        if from_prepared {
            let _ = jeff.store.mark_prepared_briefing_delivered(&today);
        }
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
    let stored_idle_seconds = last_activity_unix(&jeff.store, task.id)
        .map(|last| now.saturating_sub(last))
        .unwrap_or(i64::MAX);
    let situational = jeff.awareness_core.snapshot_immediate();
    let idle_seconds = effective_debrief_idle_seconds(stored_idle_seconds, &situational);
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
    if !should_fire_debrief(
        enabled,
        hour,
        idle_seconds,
        explicit_wrap,
        last_fired.as_deref(),
        now,
    ) {
        return;
    }

    let inputs = gather_debrief_inputs(app, &jeff, task.id, now);
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
    now: i64,
) -> BriefingInputs {
    let calendar = app
        .try_state::<CalendarState>()
        .and_then(|state: tauri::State<'_, CalendarState>| state.current())
        .filter(|event| {
            event.minutes_until >= 0
                && date_of(now.saturating_add(event.minutes_until.saturating_mul(60)))
                    == date_of(now)
        })
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
    let facts = if jeff
        .store
        .get_privacy_user_profile_memory_enabled()
        .unwrap_or(false)
    {
        memory_takeaways_for_date(&jeff.store, task_id, &date_offset(now, -1))
    } else {
        Vec::new()
    };
    let pending_approvals = pending_approvals_count(&jeff.store, task_id);
    BriefingInputs {
        calendar,
        workload,
        facts,
        pending_approvals,
        // the on-demand path composes at engagement time; overnight work belongs to
        // the prepared path (f2b), so nothing to fold in here.
        overnight: Vec::new(),
    }
}

fn gather_debrief_inputs<R: Runtime>(
    app: &AppHandle<R>,
    jeff: &JeffState,
    task_id: i64,
    now: i64,
) -> DebriefInputs {
    let memory_enabled = jeff
        .store
        .get_privacy_user_profile_memory_enabled()
        .unwrap_or(false);
    let done_today = memory_enabled
        .then(|| crate::memory::list_episodes(&jeff.store, task_id, 50))
        .transpose()
        .ok()
        .flatten()
        .map(|episodes| {
            episodes
                .into_iter()
                .filter(|episode| {
                    local_date_of_sqlite(&episode.created_at).as_deref()
                        == Some(date_of(now).as_str())
                        && (episode.kind == crate::memory::KIND_DECISION
                            || episode.kind == crate::memory::KIND_PROPOSAL_OUTCOME
                            || episode.kind == crate::memory::KIND_SESSION_SUMMARY)
                })
                .map(|episode| episode.text)
                .take(3)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let pending_approvals = pending_approvals_count(&jeff.store, task_id);
    let tomorrow = date_offset(now, 1);
    let tomorrow_first = app
        .try_state::<CalendarState>()
        .and_then(|state: tauri::State<'_, CalendarState>| state.current())
        .filter(|event| {
            event.minutes_until >= 0
                && date_of(now.saturating_add(event.minutes_until.saturating_mul(60))) == tomorrow
        })
        .map(|event| format!("{} in {} minutes", event.title, event.minutes_until));
    DebriefInputs {
        done_today,
        pending_approvals,
        tomorrow_first,
    }
}

// pub(crate) so the f2b morning-prep path (morning.rs) builds the same yesterday
// takeaways the on-demand briefing uses.
pub(crate) fn memory_takeaways_for_date(store: &TaskStore, task_id: i64, date: &str) -> Vec<String> {
    crate::memory::list_episodes(store, task_id, 100)
        .map(|episodes| {
            episodes
                .into_iter()
                .filter(|episode| {
                    local_date_of_sqlite(&episode.created_at).as_deref() == Some(date)
                        && matches!(
                            episode.kind.as_str(),
                            crate::memory::KIND_DECISION
                                | crate::memory::KIND_PROPOSAL_OUTCOME
                                | crate::memory::KIND_SESSION_SUMMARY
                                | crate::memory::KIND_DEADLINE_MENTION
                        )
                })
                .map(|episode| episode.text)
                .take(3)
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn pending_approvals_count(store: &TaskStore, task_id: i64) -> usize {
    let legacy = store
        .list_pending_file_write_proposals(task_id)
        .map(|proposals| proposals.len())
        .unwrap_or(0);
    let unified = store
        .list_action_receipts(Some(task_id), 500)
        .map(|receipts| {
            receipts
                .into_iter()
                .filter(|receipt| receipt.status == "pending_approval")
                .count()
        })
        .unwrap_or(0);
    legacy.saturating_add(unified)
}

fn last_activity_unix(store: &TaskStore, task_id: i64) -> Option<i64> {
    let chat = store
        .list_recent_chat_messages(task_id, 1)
        .ok()
        .and_then(|messages| messages.first().cloned())
        .and_then(|latest| {
            crate::awareness_core::parse_sqlite_datetime_to_unix(&latest.created_at)
        });
    let focus = store
        .get_last_task_focus(task_id)
        .ok()
        .flatten()
        .and_then(|value| crate::awareness_core::parse_sqlite_datetime_to_unix(&value));
    [chat, focus].into_iter().flatten().max()
}

fn effective_debrief_idle_seconds(
    stored_idle_seconds: i64,
    snapshot: &crate::awareness_core::SituationalSnapshot,
) -> i64 {
    if snapshot.typing_active {
        return 0;
    }
    snapshot
        .content_idle_seconds
        .map(i64::from)
        .map(|content_idle| stored_idle_seconds.min(content_idle))
        .unwrap_or(stored_idle_seconds)
}

// ---- composition -------------------------------------------------------------

pub fn compose_briefing(router: &ModelRouter, inputs: &BriefingInputs) -> String {
    let message = match compose_briefing_model(router, inputs) {
        Ok(message) if ritual_output_is_valid(&message, true) => message.trim().to_string(),
        _ => deterministic_briefing(inputs),
    };
    enforce_ritual_output(&message)
}

pub fn compose_debrief(router: &ModelRouter, inputs: &DebriefInputs) -> String {
    let message = match compose_debrief_model(router, inputs) {
        Ok(message) if ritual_output_is_valid(&message, false) => message.trim().to_string(),
        _ => deterministic_debrief(inputs),
    };
    enforce_ritual_output(&message)
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

#[cfg_attr(test, allow(dead_code))]
pub fn build_briefing_prompt(inputs: &BriefingInputs) -> String {
    format!(
        "Calendar: {}\nWorkload: {}\nYesterday's takeaways: {}\nOvernight work: {}\nPending approvals: {}",
        inputs.calendar.as_deref().unwrap_or("nothing scheduled"),
        inputs.workload,
        if inputs.facts.is_empty() {
            "none".to_string()
        } else {
            inputs.facts.join("; ")
        },
        if inputs.overnight.is_empty() {
            "none".to_string()
        } else {
            inputs.overnight.join("; ")
        },
        inputs.pending_approvals,
    )
}

#[cfg_attr(test, allow(dead_code))]
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
    // overnight work leads: it is the concrete progress made while you were away.
    if let Some(overnight) = inputs.overnight.first() {
        parts.push(format!("While you were away: {overnight}."));
    }
    if let Some(fact) = inputs.facts.first() {
        parts.push(format!("From yesterday: {fact}."));
    }
    if inputs.pending_approvals > 0 {
        parts.push(format!(
            "{} approval{} still waiting on you.",
            inputs.pending_approvals,
            if inputs.pending_approvals == 1 {
                ""
            } else {
                "s"
            }
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
            if inputs.pending_approvals == 1 {
                ""
            } else {
                "s"
            }
        ));
    }
    if let Some(tomorrow) = inputs.tomorrow_first.as_deref() {
        parts.push(format!("Tomorrow starts with: {tomorrow}."));
    }
    parts.join(" ")
}

fn enforce_ritual_output(message: &str) -> String {
    let mut output = String::new();
    let mut sentence_count = 0usize;
    let mut word_count = 0usize;
    for token in message.split_whitespace() {
        if word_count >= 100 || sentence_count >= 4 {
            break;
        }
        if !output.is_empty() {
            output.push(' ');
        }
        output.push_str(token);
        word_count += 1;
        if token.ends_with(['.', '?', '!']) {
            sentence_count += 1;
        }
    }
    output.trim().to_string()
}

fn ritual_output_is_valid(message: &str, enforce_single_offer: bool) -> bool {
    let clean = message.trim();
    if clean.is_empty() || clean.split_whitespace().count() > 100 {
        return false;
    }
    if clean
        .lines()
        .any(|line| matches!(line.trim_start().chars().next(), Some('-' | '*' | '•')))
    {
        return false;
    }
    let sentences = clean.matches(['.', '?', '!']).count();
    if sentences > 4 {
        return false;
    }
    if enforce_single_offer {
        let lower = clean.to_ascii_lowercase();
        let offer_count = ["want me to", "shall i", "can i", "would you like me to"]
            .iter()
            .map(|phrase| lower.matches(phrase).count())
            .sum::<usize>();
        if offer_count > 1 {
            return false;
        }
    }
    true
}

// ---- helpers -----------------------------------------------------------------

fn is_quiet<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<AmbientState>()
        .map(|state: tauri::State<'_, AmbientState>| state.is_quiet_mode())
        .unwrap_or(false)
}

// pub(crate) so morning-prep (f2b) keys prepared briefings by the same local date
// the delivery path retrieves them by.
pub(crate) fn date_of(now: i64) -> String {
    Local
        .timestamp_opt(now, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

fn date_offset(now: i64, days: i64) -> String {
    Local
        .timestamp_opt(now, 0)
        .single()
        .map(|dt| {
            (dt.date_naive() + Duration::days(days))
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_default()
}

fn local_date_of_sqlite(value: &str) -> Option<String> {
    crate::awareness_core::parse_sqlite_datetime_to_unix(value).map(date_of)
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
        // no prior engagement is a fresh profile, not a return.
        assert!(!should_fire_briefing(None, None, now));
    }

    #[test]
    fn c3_briefing_becomes_ready_only_on_real_return_engagement() {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        let task = store.create_task("ritual").unwrap();
        let now = 20 * DAY;
        note_user_engagement(&store, task.id, now - 7 * 3600);
        let ready_key = format!("{BRIEFING_READY_KEY_PREFIX}{}", task.id);
        assert!(store.get_app_setting(&ready_key).unwrap().is_none());
        note_user_engagement(&store, task.id, now);
        assert_eq!(
            store.get_app_setting(&ready_key).unwrap().as_deref(),
            Some(date_of(now).as_str())
        );
    }

    #[test]
    fn c3_debrief_requires_opt_in() {
        let now = 10 * DAY;
        // opted out: never fires, even in the evening after long idle.
        assert!(!should_fire_debrief(
            false,
            20,
            DEBRIEF_IDLE_SECONDS + 1,
            false,
            None,
            now
        ));
        // opted in, evening, idle -> fires.
        assert!(should_fire_debrief(
            true,
            20,
            DEBRIEF_IDLE_SECONDS + 1,
            false,
            None,
            now
        ));
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
    fn c3_debrief_idle_uses_live_typing_and_content_activity() {
        let mut snapshot = crate::awareness_core::SituationalSnapshot::default();
        snapshot.typing_active = true;
        assert_eq!(
            effective_debrief_idle_seconds(DEBRIEF_IDLE_SECONDS + 1, &snapshot),
            0
        );
        snapshot.typing_active = false;
        snapshot.content_idle_seconds = Some(30);
        assert_eq!(
            effective_debrief_idle_seconds(DEBRIEF_IDLE_SECONDS + 1, &snapshot),
            30
        );
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
            overnight: Vec::new(),
        };
        let message = deterministic_briefing(&inputs);
        assert!(message.contains("Design review"));
        assert!(message.contains("advisor"));
        assert!(message.contains("approval"));
        // exactly one offer ("want me to ...").
        assert_eq!(message.to_lowercase().matches("want me to").count(), 1);
        assert!(
            message
                .split(['.', '?'])
                .filter(|s| !s.trim().is_empty())
                .count()
                <= 5
        );
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

    #[test]
    fn c3_output_contract_is_enforced_after_generation() {
        let message = (0..6)
            .map(|index| format!("Sentence {index}."))
            .collect::<Vec<_>>()
            .join(" ");
        let limited = enforce_ritual_output(&message);
        assert_eq!(limited.matches('.').count(), 4);
        assert!(limited.split_whitespace().count() <= 100);
        assert!(!ritual_output_is_valid("- Item one\n- Item two", true));
        assert!(!ritual_output_is_valid(
            "Want me to start? Can I also email them?",
            true
        ));
        assert!(ritual_output_is_valid(
            "The review is at two. Want me to prep the notes?",
            true
        ));
    }
}
