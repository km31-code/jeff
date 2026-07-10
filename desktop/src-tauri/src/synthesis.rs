use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Manager, Runtime};

use crate::{
    ambient::{AmbientState, NotificationPayload, OVERLAY_WINDOW_LABEL},
    awareness_core::{
        self, snapshot_summary, ProactiveSpeechReason, SituationalSnapshot, SnapshotTrigger,
        UserProfile,
    },
    memory,
    model_router::{GenerateOptions, Tier},
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
    // stage 1: deterministic candidate generation (every tick).
    let reason = awareness_core::generate_proactive_candidate(
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
            Some("no_candidate"),
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

    if !jeff.model_router.any_key_available() {
        log_synthesis_decision(
            &jeff,
            task.id,
            Some(&reason),
            &snapshot,
            Some("missing_api_key"),
            None,
            false,
        );
        return;
    }

    // stage 2: one judgment-tier decision owns whether/when/how/with-what-words.
    let decision = decide_proactive_stage2(&jeff, task.id, &snapshot, &reason).await;

    match decision.verdict {
        Stage2Verdict::Speak if !decision.message.trim().is_empty() => {
            let delivered =
                deliver_by_channel(handle, &jeff, task.id, &reason, &decision).await;
            if delivered {
                // apex c2: record the interjection and the focus it landed in;
                // the reaction is filled in later (reply, dismissal, or ignored).
                let _ = jeff.store.record_interruption(
                    Some(task.id),
                    reason.reason_type(),
                    decision.channel.as_str(),
                    snapshot.focus_score,
                );
            }
            log_stage2(&jeff, task.id, &reason, &snapshot, &decision, None, delivered);
        }
        Stage2Verdict::Speak => {
            // model chose speak but produced no message; record as a drop.
            log_stage2(
                &jeff,
                task.id,
                &reason,
                &snapshot,
                &decision,
                Some("empty_message"),
                false,
            );
        }
        Stage2Verdict::Hold | Stage2Verdict::Drop => {
            log_stage2(&jeff, task.id, &reason, &snapshot, &decision, None, false);
        }
    }
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

// ---- apex c1: stage 2 (judgment-tier interruption decision) -----------------

const STAGE2_TIMEOUT_MS: u64 = 4000;

pub const STAGE2_SYSTEM_PROMPT: &str = "You are Jeff's interruption-economics judgment. \
A deterministic stage 1 produced one candidate reason to speak. You decide, like a considerate senior colleague, \
whether interrupting is worth it right now, through which channel, and with what words. \
Weigh what it costs the user to be interrupted (deep focus, mid-thought) against what it is worth to them to know. \
Return strict JSON only: \
{\"decision\":\"speak|hold|drop\",\"channel\":\"voice|bubble|notification|silent_card\",\"message\":\"1-2 sentences, <=40 words, specific, conversational, not a notification\",\"reason\":\"one short clause\"}. \
Use \"hold\" to wait for a natural boundary, \"drop\" to discard a candidate that is not worth it. \
message may be empty only when decision is hold or drop.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage2Verdict {
    Speak,
    Hold,
    Drop,
}

impl Stage2Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Stage2Verdict::Speak => "speak",
            Stage2Verdict::Hold => "hold",
            Stage2Verdict::Drop => "drop",
        }
    }

    fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "hold" => Stage2Verdict::Hold,
            "drop" => Stage2Verdict::Drop,
            _ => Stage2Verdict::Speak,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage2Channel {
    Voice,
    Bubble,
    Notification,
    SilentCard,
}

impl Stage2Channel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Stage2Channel::Voice => "voice",
            Stage2Channel::Bubble => "bubble",
            Stage2Channel::Notification => "notification",
            Stage2Channel::SilentCard => "silent_card",
        }
    }

    fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "voice" => Stage2Channel::Voice,
            "notification" => Stage2Channel::Notification,
            "silent_card" | "silent" | "card" => Stage2Channel::SilentCard,
            _ => Stage2Channel::Bubble,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Stage2Decision {
    pub verdict: Stage2Verdict,
    pub channel: Stage2Channel,
    pub message: String,
    pub reason: String,
}

