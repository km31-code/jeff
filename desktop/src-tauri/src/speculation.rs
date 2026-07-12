// apex d8: speculation scheduler. On idle, Jeff predicts likely upcoming
// requests and runs the top one as a read-only speculative job (agent_runtime),
// caching the result keyed by a normalized request signature. Incoming intents
// check the cache: a hit answers instantly, marked "already ran this while you
// were working"; misses are discarded silently and logged for the hit rate.
//
// Two invariants live here:
// 1. Speculative work is read-only. Jobs are created with speculative=1, which
//    forces the read-only tool registry and makes guard_speculative_action
//    reject any mutation (agent_runtime). Enforced at the scheduler boundary,
//    not by convention.
// 2. Spend is capped by the dedicated `speculation` sub-budget (cost_governor)
//    plus a hard daily prediction count cap.
//
// The predictor's quality (are the top-3 predictions what you will actually
// ask?) needs a Judgment-tier model and is env-gated; the deterministic
// predictor below is the tested fallback, and everything else in this module --
// cache, invalidation, serving, budget, read-only enforcement -- is
// deterministic and tested.

#![cfg_attr(test, allow(dead_code))]

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{
    agent_runtime,
    cost_governor::{self, SPECULATION_BUDGET_KEY},
    models::{SpeculationCacheDto, SpeculationServeResultDto, SpeculationStatusDto},
    store::TaskStore,
};

pub const SPECULATION_IDLE_MIN_SECONDS: i64 = 5 * 60;
pub const SPECULATION_MIN_INTERVAL_SECONDS: i64 = 10 * 60;
pub const SPECULATION_DAILY_PREDICTION_CAP: i64 = 48;
pub const SPECULATION_CACHE_TTL_SECONDS: i64 = 24 * 60 * 60;
pub const SPECULATION_MIN_PROBABILITY: f32 = 0.5;

pub const SPECULATION_ENABLED_KEY: &str = "speculation:enabled";
pub const SPECULATION_LAST_RUN_KEY: &str = "speculation:last_run";

pub const EVENT_PREDICTED: &str = "predicted";
pub const EVENT_HIT: &str = "hit";
pub const EVENT_MISS: &str = "miss";
pub const EVENT_INVALIDATED: &str = "invalidated";
pub const EVENT_REJECTED: &str = "rejected";

pub const STATUS_FRESH: &str = "fresh";
pub const STATUS_SERVED: &str = "served";
pub const STATUS_INVALIDATED: &str = "invalidated";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpeculationPrediction {
    pub request: String,
    pub probability: f32,
    pub signature: String,
}

impl SpeculationPrediction {
    pub fn new(request: &str, probability: f32) -> Self {
        Self {
            request: request.trim().to_string(),
            probability,
            signature: normalized_signature(request),
        }
    }
}

// default-on: speculation is read-only and budget-capped.
pub fn is_enabled(store: &TaskStore) -> bool {
    store
        .get_app_setting(SPECULATION_ENABLED_KEY)
        .ok()
        .flatten()
        .map(|raw| raw.trim() != "0" && !raw.trim().eq_ignore_ascii_case("false"))
        .unwrap_or(true)
}

pub fn set_enabled(store: &TaskStore, enabled: bool) -> Result<()> {
    store.set_app_setting(SPECULATION_ENABLED_KEY, if enabled { "1" } else { "0" })
}

