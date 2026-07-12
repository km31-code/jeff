use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};

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
        clear_held_candidate(&jeff.store, task.id);
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
    let now = unix_now();
    let mut held = load_held_candidate(&jeff.store, task.id, now);
    if held
        .as_ref()
        .map(|candidate| !held_candidate_is_relevant(candidate, &snapshot, now))
        .unwrap_or(false)
    {
        clear_held_candidate(&jeff.store, task.id);
        held = None;
    }
    if held.is_some() && !awareness_core::is_at_natural_boundary(&snapshot) {
        let reason = &held.as_ref().expect("checked held candidate").reason;
        log_synthesis_decision(
            &jeff,
            task.id,
            Some(reason),
            &snapshot,
            Some("held_until_natural_boundary"),
            None,
            false,
        );
        return;
    }

    let last_synthesis_at = jeff.store.get_last_synthesis_at(task.id).unwrap_or(None);
    // A durable held candidate owns the next natural boundary. Only when there
    // is no hold do we generate a fresh stage-1 candidate.
    let reason = held.as_ref().map(|value| value.reason.clone()).or_else(|| {
        awareness_core::generate_proactive_candidate(&snapshot, &profile, last_synthesis_at, now)
    });

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
            let delivered = deliver_by_channel(handle, &jeff, task.id, &reason, &decision).await;
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
            log_stage2(
                &jeff, task.id, &reason, &snapshot, &decision, None, delivered,
            );
            clear_held_candidate(&jeff.store, task.id);
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
            clear_held_candidate(&jeff.store, task.id);
        }
        Stage2Verdict::Hold | Stage2Verdict::Drop => {
            if decision.verdict == Stage2Verdict::Hold {
                persist_held_candidate(&jeff.store, task.id, &reason, held.as_ref(), now);
            } else {
                clear_held_candidate(&jeff.store, task.id);
            }
            log_stage2(&jeff, task.id, &reason, &snapshot, &decision, None, false);
        }
    }
}

const HELD_CANDIDATE_TTL_SECONDS: i64 = 4 * 3600;
const HELD_CANDIDATE_SET_EVENT: &str = "synthesis_held_candidate_set";
const HELD_CANDIDATE_CLEAR_EVENT: &str = "synthesis_held_candidate_clear";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct HeldCandidate {
    reason: ProactiveSpeechReason,
    held_at_unix: i64,
    expires_at_unix: i64,
}

fn load_held_candidate(
    store: &crate::store::TaskStore,
    task_id: i64,
    now: i64,
) -> Option<HeldCandidate> {
    let (event_type, raw) = latest_held_candidate_event(store, task_id)?;
    if event_type == HELD_CANDIDATE_CLEAR_EVENT {
        return None;
    }
    let held: HeldCandidate = match serde_json::from_str(&raw) {
        Ok(held) => held,
        Err(_) => {
            clear_held_candidate(store, task_id);
            return None;
        }
    };
    if held.expires_at_unix <= now {
        clear_held_candidate(store, task_id);
        return None;
    }
    Some(held)
}

fn persist_held_candidate(
    store: &crate::store::TaskStore,
    task_id: i64,
    reason: &ProactiveSpeechReason,
    existing: Option<&HeldCandidate>,
    now: i64,
) {
    let held_at_unix = existing.map(|held| held.held_at_unix).unwrap_or(now);
    let held = HeldCandidate {
        reason: reason.clone(),
        held_at_unix,
        expires_at_unix: held_at_unix.saturating_add(HELD_CANDIDATE_TTL_SECONDS),
    };
    if let Ok(raw) = serde_json::to_string(&held) {
        let _ = store.record_event(task_id, HELD_CANDIDATE_SET_EVENT, &raw);
    }
}

fn clear_held_candidate(store: &crate::store::TaskStore, task_id: i64) {
    if latest_held_candidate_event(store, task_id)
        .map(|(event_type, _)| event_type == HELD_CANDIDATE_SET_EVENT)
        .unwrap_or(false)
    {
        let _ = store.record_event(task_id, HELD_CANDIDATE_CLEAR_EVENT, "{}");
    }
}