pub fn parse_stage2_json(raw: &str) -> Option<Stage2Decision> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&raw[start..=end]).ok()?;
    Some(Stage2Decision {
        verdict: Stage2Verdict::parse(value["decision"].as_str().unwrap_or("speak")),
        channel: Stage2Channel::parse(value["channel"].as_str().unwrap_or("bubble")),
        message: value["message"].as_str().unwrap_or("").trim().to_string(),
        reason: value["reason"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string(),
    })
}

async fn decide_proactive_stage2(
    jeff: &JeffState,
    task_id: i64,
    snapshot: &SituationalSnapshot,
    reason: &ProactiveSpeechReason,
) -> Stage2Decision {
    let recall_block = build_stage2_recall(jeff, task_id, snapshot);
    let ledger = jeff
        .store
        .list_recent_interruptions(task_id, LEDGER_LOOKBACK)
        .unwrap_or_default();
    let now = unix_now();
    let ledger_summary = build_ledger_summary(&ledger, now);
    let user_prompt = build_stage2_prompt(
        reason,
        snapshot,
        recall_block.as_deref(),
        ledger_summary.as_deref().unwrap_or("no interruption history yet"),
    );

    match jeff
        .model_router
        .generate_async(
            Tier::Judgment,
            STAGE2_SYSTEM_PROMPT,
            &user_prompt,
            GenerateOptions {
                temperature: 0.0,
                max_tokens: Some(400),
                json_object: true,
                timeout_ms: Some(STAGE2_TIMEOUT_MS),
            },
        )
        .await
        .ok()
        .and_then(|raw| parse_stage2_json(&raw))
    {
        Some(decision) => decision,
        None => fallback_stage2(jeff, task_id, snapshot, reason, &ledger, now).await,
    }
}

// deterministic fallback when the stage 2 model call fails. keeps the phase 27
// wording path for the message so we never regress to silence. holds when the
// user is in deep focus, or when this reason has been repeatedly ignored at this
// focus band — unless we are at a natural boundary, which releases the hold.
async fn fallback_stage2(
    jeff: &JeffState,
    _task_id: i64,
    snapshot: &SituationalSnapshot,
    reason: &ProactiveSpeechReason,
    ledger: &[crate::store::InterruptionLedgerRow],
    now: i64,
) -> Stage2Decision {
    let repeatedly_ignored =
        reason_band_is_ignored(ledger, reason.reason_type(), snapshot.focus_score, now);
    let verdict = fallback_verdict(snapshot, reason.reason_type(), ledger, now);
    let should_hold = verdict == Stage2Verdict::Hold;
    let message = awareness_core::synthesize_proactive_message(reason, snapshot, &jeff.model_router)
        .await
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    let reason_text = if should_hold && repeatedly_ignored {
        "held: this cue has been ignored at this focus level"
    } else if should_hold {
        "held: deep focus, waiting for a natural boundary"
    } else {
        "stage2_fallback"
    };
    Stage2Decision {
        verdict,
        channel: Stage2Channel::Bubble,
        message,
        reason: reason_text.to_string(),
    }
}

// ---- apex c2: interruption ledger summary + learned hold ---------------------

// pure hold/speak decision for the deterministic fallback: hold during deep
// focus or a repeatedly-ignored cue at this focus band, unless we are at a
// natural boundary (which releases the hold).
fn fallback_verdict(
    snapshot: &SituationalSnapshot,
    reason_type: &str,
    ledger: &[crate::store::InterruptionLedgerRow],
    now: i64,
) -> Stage2Verdict {
    let at_boundary = awareness_core::is_at_natural_boundary(snapshot);
    let repeatedly_ignored = reason_band_is_ignored(ledger, reason_type, snapshot.focus_score, now);
    if (is_deep_focus(snapshot) || repeatedly_ignored) && !at_boundary {
        Stage2Verdict::Hold
    } else {
        Stage2Verdict::Speak
    }
}

const LEDGER_LOOKBACK: usize = 40;
// a still-pending interjection older than this is treated as ignored.
const LEDGER_IGNORE_SECONDS: i64 = 300;

fn focus_band(focus_score: f32) -> &'static str {
    if focus_score >= 0.6 {
        "deep-focus"
    } else if focus_score >= 0.3 {
        "engaged"
    } else {
        "break"
    }
}