// stable, order-insensitive-ish signature: lowercase, drop punctuation, collapse
// whitespace. Two phrasings of the same ask normalize to the same key.
pub fn normalized_signature(request: &str) -> String {
    request
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn within_speculation_budget(store: &TaskStore) -> bool {
    let Ok(spent) = store.sum_llm_usage_today(Some(SPECULATION_BUDGET_KEY)) else {
        return false;
    };
    let Ok(budget) = cost_governor::get_daily_budget_usd(store, SPECULATION_BUDGET_KEY) else {
        return false;
    };
    spent < budget
}

pub fn under_daily_prediction_cap(store: &TaskStore, task_id: i64) -> bool {
    predicted_today(store, task_id)
        .map(|count| count < SPECULATION_DAILY_PREDICTION_CAP)
        .unwrap_or(false)
}

pub fn should_run_speculation(store: &TaskStore, idle_seconds: i64, now: i64) -> bool {
    if !is_enabled(store) {
        return false;
    }
    if idle_seconds < SPECULATION_IDLE_MIN_SECONDS {
        return false;
    }
    let last = store
        .get_app_setting(SPECULATION_LAST_RUN_KEY)
        .ok()
        .flatten()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .unwrap_or(0);
    if now.saturating_sub(last) < SPECULATION_MIN_INTERVAL_SECONDS {
        return false;
    }
    let Some(task_id) = store.get_active_task().ok().flatten().map(|task| task.id) else {
        return false;
    };
    within_speculation_budget(store) && under_daily_prediction_cap(store, task_id)
}

pub fn mark_speculation_ran(store: &TaskStore, now: i64) -> Result<()> {
    store.set_app_setting(SPECULATION_LAST_RUN_KEY, &now.to_string())
}

// deterministic predictor: derive likely next asks from recent user requests and
// the active goal. The Judgment-tier model predictor (env-gated) plugs in at the
// app-handle path and falls back to this.
pub fn deterministic_predictions(
    recent_requests: &[String],
    goal: Option<&str>,
) -> Vec<SpeculationPrediction> {
    let mut predictions = Vec::new();
    if let Some(goal) = goal.map(str::trim).filter(|g| !g.is_empty()) {
        predictions.push(SpeculationPrediction::new(
            &format!("Prepare an assessment of progress on: {goal}"),
            0.6,
        ));
    }
    if let Some(last) = recent_requests.iter().rev().find(|r| !r.trim().is_empty()) {
        predictions.push(SpeculationPrediction::new(
            &format!("Follow up on the previous request: {}", last.trim()),
            0.55,
        ));
    }
    predictions.truncate(3);
    predictions
}

fn deterministic_predictions_with_context(
    recent_requests: &[String],
    goal: Option<&str>,
    snapshot: &str,
    recall: &[String],
) -> Vec<SpeculationPrediction> {
    let mut predictions = deterministic_predictions(recent_requests, goal);
    if predictions.len() < 3 {
        if let Some(memory) = recall.iter().find(|item| !item.trim().is_empty()) {
            predictions.push(SpeculationPrediction::new(
                &format!(
                    "Prepare a follow-up using remembered context: {}",
                    memory.trim()
                ),
                0.52,
            ));
        } else if !snapshot.trim().is_empty() {
            predictions.push(SpeculationPrediction::new(
                &format!(
                    "Prepare the next useful step from this task state: {}",
                    snapshot.trim()
                ),
                0.5,
            ));
        }
    }
    predictions.truncate(3);
    predictions
}

pub fn gather_recent_requests(store: &TaskStore, task_id: i64, limit: usize) -> Vec<String> {
    store
        .list_chat_messages(task_id)
        .map(|messages| {
            messages
                .into_iter()
                .filter(|m| m.role == "user" && !m.content.trim().is_empty())
                .map(|m| m.content)
                .rev()
                .take(limit)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

// idle here means "unengaged with Jeff": seconds since the last chat turn.
pub fn idle_seconds_for_task(store: &TaskStore, task_id: i64, now: i64) -> i64 {
    let last = store.list_chat_messages(task_id).ok().and_then(|messages| {
        messages
            .into_iter()
            .filter_map(|m| parse_epoch(&m.created_at))
            .max()
    });
    match last {
        Some(ts) => now.saturating_sub(ts),
        None => i64::MAX / 2,
    }
}

fn parse_epoch(raw: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.timestamp())
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.fZ")
                .map(|ndt| ndt.and_utc().timestamp())
                .ok()
        })
}

// run the top prediction (p >= threshold) as a read-only speculative job and
// cache its deliverable. Returns the cache row id when one is produced.
pub fn run_speculation_cycle(
    store: &TaskStore,
    task_id: i64,
    predictions: &[SpeculationPrediction],
    now: i64,
) -> Result<Option<i64>> {
    if !is_enabled(store)
        || !within_speculation_budget(store)
        || !under_daily_prediction_cap(store, task_id)
    {
        return Ok(None);
    }
    mark_speculation_ran(store, now)?;
    let Some(top) = predictions
        .iter()
        .filter(|p| p.probability >= SPECULATION_MIN_PROBABILITY)
        .max_by(|a, b| a.probability.partial_cmp(&b.probability).unwrap())
    else {
        append_event(store, task_id, EVENT_PREDICTED, None)?;
        return Ok(None);
    };
    // Count predictor work before execution, including predictions that later
    // fail. This makes the daily cap a hard call cap rather than a success cap.
    append_event(store, task_id, EVENT_PREDICTED, Some(&top.signature))?;

    // speculative=true forces the read-only registry + guard (agent_runtime).
    let budget = serde_json::json!({
        "max_steps": 6,
        "max_tool_calls": 5,
        "max_wall_seconds": 30,
        "max_tokens": 3000
    });
    let detail = agent_runtime::create_and_run_job(
        store,
        task_id,
        &top.request,
        Some(&budget.to_string()),
        true,
    )?;
    // runtime invariant: refuse to cache anything from a job that is not
    // read-only. guard_speculative_action rejects mutations for speculative
    // jobs; if it would *allow* one, the speculative flag was lost and we abort.
    if agent_runtime::guard_speculative_action(&detail.job, "file.write").is_ok() {
        return Err(anyhow::anyhow!(
            "speculation invariant violated: job {} is not read-only",
            detail.job.id
        ));
    }
    if detail.job.status != agent_runtime::JOB_STATUS_COMPLETED {
        append_event(store, task_id, EVENT_REJECTED, Some(&top.signature))?;
        return Ok(None);
    }
    let artifact = detail.job.deliverable_json.clone();
    let verified = artifact
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| value.get("verified").and_then(serde_json::Value::as_bool))
        .unwrap_or(false);
    if !verified
        || artifact
            .as_deref()
            .map(str::trim)
            .filter(|raw| !raw.is_empty())
            .is_none()
    {
        append_event(store, task_id, EVENT_REJECTED, Some(&top.signature))?;
        return Ok(None);
    }
    let cache_id = insert_cache_entry(
        store,
        task_id,
        &top.signature,
        &top.request,
        Some(detail.job.id),
        artifact.as_deref(),
    )?;
    Ok(Some(cache_id))
}