fn latest_held_candidate_event(
    store: &crate::store::TaskStore,
    task_id: i64,
) -> Option<(String, String)> {
    let conn = store.connect().ok()?;
    conn.query_row(
        "SELECT event_type, payload_json FROM event_log
         WHERE task_id = ?1
           AND event_type IN ('synthesis_held_candidate_set', 'synthesis_held_candidate_clear')
         ORDER BY id DESC LIMIT 1",
        [task_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .ok()
}

fn held_candidate_is_relevant(
    held: &HeldCandidate,
    snapshot: &SituationalSnapshot,
    now: i64,
) -> bool {
    match &held.reason {
        ProactiveSpeechReason::DeadlinePressure { event, .. } => snapshot
            .time_pressure
            .as_ref()
            .and_then(|pressure| pressure.minutes_until.map(|minutes| (pressure, minutes)))
            .map(|(pressure, minutes)| {
                minutes >= 0
                    && (pressure.description.contains(event)
                        || event.contains(&pressure.description))
            })
            .unwrap_or(false),
        ProactiveSpeechReason::BlockerDetected { blocker } => snapshot
            .current_blockers
            .iter()
            .any(|current| current == blocker),
        ProactiveSpeechReason::PendingApprovalAging { .. } => snapshot
            .pending_work
            .iter()
            .map(|item| item.created_at)
            .filter(|created_at| *created_at > 0)
            .min()
            .map(|oldest| now.saturating_sub(oldest) >= 20 * 60)
            .unwrap_or(false),
        ProactiveSpeechReason::TaskReturn { .. } => {
            snapshot.attention_state == awareness_core::AttentionState::Returning
        }
        ProactiveSpeechReason::WorkQualityObservation { .. } => {
            snapshot.document_churn_score >= 2
                || snapshot
                    .work_understanding
                    .as_ref()
                    .and_then(|understanding| understanding.stuck_signal.as_ref())
                    .is_some()
        }
        ProactiveSpeechReason::ComprehensionObservation { .. } => snapshot
            .work_understanding
            .as_ref()
            .and_then(|understanding| understanding.candidate_observation.as_ref())
            .is_some(),
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
pub const STAGE2_VOICE_DELIVERY_EVENT: &str = "synthesis://voice-delivery";
pub const STAGE2_SILENT_CARD_EVENT: &str = "synthesis://silent-card";
const STAGE2_SILENT_CARD_LOG_EVENT: &str = "proactive_silent_card";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Stage2DeliveryPayload {
    pub task_id: i64,
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(dead_code)]
pub struct Stage2SilentCardRecord {
    pub event_id: i64,
    pub created_at: String,
    pub card: Stage2DeliveryPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[allow(dead_code)]
pub struct Stage2AuditRecord {
    pub id: i64,
    pub reason_type: String,
    pub reason_detail: Option<String>,
    pub snapshot_confidence: f32,
    pub snapshot_attention_state: String,
    pub message: Option<String>,
    pub delivered: bool,
    pub created_at: String,
    pub decision: Option<String>,
    pub channel: Option<String>,
    pub decision_reason: Option<String>,
}

#[allow(dead_code)]
pub fn list_stage2_audit(
    store: &crate::store::TaskStore,
    task_id: i64,
    limit: usize,
) -> Vec<Stage2AuditRecord> {
    let Ok(conn) = store.connect() else {
        return Vec::new();
    };
    let Ok(mut statement) = conn.prepare(
        "SELECT id, reason_type, reason_detail, snapshot_confidence,
                snapshot_attention_state, message, delivered, created_at,
                stage2_decision, stage2_channel, stage2_reason
         FROM synthesis_log WHERE task_id = ?1
         ORDER BY id DESC LIMIT ?2",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map([task_id, limit.min(500) as i64], |row| {
        Ok(Stage2AuditRecord {
            id: row.get(0)?,
            reason_type: row.get(1)?,
            reason_detail: row.get(2)?,
            snapshot_confidence: row.get::<_, f64>(3)? as f32,
            snapshot_attention_state: row.get(4)?,
            message: row.get(5)?,
            delivered: row.get::<_, i64>(6)? != 0,
            created_at: row.get(7)?,
            decision: row.get(8)?,
            channel: row.get(9)?,
            decision_reason: row.get(10)?,
        })
    }) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}

#[allow(dead_code)]
pub fn list_recent_silent_cards(
    store: &crate::store::TaskStore,
    task_id: i64,
    limit: usize,
) -> Vec<Stage2SilentCardRecord> {
    let Ok(conn) = store.connect() else {
        return Vec::new();
    };
    let Ok(mut statement) = conn.prepare(
        "SELECT id, payload_json, created_at FROM event_log
         WHERE task_id = ?1 AND event_type = 'proactive_silent_card'
         ORDER BY id DESC LIMIT ?2",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map([task_id, limit.min(100) as i64], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    }) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok)
        .filter_map(|(event_id, raw, created_at)| {
            serde_json::from_str(&raw)
                .ok()
                .map(|card| Stage2SilentCardRecord {
                    event_id,
                    created_at,
                    card,
                })
        })
        .collect()
}

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

    fn parse_strict(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "speak" => Some(Stage2Verdict::Speak),
            "hold" => Some(Stage2Verdict::Hold),
            "drop" => Some(Stage2Verdict::Drop),
            _ => None,
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

    fn parse_strict(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "voice" => Some(Stage2Channel::Voice),
            "bubble" => Some(Stage2Channel::Bubble),
            "notification" => Some(Stage2Channel::Notification),
            "silent_card" => Some(Stage2Channel::SilentCard),
            _ => None,
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
    let verdict = Stage2Verdict::parse_strict(value.get("decision")?.as_str()?)?;
    let channel = Stage2Channel::parse_strict(value.get("channel")?.as_str()?)?;
    let message = value
        .get("message")
        .and_then(|message| message.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if verdict == Stage2Verdict::Speak && message.is_empty() {
        return None;
    }
    if message.split_whitespace().count() > 40 {
        return None;
    }
    let reason = value
        .get("reason")
        .and_then(|reason| reason.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    Some(Stage2Decision {
        verdict,
        channel,
        message,
        reason,
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
    if recent_pending_same_reason(&ledger, reason.reason_type(), now) {
        return Stage2Decision {
            verdict: Stage2Verdict::Hold,
            channel: Stage2Channel::SilentCard,
            message: String::new(),
            reason: "equivalent interruption is still awaiting a reaction".to_string(),
        };
    }
    let ledger_summary = build_ledger_summary(&ledger, now);
    let last_delivery_age = jeff
        .store
        .get_last_synthesis_at(task_id)
        .ok()
        .flatten()
        .map(|last| now.saturating_sub(last));
    let user_prompt = build_stage2_prompt(
        reason,
        snapshot,
        recall_block.as_deref(),
        ledger_summary
            .as_deref()
            .unwrap_or("no interruption history yet"),
        last_delivery_age,
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
    {
        Ok(raw) => parse_stage2_json(&raw).unwrap_or_else(invalid_stage2_hold),
        Err(_) => fallback_stage2(jeff, task_id, snapshot, reason, &ledger, now).await,
    }
}

fn invalid_stage2_hold() -> Stage2Decision {
    Stage2Decision {
        verdict: Stage2Verdict::Hold,
        channel: Stage2Channel::SilentCard,
        message: String::new(),
        reason: "invalid stage2 response; failed closed".to_string(),
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
    let economics = fallback_stage2_economics(snapshot, reason, ledger, now);
    let repeatedly_ignored =
        reason_band_is_ignored(ledger, reason.reason_type(), snapshot.focus_score, now);
    let mut verdict = Stage2Verdict::parse(&economics.decision);
    let message = if verdict == Stage2Verdict::Speak {
        awareness_core::synthesize_proactive_message(reason, snapshot, &jeff.model_router)
            .await
            .map(|value| limit_stage2_words(value.trim(), 40))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let wording_unavailable = verdict == Stage2Verdict::Speak && message.is_empty();
    if wording_unavailable {
        verdict = Stage2Verdict::Hold;
    }
    let should_hold = verdict == Stage2Verdict::Hold;
    let reason_text = if should_hold && repeatedly_ignored {
        "held: this cue has been ignored at this focus level"
    } else if wording_unavailable {
        "held: safe wording was unavailable"
    } else if should_hold {
        "held: deep focus, waiting for a natural boundary"
    } else {
        "stage2_fallback"
    };
    Stage2Decision {
        verdict,
        channel: if wording_unavailable {
            Stage2Channel::SilentCard
        } else {
            Stage2Channel::parse(&economics.channel)
        },
        message,
        reason: reason_text.to_string(),
    }
}

// ---- apex c2: interruption ledger summary + learned hold ---------------------

// pure hold/speak decision for the deterministic fallback: hold during deep
// focus or a repeatedly-ignored cue at this focus band, unless we are at a
// natural boundary (which releases the hold).
#[cfg(test)]
fn fallback_verdict(
    snapshot: &SituationalSnapshot,
    reason_type: &str,
    ledger: &[crate::store::InterruptionLedgerRow],
    now: i64,
) -> Stage2Verdict {
    let (ignored_count, engaged_count) =
        reason_band_stats(ledger, reason_type, snapshot.focus_score, now);
    let economics = crate::judgment_eval_core::evaluate_stage2_economics(
        &crate::judgment_eval_core::JudgmentStage2Input {
            attention_state: snapshot.attention_state.as_str().to_string(),
            focus_score: snapshot.focus_score,
            content_idle_seconds: snapshot.content_idle_seconds,
            snapshot_confidence: 1.0,
            quiet_mode: false,
            natural_boundary: awareness_core::is_at_natural_boundary(snapshot),
            reason_type: reason_type.to_string(),
            candidate_confidence: 1.0,
            candidate_importance: 0.6,
            deadline_minutes: None,
            ignored_count,
            engaged_count,
        },
    );
    Stage2Verdict::parse(&economics.decision)
}

fn fallback_stage2_economics(
    snapshot: &SituationalSnapshot,
    reason: &ProactiveSpeechReason,
    ledger: &[crate::store::InterruptionLedgerRow],
    now: i64,
) -> crate::judgment_eval_core::JudgmentStage2Output {
    if recent_pending_same_reason(ledger, reason.reason_type(), now)
        && !awareness_core::is_at_natural_boundary(snapshot)
    {
        return crate::judgment_eval_core::JudgmentStage2Output {
            decision: "hold".to_string(),
            channel: "silent_card".to_string(),
            reason: "equivalent_interruption_still_awaiting_reaction".to_string(),
        };
    }
    let (ignored_count, engaged_count) =
        reason_band_stats(ledger, reason.reason_type(), snapshot.focus_score, now);
    crate::judgment_eval_core::evaluate_stage2_economics(
        &crate::judgment_eval_core::JudgmentStage2Input {
            attention_state: snapshot.attention_state.as_str().to_string(),
            focus_score: snapshot.focus_score,
            content_idle_seconds: snapshot.content_idle_seconds,
            snapshot_confidence: snapshot.snapshot_confidence.max(0.3),
            quiet_mode: false,
            natural_boundary: awareness_core::is_at_natural_boundary(snapshot),
            reason_type: reason.reason_type().to_string(),
            candidate_confidence: 1.0,
            candidate_importance: reason_importance(reason),
            deadline_minutes: reason_deadline_minutes(reason),
            ignored_count,
            engaged_count,
        },
    )
}

fn reason_deadline_minutes(reason: &ProactiveSpeechReason) -> Option<i64> {
    match reason {
        ProactiveSpeechReason::DeadlinePressure { minutes_until, .. } => Some(*minutes_until),
        _ => None,
    }
}

fn reason_importance(reason: &ProactiveSpeechReason) -> f32 {
    match reason {
        ProactiveSpeechReason::DeadlinePressure { .. } => 0.9,
        ProactiveSpeechReason::BlockerDetected { .. } => 0.7,
        ProactiveSpeechReason::PendingApprovalAging { .. } => 0.6,
        ProactiveSpeechReason::ComprehensionObservation { .. } => 0.55,
        ProactiveSpeechReason::WorkQualityObservation { .. } => 0.5,
        ProactiveSpeechReason::TaskReturn { .. } => 0.45,
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
fn build_ledger_summary(
    ledger: &[crate::store::InterruptionLedgerRow],
    now: i64,
) -> Option<String> {
    use std::collections::BTreeMap;
    let mut buckets: BTreeMap<(String, &str), (u32, u32, u32)> = BTreeMap::new();
    for row in ledger {
        let entry = buckets
            .entry((row.reason_type.clone(), focus_band(row.focus_score)))
            .or_insert((0, 0, 0));
        match settled_outcome(row, now) {
            Some(engaged) => {
                entry.0 += 1;
                if engaged {
                    entry.1 += 1;
                }
            }
            None => entry.2 += 1,
        }
    }
    if buckets.is_empty() {
        return None;
    }
    let lines = buckets
        .into_iter()
        .map(|((reason, band), (settled, engaged, pending))| {
            format!("{band} {reason}: engaged {engaged}/{settled}, pending {pending}")
        })
        .collect::<Vec<_>>();
    Some(lines.join("; "))
}

fn recent_pending_same_reason(
    ledger: &[crate::store::InterruptionLedgerRow],
    reason_type: &str,
    now: i64,
) -> bool {
    ledger.iter().any(|row| {
        row.reason_type == reason_type
            && row.reaction.is_none()
            && now.saturating_sub(row.delivered_at_unix) < LEDGER_IGNORE_SECONDS
    })
}

fn limit_stage2_words(input: &str, max_words: usize) -> String {
    input
        .split_whitespace()
        .take(max_words)
        .collect::<Vec<_>>()
        .join(" ")
}

// true when this reason at this focus band has a clear ignored pattern: at least
// three settled outcomes and none engaged.
fn reason_band_is_ignored(
    ledger: &[crate::store::InterruptionLedgerRow],
    reason_type: &str,
    focus_score: f32,
    now: i64,
) -> bool {
    let (delivered, engaged) = reason_band_stats(ledger, reason_type, focus_score, now);
    delivered >= 3 && engaged == 0
}

fn reason_band_stats(
    ledger: &[crate::store::InterruptionLedgerRow],
    reason_type: &str,
    focus_score: f32,
    now: i64,
) -> (u32, u32) {
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
    (delivered, engaged)
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

#[allow(dead_code)]
pub fn record_interruption_dismissal(store: &crate::store::TaskStore, task_id: i64) {
    let _ = store.record_interruption_reaction_within(task_id, LEDGER_IGNORE_SECONDS, "dismissed");
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

fn build_stage2_recall(
    jeff: &JeffState,
    _task_id: i64,
    snapshot: &SituationalSnapshot,
) -> Option<String> {
    if !jeff
        .store
        .get_privacy_user_profile_memory_enabled()
        .unwrap_or(false)
    {
        return None;
    }
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
    last_delivery_age_seconds: Option<i64>,
) -> String {
    format!(
        "Candidate reason: {}\nDetail: {}\nNatural boundary now: {}\nLast delivered interruption: {}\n\nSituation:\n{}\n\nMemory recall:\n{}\n\nInterruption ledger (pending means do not repeat it):\n{}\n\nDecide now.",
        reason.reason_type(),
        reason.detail(),
        awareness_core::is_at_natural_boundary(snapshot),
        last_delivery_age_seconds
            .map(|age| format!("{age}s ago"))
            .unwrap_or_else(|| "none".to_string()),
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
    let payload = Stage2DeliveryPayload {
        task_id,
        kind: kind.to_string(),
        message: message.to_string(),
    };

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

    // A silent card is durable and reply-addressable through its event payload,
    // but it is deliberately not inserted into chat and never notifies.
    if decision.channel == Stage2Channel::SilentCard {
        if jeff
            .store
            .record_event(
                task_id,
                STAGE2_SILENT_CARD_LOG_EVENT,
                &serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string()),
            )
            .is_err()
        {
            return false;
        }
        return handle.emit(STAGE2_SILENT_CARD_EVENT, &payload).is_ok();
    }

    // Bubble and voice both retain a conversation transcript, but only the
    // voice channel requests audio playback. The frontend owns session-aware
    // realtime/TTS playback for this event.
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

    if decision.channel == Stage2Channel::Voice {
        return handle.emit(STAGE2_VOICE_DELIVERY_EVENT, &payload).is_ok();
    }

    // Bubble raises a notification only when the overlay is hidden.
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
        assert!(!reason_band_is_ignored(
            &with_engaged,
            "task_return",
            0.8,
            now
        ));
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
        let summary = build_ledger_summary(&recent, now).unwrap();
        assert!(summary.contains("pending 1"));
        assert!(recent_pending_same_reason(&recent, "task_return", now));
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
    fn c1_malformed_stage2_output_fails_closed() {
        assert!(parse_stage2_json(
            r#"{"decision":"maybe","channel":"bubble","message":"Interrupt."}"#
        )
        .is_none());
        assert!(parse_stage2_json(
            r#"{"decision":"speak","channel":"pager","message":"Interrupt."}"#
        )
        .is_none());
        assert!(
            parse_stage2_json(r#"{"decision":"speak","channel":"bubble","message":""}"#).is_none()
        );
        let too_long = format!(
            "{{\"decision\":\"speak\",\"channel\":\"bubble\",\"message\":\"{}\"}}",
            vec!["word"; 41].join(" ")
        );
        assert!(parse_stage2_json(&too_long).is_none());
        assert_eq!(invalid_stage2_hold().verdict, Stage2Verdict::Hold);
    }

    #[test]
    fn c1_held_candidate_survives_ticks_and_expires() {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        let task = store.create_task("held").unwrap();
        let reason = ProactiveSpeechReason::BlockerDetected {
            blocker: "citation missing".to_string(),
        };
        persist_held_candidate(&store, task.id, &reason, None, 1_000);
        assert_eq!(
            load_held_candidate(&store, task.id, 1_001).unwrap().reason,
            reason
        );
        assert!(load_held_candidate(&store, task.id, 1_000 + HELD_CANDIDATE_TTL_SECONDS).is_none());

        let held = HeldCandidate {
            reason: ProactiveSpeechReason::DeadlinePressure {
                event: "Design review".to_string(),
                minutes_until: 30,
            },
            held_at_unix: 1_000,
            expires_at_unix: 2_000,
        };
        let mut snapshot = SituationalSnapshot::default();
        snapshot.time_pressure = Some(crate::awareness_core::TimePressure {
            source: "calendar".to_string(),
            description: "Design review".to_string(),
            minutes_until: Some(20),
        });
        assert!(held_candidate_is_relevant(&held, &snapshot, 1_100));
        snapshot.time_pressure.as_mut().unwrap().minutes_until = Some(-1);
        assert!(!held_candidate_is_relevant(&held, &snapshot, 1_100));
    }

    #[test]
    fn c1_silent_cards_are_durable_and_task_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        let task = store.create_task("cards").unwrap();
        let other = store.create_task("other").unwrap();
        let card = Stage2DeliveryPayload {
            task_id: task.id,
            kind: "proactive_deadline".to_string(),
            message: "The filing window closes soon.".to_string(),
        };
        store
            .record_event(
                task.id,
                STAGE2_SILENT_CARD_LOG_EVENT,
                &serde_json::to_string(&card).unwrap(),
            )
            .unwrap();
        assert_eq!(list_recent_silent_cards(&store, task.id, 10)[0].card, card);
        assert!(list_recent_silent_cards(&store, other.id, 10).is_empty());
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
        let audit = list_stage2_audit(&store, task.id, 10);
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].decision.as_deref(), Some("hold"));
        assert_eq!(audit[0].channel.as_deref(), Some("bubble"));
        assert_eq!(
            audit[0].decision_reason.as_deref(),
            Some("deep focus, wait for a boundary")
        );
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