// classify a ledger row's settled outcome. returns None when the outcome is not
// yet known (recently delivered, still awaiting a reaction).
fn settled_outcome(row: &crate::store::InterruptionLedgerRow, now: i64) -> Option<bool> {
    match row.reaction.as_deref() {
        Some("engaged") => Some(true),
        Some(_) => Some(false), // dismissed / explicit_negative / ignored
        None => {
            if now.saturating_sub(row.delivered_at_unix) >= LEDGER_IGNORE_SECONDS {
                Some(false) // ignored: delivered long ago, never reacted to
            } else {
                None
            }
        }
    }
}

// a compact (~60 token) engagement summary bucketed by focus band and reason.
fn build_ledger_summary(ledger: &[crate::store::InterruptionLedgerRow], now: i64) -> Option<String> {
    use std::collections::BTreeMap;
    let mut buckets: BTreeMap<(String, &str), (u32, u32)> = BTreeMap::new();
    for row in ledger {
        let Some(engaged) = settled_outcome(row, now) else {
            continue;
        };
        let entry = buckets
            .entry((row.reason_type.clone(), focus_band(row.focus_score)))
            .or_insert((0, 0));
        entry.0 += 1;
        if engaged {
            entry.1 += 1;
        }
    }
    if buckets.is_empty() {
        return None;
    }
    let lines = buckets
        .into_iter()
        .map(|((reason, band), (delivered, engaged))| {
            format!("{band} {reason}: engaged {engaged}/{delivered}")
        })
        .collect::<Vec<_>>();
    Some(lines.join("; "))
}

// true when this reason at this focus band has a clear ignored pattern: at least
// three settled outcomes and none engaged.
fn reason_band_is_ignored(
    ledger: &[crate::store::InterruptionLedgerRow],
    reason_type: &str,
    focus_score: f32,
    now: i64,
) -> bool {
    let band = focus_band(focus_score);
    let mut delivered = 0u32;
    let mut engaged = 0u32;
    for row in ledger {
        if row.reason_type != reason_type || focus_band(row.focus_score) != band {
            continue;
        }
        if let Some(is_engaged) = settled_outcome(row, now) {
            delivered += 1;
            if is_engaged {
                engaged += 1;
            }
        }
    }
    delivered >= 3 && engaged == 0
}

// ---- apex c2: reaction capture ----------------------------------------------

// record the user's reaction to a recent interjection when they send a message:
// "not now"-class replies are explicit_negative, everything else is engaged.
pub fn record_interruption_reaction_for_reply(
    store: &crate::store::TaskStore,
    task_id: i64,
    message: &str,
) {
    let reaction = if is_explicit_negative(message) {
        "explicit_negative"
    } else {
        "engaged"
    };
    let _ = store.record_interruption_reaction_within(task_id, LEDGER_IGNORE_SECONDS, reaction);
}