// serving: incoming intents check the cache. A non-invalidated entry answers
// instantly, marked precomputed; the ask is logged as a hit. A miss is logged.
pub fn serve_speculation(
    store: &TaskStore,
    incoming_request: &str,
) -> Result<Option<SpeculationServeResultDto>> {
    let Some(task_id) = store.get_active_task()?.map(|task| task.id) else {
        return Ok(None);
    };
    serve_speculation_for_task(store, task_id, incoming_request)
}

pub fn serve_speculation_for_task(
    store: &TaskStore,
    task_id: i64,
    incoming_request: &str,
) -> Result<Option<SpeculationServeResultDto>> {
    if !is_enabled(store) {
        return Ok(None);
    }
    invalidate_stale(store, unix_now())?;
    let signature = normalized_signature(incoming_request);
    if signature.is_empty() {
        return Ok(None);
    }
    let conn = store.connect()?;
    let row = conn
        .query_row(
            "SELECT id, request_text, artifact_json
             FROM speculation_cache
             WHERE task_id = ?1 AND request_signature = ?2 AND status = ?3
             ORDER BY id DESC LIMIT 1",
            params![task_id, signature, STATUS_FRESH],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
        .context("failed to look up speculation cache")?;
    drop(conn);

    match row {
        Some((cache_id, request_text, artifact_json)) => {
            mark_cache_status(store, cache_id, STATUS_SERVED)?;
            append_event(store, task_id, EVENT_HIT, Some(&signature))?;
            Ok(Some(SpeculationServeResultDto {
                request_text,
                artifact_json,
                precomputed: true,
                cache_id,
            }))
        }
        None => {
            append_event(store, task_id, EVENT_MISS, Some(&signature))?;
            Ok(None)
        }
    }
}

// invalidate all fresh entries for a task (document delta / calendar change).
pub fn invalidate_for_task(store: &TaskStore, task_id: i64) -> Result<usize> {
    let conn = store.connect()?;
    let changed = conn
        .execute(
            "UPDATE speculation_cache
             SET status = ?1, invalidated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE task_id = ?2 AND status != ?1",
            params![STATUS_INVALIDATED, task_id],
        )
        .context("failed to invalidate speculation cache for task")?;
    drop(conn);
    if changed > 0 {
        append_event(store, task_id, EVENT_INVALIDATED, None)?;
    }
    Ok(changed)
}

#[allow(dead_code)]
pub fn invalidate_for_calendar_change(store: &TaskStore, task_id: i64) -> Result<usize> {
    invalidate_for_task(store, task_id)
}

// invalidate entries older than the 24h TTL.
pub fn invalidate_stale(store: &TaskStore, now: i64) -> Result<usize> {
    let cutoff = now.saturating_sub(SPECULATION_CACHE_TTL_SECONDS);
    let cutoff_rfc = chrono::DateTime::<chrono::Utc>::from_timestamp(cutoff, 0)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default();
    let conn = store.connect()?;
    let task_ids = {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT task_id FROM speculation_cache
             WHERE status != ?1 AND created_at <= ?2",
        )?;
        let ids = stmt
            .query_map(params![STATUS_INVALIDATED, cutoff_rfc.clone()], |row| {
                row.get::<_, i64>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids
    };
    let changed = conn
        .execute(
            "UPDATE speculation_cache
             SET status = ?1, invalidated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE status != ?1 AND created_at <= ?2",
            params![STATUS_INVALIDATED, cutoff_rfc],
        )
        .context("failed to invalidate stale speculation cache")?;
    drop(conn);
    if changed > 0 {
        for task_id in task_ids {
            append_event(store, task_id, EVENT_INVALIDATED, None)?;
        }
    }
    Ok(changed)
}

pub fn speculation_status(store: &TaskStore) -> Result<SpeculationStatusDto> {
    let task_id = store.get_active_task()?.map(|task| task.id);
    let (predicted_count, hit_count, miss_count) = match task_id {
        Some(task_id) => event_counts_today(store, task_id)?,
        None => (0, 0, 0),
    };
    let fresh_cached = match task_id {
        Some(task_id) => fresh_cached_count(store, task_id)?,
        None => 0,
    };
    let denom = hit_count + miss_count;
    let hit_rate = if denom > 0 {
        hit_count as f32 / denom as f32
    } else {
        0.0
    };
    let spent_today_usd = store
        .sum_llm_usage_today(Some(SPECULATION_BUDGET_KEY))
        .unwrap_or(0.0);
    let daily_budget_usd = cost_governor::get_daily_budget_usd(store, SPECULATION_BUDGET_KEY)
        .unwrap_or_else(|_| cost_governor::default_daily_budget_usd(SPECULATION_BUDGET_KEY));
    Ok(SpeculationStatusDto {
        enabled: is_enabled(store),
        spent_today_usd,
        daily_budget_usd,
        within_budget: spent_today_usd < daily_budget_usd,
        hit_rate,
        predicted_count,
        hit_count,
        miss_count,
        fresh_cached,
    })
}

pub fn list_speculation_cache(store: &TaskStore, limit: usize) -> Result<Vec<SpeculationCacheDto>> {
    let Some(task_id) = store.get_active_task()?.map(|task| task.id) else {
        return Ok(Vec::new());
    };
    list_speculation_cache_for_task(store, task_id, limit)
}

pub fn list_speculation_cache_for_task(
    store: &TaskStore,
    task_id: i64,
    limit: usize,
) -> Result<Vec<SpeculationCacheDto>> {
    let conn = store.connect()?;
    let max = limit.min(200) as i64;
    let mut stmt = conn.prepare(
        "SELECT id, task_id, request_text, request_signature, job_id, status, created_at
         FROM speculation_cache
         WHERE task_id = ?1
         ORDER BY id DESC LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![task_id, max], |row| {
            Ok(SpeculationCacheDto {
                id: row.get(0)?,
                task_id: row.get(1)?,
                request_text: row.get(2)?,
                request_signature: row.get(3)?,
                job_id: row.get(4)?,
                status: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn discard_speculation_entry(store: &TaskStore, cache_id: i64) -> Result<()> {
    let task_id = store
        .get_active_task()?
        .map(|task| task.id)
        .ok_or_else(|| anyhow::anyhow!("no active task for speculation discard"))?;
    let conn = store.connect()?;
    let changed = conn
        .execute(
            "DELETE FROM speculation_cache WHERE id = ?1 AND task_id = ?2",
            params![cache_id, task_id],
        )
        .context("failed to discard speculation cache entry")?;
    if changed == 0 {
        return Err(anyhow::anyhow!(
            "speculation cache entry not found for active task"
        ));
    }
    Ok(())
}

fn insert_cache_entry(
    store: &TaskStore,
    task_id: i64,
    signature: &str,
    request_text: &str,
    job_id: Option<i64>,
    artifact_json: Option<&str>,
) -> Result<i64> {
    let conn = store.connect()?;
    conn.execute(
        "INSERT INTO speculation_cache
         (task_id, request_signature, request_text, job_id, artifact_json, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            task_id,
            signature,
            request_text,
            job_id,
            artifact_json,
            STATUS_FRESH
        ],
    )
    .context("failed to insert speculation cache entry")?;
    Ok(conn.last_insert_rowid())
}

fn mark_cache_status(store: &TaskStore, cache_id: i64, status: &str) -> Result<()> {
    let conn = store.connect()?;
    conn.execute(
        "UPDATE speculation_cache SET status = ?1 WHERE id = ?2",
        params![status, cache_id],
    )
    .context("failed to update speculation cache status")?;
    Ok(())
}

fn append_event(
    store: &TaskStore,
    task_id: i64,
    kind: &str,
    signature: Option<&str>,
) -> Result<()> {
    let conn = store.connect()?;
    conn.execute(
        "INSERT INTO speculation_events (task_id, kind, request_signature) VALUES (?1, ?2, ?3)",
        params![task_id, kind, signature],
    )
    .context("failed to append speculation event")?;
    Ok(())
}

fn predicted_today(store: &TaskStore, task_id: i64) -> Result<i64> {
    let conn = store.connect()?;
    conn.query_row(
        "SELECT COUNT(*) FROM speculation_events
         WHERE task_id = ?1 AND kind = ?2 AND date(created_at) = date('now')",
        params![task_id, EVENT_PREDICTED],
        |row| row.get(0),
    )
    .context("failed to count predictions today")
}

fn event_counts_today(store: &TaskStore, task_id: i64) -> Result<(i64, i64, i64)> {
    let conn = store.connect()?;
    let count = |kind: &str| -> Result<i64> {
        conn.query_row(
            "SELECT COUNT(*) FROM speculation_events
             WHERE task_id = ?1 AND kind = ?2 AND date(created_at) = date('now')",
            params![task_id, kind],
            |row| row.get(0),
        )
        .context("failed to count speculation events")
    };
    Ok((
        count(EVENT_PREDICTED)?,
        count(EVENT_HIT)?,
        count(EVENT_MISS)?,
    ))
}

fn fresh_cached_count(store: &TaskStore, task_id: i64) -> Result<i64> {
    let conn = store.connect()?;
    conn.query_row(
        "SELECT COUNT(*) FROM speculation_cache WHERE task_id = ?1 AND status = ?2",
        params![task_id, STATUS_FRESH],
        |row| row.get(0),
    )
    .context("failed to count fresh cached speculations")
}

#[allow(dead_code)]
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

// apex d8: app-path scheduler entry. The Judgment-tier predictor is env-gated;
// it falls back to the deterministic predictor. All downstream work (the
// speculative job, cache, budget) is deterministic and tested.
pub const SPECULATION_SYSTEM_PROMPT: &str = "You are Jeff's speculation predictor. \
Given the user's active goal and recent requests, predict the top 3 requests the user is most \
likely to make next. Return strict JSON only: \
{\"predictions\":[{\"request\":\"plain request\",\"probability\":0.0}]}. \
Probabilities in [0,1]. Predict only read-only preparation the user would plausibly ask for.";

pub fn maybe_run_for_active_task(
    store: &TaskStore,
    router: &crate::model_router::ModelRouter,
    now: i64,
) -> Result<Option<i64>> {
    let Some(task) = store.get_active_task()? else {
        return Ok(None);
    };
    // opportunistic ttl cleanup each tick.
    let _ = invalidate_stale(store, now);
    if !user_recently_active_on_task(store, task.id, now)? {
        return Ok(None);
    }
    let idle = idle_seconds_for_task(store, task.id, now);
    if !should_run_speculation(store, idle, now) {
        return Ok(None);
    }
    let recent = gather_recent_requests(store, task.id, 10);
    let goal = crate::relational_model::latest_active_goal_text(store, task.id);
    let snapshot = store.get_task_summary(task.id)?.summary_text;
    let recall = gather_recent_recall(store, task.id, 8)?;
    let predictions = predict_via_model(router, &recent, goal.as_deref(), &snapshot, &recall)
        .unwrap_or_else(|_| {
            deterministic_predictions_with_context(&recent, goal.as_deref(), &snapshot, &recall)
        });
    run_speculation_cycle(store, task.id, &predictions, now)
}

fn predict_via_model(
    router: &crate::model_router::ModelRouter,
    recent_requests: &[String],
    goal: Option<&str>,
    snapshot: &str,
    recall: &[String],
) -> Result<Vec<SpeculationPrediction>> {
    use crate::model_router::{GenerateOptions, ModelRequest, Tier};
    let prompt = build_prediction_prompt(recent_requests, goal, snapshot, recall);
    let mut request = ModelRequest::new(Tier::Judgment, SPECULATION_SYSTEM_PROMPT, prompt)
        .with_options(GenerateOptions {
            temperature: 0.2,
            max_tokens: Some(400),
            json_object: true,
            timeout_ms: Some(8_000),
        })
        .with_budget_key(SPECULATION_BUDGET_KEY);
    request.purpose = Some("speculation".to_string());
    let raw = router.route(request)?.text;
    parse_predictions_json(&raw)
}

fn build_prediction_prompt(
    recent_requests: &[String],
    goal: Option<&str>,
    snapshot: &str,
    recall: &[String],
) -> String {
    let recent = if recent_requests.is_empty() {
        "<none>".to_string()
    } else {
        recent_requests
            .iter()
            .take(10)
            .enumerate()
            .map(|(index, request)| format!("{}. {}", index + 1, request.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let recall = if recall.is_empty() {
        "<none>".to_string()
    } else {
        recall.join("\n")
    };
    format!(
        "Active goal:\n{}\n\nSituational snapshot:\n{}\n\nRelevant recent memory:\n{}\n\nRecent user requests (newest first):\n{}",
        goal.unwrap_or("<none>"),
        snapshot,
        recall,
        recent
    )
}

fn user_recently_active_on_task(store: &TaskStore, task_id: i64, now: i64) -> Result<bool> {
    let Some(last_focus) = store.get_last_task_focus(task_id)? else {
        return Ok(false);
    };
    let Some(focused_at) = parse_epoch(&last_focus) else {
        return Ok(false);
    };
    Ok(now.saturating_sub(focused_at) <= 30 * 60)
}

fn gather_recent_recall(store: &TaskStore, task_id: i64, limit: usize) -> Result<Vec<String>> {
    let conn = store.connect()?;
    let mut stmt = conn.prepare(
        "SELECT text FROM episodes WHERE task_id = ?1
         ORDER BY salience DESC, created_at DESC, id DESC LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![task_id, limit.min(32) as i64], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn parse_predictions_json(raw: &str) -> Result<Vec<SpeculationPrediction>> {
    #[derive(serde::Deserialize)]
    struct Raw {
        request: String,
        #[serde(default)]
        probability: f32,
    }
    #[derive(serde::Deserialize)]
    struct Envelope {
        #[serde(default)]
        predictions: Vec<Raw>,
    }
    let start = raw
        .find('{')
        .ok_or_else(|| anyhow::anyhow!("no JSON object in prediction response"))?;
    let end = raw
        .rfind('}')
        .ok_or_else(|| anyhow::anyhow!("no JSON object in prediction response"))?;
    let envelope: Envelope = serde_json::from_str(&raw[start..=end])
        .context("failed to parse speculation predictions")?;
    Ok(envelope
        .predictions
        .into_iter()
        .filter(|item| !item.request.trim().is_empty())
        .map(|item| SpeculationPrediction::new(&item.request, item.probability.clamp(0.0, 1.0)))
        .take(3)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost_governor::set_daily_budget_usd;
    use crate::store::LlmUsageLogInput;
    use tempfile::TempDir;

    fn test_store() -> (TempDir, TaskStore, i64) {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        let task = store.create_task("speculation").unwrap();
        store.record_task_focus(task.id).unwrap();
        (dir, store, task.id)
    }

    #[test]
    fn d8_signature_normalizes_phrasings() {
        assert_eq!(
            normalized_signature("What did the Reviewer say?!"),
            normalized_signature("what did   the reviewer say")
        );
    }

    #[test]
    fn d8_cache_hit_serves_and_marks_precomputed() {
        let (_dir, store, task_id) = test_store();
        let predictions = vec![SpeculationPrediction::new(
            "Summarize the methods section",
            0.7,
        )];
        let cache_id = run_speculation_cycle(&store, task_id, &predictions, 10_000)
            .unwrap()
            .expect("expected a cached prediction");
        assert!(cache_id > 0);

        let hit = serve_speculation(&store, "summarize the methods section!")
            .unwrap()
            .expect("expected a cache hit");
        assert!(hit.precomputed);
        assert_eq!(hit.cache_id, cache_id);

        let status = speculation_status(&store).unwrap();
        assert_eq!(status.predicted_count, 1);
        assert_eq!(status.hit_count, 1);
        assert!((status.hit_rate - 1.0).abs() < 0.001);
        // Served entries are one-use and cannot be replayed indefinitely.
        assert!(serve_speculation(&store, "summarize the methods section!")
            .unwrap()
            .is_none());
    }

    #[test]
    fn d8_document_delta_invalidates_cache_entry() {
        let (_dir, store, task_id) = test_store();
        let predictions = vec![SpeculationPrediction::new(
            "Summarize the methods section",
            0.8,
        )];
        run_speculation_cycle(&store, task_id, &predictions, 10_000).unwrap();

        let invalidated = invalidate_for_task(&store, task_id).unwrap();
        assert_eq!(invalidated, 1);
        // a previously-cached ask now misses.
        assert!(serve_speculation(&store, "summarize the methods section")
            .unwrap()
            .is_none());
        let status = speculation_status(&store).unwrap();
        assert_eq!(status.miss_count, 1);
        assert_eq!(status.fresh_cached, 0);
    }

    #[test]
    fn d8_stale_entries_invalidated_after_ttl() {
        let (_dir, store, task_id) = test_store();
        // cache rows are stamped with real wall-clock created_at, so anchor the
        // ttl comparison to real time.
        let now = chrono::Utc::now().timestamp();
        let predictions = vec![SpeculationPrediction::new("Draft the intro", 0.9)];
        run_speculation_cycle(&store, task_id, &predictions, now).unwrap();
        // a fresh entry is not yet stale.
        assert_eq!(invalidate_stale(&store, now).unwrap(), 0);
        // once the clock is past the ttl, the entry is invalidated.
        let future = now + SPECULATION_CACHE_TTL_SECONDS + 60;
        assert_eq!(invalidate_stale(&store, future).unwrap(), 1);
    }

    #[test]
    fn d8_speculative_job_is_read_only() {
        let (_dir, store, task_id) = test_store();
        let predictions = vec![SpeculationPrediction::new("Prep methods answer", 0.7)];
        run_speculation_cycle(&store, task_id, &predictions, 10_000).unwrap();
        let cache = list_speculation_cache(&store, 10).unwrap();
        assert_eq!(cache.len(), 1);
        let job_id = cache[0].job_id.unwrap();
        let detail = agent_runtime::get_job_detail(&store, job_id).unwrap();
        assert!(detail.job.speculative);
        assert!(detail.job.plan_json.contains("\"read_only\":true"));
        assert!(agent_runtime::guard_speculative_action(&detail.job, "file.write").is_err());
    }

    #[test]
    fn d8_budget_and_daily_cap_gate_scheduling() {
        let (_dir, store, task_id) = test_store();
        // idle + interval satisfied by default (last_run=0).
        assert!(should_run_speculation(
            &store,
            SPECULATION_IDLE_MIN_SECONDS,
            1_000_000
        ));

        // over the speculation sub-budget -> should not run.
        set_daily_budget_usd(&store, SPECULATION_BUDGET_KEY, 0.0).unwrap();
        store
            .append_llm_usage_log(&LlmUsageLogInput {
                tier: SPECULATION_BUDGET_KEY.to_string(),
                model: "test".to_string(),
                purpose: "speculation".to_string(),
                input_tokens: 1,
                output_tokens: 1,
                cached_tokens: 0,
                est_cost_usd: 0.5,
            })
            .unwrap();
        assert!(!within_speculation_budget(&store));
        assert!(!should_run_speculation(
            &store,
            SPECULATION_IDLE_MIN_SECONDS,
            1_000_000
        ));

        let (_dir2, capped_store, capped_task_id) = test_store();
        for _ in 0..SPECULATION_DAILY_PREDICTION_CAP {
            append_event(&capped_store, capped_task_id, EVENT_PREDICTED, None).unwrap();
        }
        assert!(!under_daily_prediction_cap(&capped_store, capped_task_id));
        assert!(run_speculation_cycle(
            &capped_store,
            capped_task_id,
            &[SpeculationPrediction::new("Prepare a bounded answer", 0.9)],
            chrono::Utc::now().timestamp()
        )
        .unwrap()
        .is_none());
        assert_eq!(
            predicted_today(&capped_store, capped_task_id).unwrap(),
            SPECULATION_DAILY_PREDICTION_CAP
        );
        assert!(task_id > 0);
    }

    #[test]
    fn d8_disabled_and_low_idle_block_scheduling() {
        let (_dir, store, _task_id) = test_store();
        assert!(should_run_speculation(
            &store,
            SPECULATION_IDLE_MIN_SECONDS,
            1_000_000
        ));
        // below idle threshold.
        assert!(!should_run_speculation(&store, 60, 1_000_000));
        // disabled.
        set_enabled(&store, false).unwrap();
        assert!(!is_enabled(&store));
        assert!(!should_run_speculation(
            &store,
            SPECULATION_IDLE_MIN_SECONDS,
            1_000_000
        ));
    }

    #[test]
    fn d8_discard_removes_entry() {
        let (_dir, store, task_id) = test_store();
        let predictions = vec![SpeculationPrediction::new("Draft the intro", 0.9)];
        let cache_id = run_speculation_cycle(&store, task_id, &predictions, 10_000)
            .unwrap()
            .unwrap();
        discard_speculation_entry(&store, cache_id).unwrap();
        assert!(list_speculation_cache(&store, 10).unwrap().is_empty());
    }

    #[test]
    fn d8_cache_is_task_scoped_and_served_entries_are_invalidated() {
        let (_dir, store, first_task_id) = test_store();
        let first_cache = run_speculation_cycle(
            &store,
            first_task_id,
            &[SpeculationPrediction::new("Summarize the shared ask", 0.9)],
            10_000,
        )
        .unwrap()
        .unwrap();
        let second = store.create_task("second task").unwrap();
        store.set_active_task(second.id).unwrap();
        assert!(
            serve_speculation_for_task(&store, second.id, "Summarize the shared ask")
                .unwrap()
                .is_none()
        );
        let served = serve_speculation_for_task(&store, first_task_id, "Summarize the shared ask")
            .unwrap()
            .unwrap();
        assert_eq!(served.cache_id, first_cache);
        assert_eq!(invalidate_for_task(&store, first_task_id).unwrap(), 1);
        assert!(
            serve_speculation_for_task(&store, first_task_id, "Summarize the shared ask")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn d8_does_not_cache_blocked_or_unverified_jobs() {
        let (_dir, store, task_id) = test_store();
        let cache = run_speculation_cycle(
            &store,
            task_id,
            &[SpeculationPrediction::new(
                "Search the live web for a source",
                0.9,
            )],
            10_000,
        )
        .unwrap();
        assert!(cache.is_none());
        assert!(list_speculation_cache_for_task(&store, task_id, 10)
            .unwrap()
            .is_empty());
    }
}