fn is_explicit_negative(message: &str) -> bool {
    let lower = message.trim().to_ascii_lowercase();
    [
        "not now",
        "not right now",
        "leave me",
        "go away",
        "stop interrupting",
        "not helpful",
        "quiet",
        "shush",
        "later",
        "busy",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
}

// deep focus: actively engaged (focused, content changing) — a high bar for
// interruption. a stand-in for the C2 focus-depth model.
fn is_deep_focus(snapshot: &SituationalSnapshot) -> bool {
    snapshot.attention_state == crate::awareness_core::AttentionState::Focused
        && snapshot.content_idle_seconds.map(|s| s < 30).unwrap_or(false)
}

fn build_stage2_recall(
    jeff: &JeffState,
    _task_id: i64,
    snapshot: &SituationalSnapshot,
) -> Option<String> {
    let query = memory::build_recall_query(
        snapshot.current_goal.as_deref(),
        snapshot.active_document_excerpt.as_deref(),
        None,
    );
    if query.trim().is_empty() {
        return None;
    }
    memory::build_recall_block(&jeff.store, jeff.embeddings.as_ref(), &query, 4)
}

fn build_stage2_prompt(
    reason: &ProactiveSpeechReason,
    snapshot: &SituationalSnapshot,
    recall_block: Option<&str>,
    ledger_summary: &str,
) -> String {
    format!(
        "Candidate reason: {}\nDetail: {}\n\nSituation:\n{}\n\nMemory recall:\n{}\n\nInterruption ledger:\n{}\n\nDecide now.",
        reason.reason_type(),
        reason.detail(),
        snapshot_summary(snapshot),
        recall_block.unwrap_or("<none>"),
        ledger_summary,
    )
}

async fn deliver_by_channel<R: Runtime>(
    handle: &AppHandle<R>,
    jeff: &JeffState,
    task_id: i64,
    reason: &ProactiveSpeechReason,
    decision: &Stage2Decision,
) -> bool {
    let kind = proactive_message_kind_for_reason(reason);
    let message = decision.message.as_str();

    // notification channel: a system notification, no chat bubble.
    if decision.channel == Stage2Channel::Notification {
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

    // voice (until C4), bubble, and silent_card all render as a chat bubble.
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

    // silent_card is intentionally non-notifying; bubble/voice raise a
    // notification only when the overlay is hidden.
    if decision.channel == Stage2Channel::SilentCard {
        return true;
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

fn log_stage2(
    jeff: &JeffState,
    task_id: i64,
    reason: &ProactiveSpeechReason,
    snapshot: &SituationalSnapshot,
    decision: &Stage2Decision,
    suppression: Option<&str>,
    delivered: bool,
) {
    let detail = match suppression {
        Some(suppression) => format!("{}; note={suppression}", reason.detail()),
        None => reason.detail(),
    };
    let stage2_reason = if decision.reason.is_empty() {
        None
    } else {
        Some(decision.reason.as_str())
    };
    let _ = jeff.store.log_synthesis_decision_staged(
        Some(task_id),
        reason.reason_type(),
        Some(detail.as_str()),
        snapshot.snapshot_confidence,
        snapshot.attention_state.as_str(),
        (!decision.message.is_empty()).then_some(decision.message.as_str()),
        delivered,
        Some(decision.verdict.as_str()),
        Some(decision.channel.as_str()),
        stage2_reason,
    );
}

fn proactive_message_kind_for_reason(reason: &ProactiveSpeechReason) -> &'static str {
    match reason {
        ProactiveSpeechReason::TaskReturn { .. } => "proactive_reorientation",
        ProactiveSpeechReason::DeadlinePressure { .. } => "proactive_deadline",
        ProactiveSpeechReason::BlockerDetected { .. } => "proactive_blocker",
        ProactiveSpeechReason::WorkQualityObservation { .. } => "proactive_drift",
        ProactiveSpeechReason::ComprehensionObservation { .. } => "proactive_drift",
        ProactiveSpeechReason::PendingApprovalAging { .. } => "proactive_reorientation",
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
    use crate::store::TaskStore;

    use crate::awareness_core::{AttentionState, SituationalSnapshot};
    use crate::store::InterruptionLedgerRow;

    fn ignored_row(reason: &str, focus: f32, age: i64, now: i64) -> InterruptionLedgerRow {
        InterruptionLedgerRow {
            reason_type: reason.to_string(),
            focus_score: focus,
            reaction: Some("ignored".to_string()),
            delivered_at_unix: now - age,
        }
    }

    #[test]
    fn c2_three_ignored_at_high_focus_holds_the_fourth() {
        let now = 100_000;
        // three prior task_return interjections at deep focus, all ignored.
        let ledger = vec![
            ignored_row("task_return", 0.8, 600, now),
            ignored_row("task_return", 0.85, 500, now),
            ignored_row("task_return", 0.75, 400, now),
        ];
        // a fourth comparable candidate arrives during deep focus (no boundary).
        let mut deep = SituationalSnapshot::default();
        deep.attention_state = AttentionState::Focused;
        deep.content_idle_seconds = Some(0);
        deep.focus_score = 0.8;
        assert_eq!(
            fallback_verdict(&deep, "task_return", &ledger, now),
            Stage2Verdict::Hold
        );

        // the same ignored history releases at a natural boundary (user idle).
        let mut boundary = SituationalSnapshot::default();
        boundary.attention_state = AttentionState::Idle;
        boundary.focus_score = 0.8;
        assert_eq!(
            fallback_verdict(&boundary, "task_return", &ledger, now),
            Stage2Verdict::Speak
        );
    }

    #[test]
    fn c2_reason_band_needs_three_settled_ignores() {
        let now = 100_000;
        let two = vec![
            ignored_row("task_return", 0.8, 600, now),
            ignored_row("task_return", 0.8, 500, now),
        ];
        assert!(!reason_band_is_ignored(&two, "task_return", 0.8, now));
        let mut three = two.clone();
        three.push(ignored_row("task_return", 0.8, 400, now));
        assert!(reason_band_is_ignored(&three, "task_return", 0.8, now));
        // one engaged breaks the ignored pattern.
        let mut with_engaged = three.clone();
        with_engaged[0].reaction = Some("engaged".to_string());
        assert!(!reason_band_is_ignored(&with_engaged, "task_return", 0.8, now));
        // a different focus band does not count.
        assert!(!reason_band_is_ignored(&three, "task_return", 0.2, now));
    }

    #[test]
    fn c2_pending_recent_row_is_not_yet_ignored() {
        let now = 100_000;
        let recent = vec![InterruptionLedgerRow {
            reason_type: "task_return".to_string(),
            focus_score: 0.8,
            reaction: None,
            delivered_at_unix: now - 10, // just delivered
        }];
        assert!(settled_outcome(&recent[0], now).is_none());
        assert!(build_ledger_summary(&recent, now).is_none());
    }

    #[test]
    fn c2_reaction_capture_records_engaged_and_explicit_negative() {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        let task = store.create_task("react").unwrap();

        // engaged: a normal reply within the window.
        store
            .record_interruption(Some(task.id), "task_return", "bubble", 0.8)
            .unwrap();
        record_interruption_reaction_for_reply(&store, task.id, "yes, fix that transition");
        let (delivered, engaged) = store.interruption_audit(7).unwrap();
        assert_eq!(delivered, 1);
        assert_eq!(engaged, 1);

        // explicit_negative: a "not now"-class reply.
        store
            .record_interruption(Some(task.id), "deadline_pressure", "bubble", 0.9)
            .unwrap();
        record_interruption_reaction_for_reply(&store, task.id, "not now, I'm busy");
        let (delivered, engaged) = store.interruption_audit(7).unwrap();
        assert_eq!(delivered, 2);
        assert_eq!(engaged, 1); // the negative reply is not engagement
    }

    #[test]
    fn c1_parse_stage2_json_reads_decision_channel_message() {
        let hold = parse_stage2_json(
            r#"{"decision":"hold","channel":"silent_card","message":"","reason":"deep focus"}"#,
        )
        .unwrap();
        assert_eq!(hold.verdict, Stage2Verdict::Hold);
        assert_eq!(hold.channel, Stage2Channel::SilentCard);
        assert_eq!(hold.reason, "deep focus");

        let speak = parse_stage2_json(
            "here you go {\"decision\":\"speak\",\"channel\":\"bubble\",\"message\":\"Paragraph two makes this point already.\",\"reason\":\"at a pause\"} thanks",
        )
        .unwrap();
        assert_eq!(speak.verdict, Stage2Verdict::Speak);
        assert_eq!(speak.channel, Stage2Channel::Bubble);
        assert_eq!(speak.message, "Paragraph two makes this point already.");

        let drop = parse_stage2_json(r#"{"decision":"drop","channel":"bubble"}"#).unwrap();
        assert_eq!(drop.verdict, Stage2Verdict::Drop);
    }

    #[test]
    fn c1_staged_log_records_hold_decision_with_reason() {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        let task = store.create_task("stage2").unwrap();
        store
            .log_synthesis_decision_staged(
                Some(task.id),
                "task_return",
                Some("idle_minutes=8"),
                0.7,
                "returning",
                None,
                false,
                Some("hold"),
                Some("bubble"),
                Some("deep focus, wait for a boundary"),
            )
            .unwrap();
        let conn = store.connect().unwrap();
        let (decision, channel, reason, delivered): (String, String, String, i64) = conn
            .query_row(
                "SELECT stage2_decision, stage2_channel, stage2_reason, delivered
                 FROM synthesis_log WHERE task_id = ?1",
                [task.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(decision, "hold");
        assert_eq!(channel, "bubble");
        assert_eq!(reason, "deep focus, wait for a boundary");
        assert_eq!(delivered, 0);
    }

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
