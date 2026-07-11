use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::store::TaskStore;

const SQLITE_NOW_EXPR: &str = "strftime('%Y-%m-%dT%H:%M:%fZ','now')";
const EMA_ALPHA: f32 = 0.1;

const STYLE_KEYS: [&str; 4] = [
    "prefers_opinions",
    "wants_explanations",
    "delegation_comfort",
    "interruption_tolerance",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GoalStatus {
    Active,
    Completed,
    Abandoned,
}

impl GoalStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Abandoned => "abandoned",
        }
    }

    fn from_db(value: &str) -> Self {
        match value {
            "completed" => Self::Completed,
            "abandoned" => Self::Abandoned,
            _ => Self::Active,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StatedGoal {
    pub id: i64,
    pub task_id: i64,
    pub goal_text: String,
    pub stated_at: String,
    pub status: GoalStatus,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StrugglePattern {
    pub id: i64,
    pub pattern_text: String,
    pub task_ids_json: String,
    pub first_seen: String,
    pub last_seen: String,
    pub occurrence_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CollaborationStyle {
    pub prefers_opinions: f32,
    pub wants_explanations: f32,
    pub delegation_comfort: f32,
    pub interruption_tolerance: f32,
}

impl Default for CollaborationStyle {
    fn default() -> Self {
        Self {
            prefers_opinions: 0.5,
            wants_explanations: 0.5,
            delegation_comfort: 0.5,
            interruption_tolerance: 0.5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TrustMetrics {
    pub times_accepted_opinion: i64,
    pub times_pushed_back: i64,
    pub times_asked_for_more: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RelationalProfile {
    pub stated_goals: Vec<StatedGoal>,
    pub struggle_patterns: Vec<StrugglePattern>,
    pub collaboration_style: CollaborationStyle,
    pub trust_metrics: TrustMetrics,
}

// retired prefix matcher; off the live path in b2. kept as the goal-eval
// contrast baseline (scored against the heuristic and llm extractors) and for
// backward-compatible tests.
#[allow(dead_code)]
pub fn extract_goal_from_text(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    for pattern in [
        "i'm working on",
        "i am working on",
        "i need to",
        "i'm trying to",
        "i am trying to",
        "my goal is",
        "i want to",
        "i'm trying to finish",
        "i am trying to finish",
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

pub fn message_asks_for_more(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("tell me more")
        || lower.contains("can you elaborate")
        || lower.contains("could you elaborate")
        || lower.contains("explain")
        || lower.contains("why?")
        || lower.trim() == "why"
}

pub fn message_marks_goal_done(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("i finished")
        || lower.contains("i'm finished")
        || lower.contains("i am finished")
        || lower.contains("done with")
        || lower.contains("i completed")
        || lower.contains("it's complete")
        || lower.contains("its complete")
}

pub fn record_message_signals(store: &TaskStore, task_id: i64, message: &str) -> Result<()> {
    record_proactive_engaged_if_reply(store, task_id)?;

    // apex b2: the retired prefix matcher is replaced on the live path by the
    // broader heuristic extractor. the reflex-tier llm extractor refines this
    // on conversation lulls (see the goal extraction loop in main.rs). the
    // relational model keeps its dedup/update semantics via record_goal_stated.
    if let Some((goal, confidence, _evidence)) =
        crate::goal_extraction::heuristic_goal_from_message(message)
    {
        if confidence >= crate::goal_extraction::RECORD_CONFIDENCE_MIN {
            record_goal_stated(store, task_id, &goal)?;
        }
    }

    if message_marks_goal_done(message) {
        mark_latest_active_goal_status(store, task_id, GoalStatus::Completed)?;
    }

    if message_asks_for_more(message) {
        record_asked_for_more(store)?;
    }

    Ok(())
}

pub fn record_goal_stated(store: &TaskStore, task_id: i64, goal_text: &str) -> Result<()> {
    let clean = goal_text.trim();
    if clean.is_empty() {
        return Ok(());
    }

    let conn = store.connect()?;
    let existing = list_goals_for_task_status(&conn, task_id, "active")?;
    if let Some(goal) = existing
        .into_iter()
        .find(|goal| is_similar_text(&goal.goal_text, clean))
    {
        conn.execute(
            &format!(
                "UPDATE stated_goals
                 SET goal_text = ?1, status = 'active', updated_at = ({now})
                 WHERE id = ?2",
                now = SQLITE_NOW_EXPR,
            ),
            params![clean, goal.id],
        )
        .context("failed to update stated goal")?;
        return Ok(());
    }

    conn.execute(
        &format!(
            "INSERT INTO stated_goals (task_id, goal_text, status, stated_at, updated_at)
             VALUES (?1, ?2, 'active', ({now}), ({now}))",
            now = SQLITE_NOW_EXPR,
        ),
        params![task_id, clean],
    )
    .context("failed to insert stated goal")?;
    Ok(())
}

// apex b2: the most recently updated active goal for a task, used by the
// snapshot as the primary current_goal source (populated by the extractor).
pub fn latest_active_goal_text(store: &TaskStore, task_id: i64) -> Option<String> {
    let conn = store.connect().ok()?;
    let goals = list_goals_for_task_status(&conn, task_id, "active").ok()?;
    goals
        .into_iter()
        .next()
        .map(|goal| goal.goal_text)
        .filter(|text| !text.trim().is_empty())
}

pub fn update_goal_status(store: &TaskStore, goal_id: i64, status: GoalStatus) -> Result<()> {
    let conn = store.connect()?;
    conn.execute(
        &format!(
            "UPDATE stated_goals
             SET status = ?1, updated_at = ({now})
             WHERE id = ?2",
            now = SQLITE_NOW_EXPR,
        ),
        params![status.as_str(), goal_id],
    )
    .context("failed to update stated goal status")?;
    Ok(())
}

pub fn mark_latest_active_goal_status(
    store: &TaskStore,
    task_id: i64,
    status: GoalStatus,
) -> Result<()> {
    let conn = store.connect()?;
    let goal_id = conn
        .query_row(
            "SELECT id
             FROM stated_goals
             WHERE task_id = ?1 AND status = 'active'
             ORDER BY updated_at DESC, id DESC
             LIMIT 1",
            params![task_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .context("failed to query latest active goal")?;

    drop(conn);
    if let Some(goal_id) = goal_id {
        update_goal_status(store, goal_id, status)?;
    }

    Ok(())
}

#[allow(dead_code)]
pub fn record_struggle(store: &TaskStore, task_id: i64, description: &str) -> Result<()> {
    let clean = description.trim();
    if clean.is_empty() {
        return Ok(());
    }

    let conn = store.connect()?;
    let mut stmt = conn
        .prepare(
            "SELECT id, pattern_text, task_ids_json, first_seen, last_seen, occurrence_count
             FROM struggle_patterns
             ORDER BY last_seen DESC, id DESC",
        )
        .context("failed to prepare struggle pattern query")?;
    let rows = stmt
        .query_map([], struggle_pattern_from_row)
        .context("failed to query struggle patterns")?;

    for row in rows {
        let pattern = row.context("failed to map struggle pattern")?;
        if !is_similar_text(&pattern.pattern_text, clean) {
            continue;
        }

        let mut task_ids = parse_task_ids(&pattern.task_ids_json);
        let last_seen_unix = conn
            .query_row(
                "SELECT CAST(strftime('%s', last_seen) AS INTEGER)
                 FROM struggle_patterns
                 WHERE id = ?1",
                params![pattern.id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()
            .context("failed to read struggle last_seen")?
            .flatten()
            .unwrap_or(0);

        if task_ids.contains(&task_id) && unix_now().saturating_sub(last_seen_unix) < 86_400 {
            return Ok(());
        }

        if !task_ids.contains(&task_id) {
            task_ids.push(task_id);
        }
        let task_ids_json = serde_json::to_string(&task_ids).unwrap_or_else(|_| "[]".to_string());
        conn.execute(
            &format!(
                "UPDATE struggle_patterns
                 SET pattern_text = ?1,
                     task_ids_json = ?2,
                     occurrence_count = occurrence_count + 1,
                     last_seen = ({now})
                 WHERE id = ?3",
                now = SQLITE_NOW_EXPR,
            ),
            params![clean, task_ids_json, pattern.id],
        )
        .context("failed to update struggle pattern")?;
        return Ok(());
    }

    let task_ids_json = serde_json::to_string(&vec![task_id]).unwrap_or_else(|_| "[]".to_string());
    conn.execute(
        &format!(
            "INSERT INTO struggle_patterns
             (pattern_text, task_ids_json, first_seen, last_seen, occurrence_count)
             VALUES (?1, ?2, ({now}), ({now}), 1)",
            now = SQLITE_NOW_EXPR,
        ),
        params![clean, task_ids_json],
    )
    .context("failed to insert struggle pattern")?;
    Ok(())
}

#[allow(dead_code)]
pub fn maybe_record_drift_struggle(store: &TaskStore, task_id: i64) -> Result<()> {
    let conn = store.connect()?;
    let count: i64 = conn
        .query_row(
            "SELECT
                (SELECT COUNT(*)
                 FROM proactive_trigger_log
                 WHERE task_id = ?1
                   AND trigger_type = 'drift'
                   AND suppressed = 0
                   AND CAST(strftime('%s', fired_at) AS INTEGER) >= CAST(strftime('%s','now','-7 days') AS INTEGER))
                +
                (SELECT COUNT(*)
                 FROM synthesis_log
                 WHERE task_id = ?1
                   AND reason_type = 'work_quality_observation'
                   AND CAST(strftime('%s', created_at) AS INTEGER) >= CAST(strftime('%s','now','-7 days') AS INTEGER))",
            params![task_id],
            |row| row.get(0),
        )
        .context("failed to count recent drift signals")?;

    if count < 3 {
        return Ok(());
    }

    let description = conn
        .query_row(
            "SELECT reason_detail
             FROM synthesis_log
             WHERE task_id = ?1
               AND reason_type = 'work_quality_observation'
               AND reason_detail IS NOT NULL
             ORDER BY id DESC
             LIMIT 1",
            params![task_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .context("failed to read latest drift reason")?
        .unwrap_or_else(|| "current work keeps drifting from the stated goal".to_string());

    drop(conn);
    record_struggle(store, task_id, &description)
}

pub fn record_opinion_accepted(store: &TaskStore) -> Result<()> {
    increment_trust_metric(store, "times_accepted_opinion")?;
    update_style_signal(store, "prefers_opinions", 0.9)
}

pub fn record_opinion_pushback(store: &TaskStore) -> Result<()> {
    increment_trust_metric(store, "times_pushed_back")?;
    update_style_signal(store, "prefers_opinions", 0.0)
}

pub fn record_asked_for_more(store: &TaskStore) -> Result<()> {
    increment_trust_metric(store, "times_asked_for_more")?;
    update_style_signal(store, "wants_explanations", 0.85)
}

pub fn record_delegation_accepted(store: &TaskStore) -> Result<()> {
    update_style_signal(store, "delegation_comfort", 0.8)
}

pub fn record_delegation_rejected(store: &TaskStore) -> Result<()> {
    update_style_signal(store, "delegation_comfort", 0.2)
}

pub fn record_proactive_engaged_if_reply(store: &TaskStore, task_id: i64) -> Result<()> {
    let last = store.list_recent_chat_messages(task_id, 1)?;
    if last
        .first()
        .map(|message| {
            message.role == "assistant" && message.message_kind.starts_with("proactive_")
        })
        .unwrap_or(false)
    {
        update_style_signal(store, "interruption_tolerance", 0.8)?;
    }
    Ok(())
}

pub fn record_proactive_dismissed(store: &TaskStore) -> Result<()> {
    update_style_signal(store, "interruption_tolerance", 0.2)
}

pub fn get_collaboration_style(store: &TaskStore) -> Result<CollaborationStyle> {
    let conn = store.connect()?;
    ensure_style_defaults(&conn)?;

    let mut style = CollaborationStyle::default();
    let mut stmt = conn
        .prepare("SELECT key, value FROM collaboration_style_signals")
        .context("failed to prepare collaboration style query")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f32>(1)?))
        })
        .context("failed to query collaboration style")?;

    for row in rows {
        let (key, value) = row.context("failed to map collaboration style")?;
        let value = value.clamp(0.0, 1.0);
        match key.as_str() {
            "prefers_opinions" => style.prefers_opinions = value,
            "wants_explanations" => style.wants_explanations = value,
            "delegation_comfort" => style.delegation_comfort = value,
            "interruption_tolerance" => style.interruption_tolerance = value,
            _ => {}
        }
    }

    Ok(style)
}

pub fn get_relational_profile(store: &TaskStore) -> Result<RelationalProfile> {
    let conn = store.connect()?;
    ensure_style_defaults(&conn)?;
    ensure_trust_metrics(&conn)?;

    let stated_goals = {
        let mut stmt = conn
            .prepare(
                "SELECT id, task_id, goal_text, stated_at, status, updated_at
                 FROM stated_goals
                 ORDER BY updated_at DESC, id DESC
                 LIMIT 50",
            )
            .context("failed to prepare stated goals query")?;
        let rows = stmt
            .query_map([], stated_goal_from_row)
            .context("failed to query stated goals")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to collect stated goals")?
    };

    let struggle_patterns = {
        let mut stmt = conn
            .prepare(
                "SELECT id, pattern_text, task_ids_json, first_seen, last_seen, occurrence_count
                 FROM struggle_patterns
                 ORDER BY last_seen DESC, id DESC
                 LIMIT 50",
            )
            .context("failed to prepare struggle patterns query")?;
        let rows = stmt
            .query_map([], struggle_pattern_from_row)
            .context("failed to query struggle patterns")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to collect struggle patterns")?
    };

    let trust_metrics = conn
        .query_row(
            "SELECT times_accepted_opinion, times_pushed_back, times_asked_for_more
             FROM trust_metrics
             WHERE id = 1",
            [],
            |row| {
                Ok(TrustMetrics {
                    times_accepted_opinion: row.get(0)?,
                    times_pushed_back: row.get(1)?,
                    times_asked_for_more: row.get(2)?,
                })
            },
        )
        .optional()
        .context("failed to query trust metrics")?
        .unwrap_or_default();

    drop(conn);
    let collaboration_style = get_collaboration_style(store)?;

    Ok(RelationalProfile {
        stated_goals,
        struggle_patterns,
        collaboration_style,
        trust_metrics,
    })
}

pub fn build_relational_context(store: &TaskStore) -> Result<Option<String>> {
    let profile = get_relational_profile(store)?;
    let active_goal = profile
        .stated_goals
        .iter()
        .find(|goal| goal.status == GoalStatus::Active);
    let recent_pattern = profile.struggle_patterns.first();
    let default_style = is_default_style(&profile.collaboration_style);

    if active_goal.is_none() && recent_pattern.is_none() && default_style {
        return Ok(None);
    }

    let mut parts = Vec::new();
    if let Some(goal) = active_goal {
        parts.push(format!(
            "stated goal: {}",
            truncate_chars(&goal.goal_text, 100)
        ));
    }
    if let Some(pattern) = recent_pattern {
        parts.push(format!(
            "recurring struggle: {}",
            truncate_chars(&pattern.pattern_text, 90)
        ));
    }

    let mut notes = Vec::new();
    if profile.collaboration_style.prefers_opinions > 0.7 {
        notes.push("values direct assessments");
    } else if profile.collaboration_style.prefers_opinions < 0.3 {
        notes.push("prefers options over opinions");
    }
    if profile.collaboration_style.wants_explanations > 0.7 {
        notes.push("wants brief reasons");
    }
    if profile.collaboration_style.interruption_tolerance < 0.3 {
        notes.push("interrupt sparingly");
    }
    if !notes.is_empty() {
        parts.push(format!("collaboration note: {}", notes.join("; ")));
    }

    if parts.is_empty() {
        return Ok(None);
    }

    Ok(Some(cap_words(
        &format!("[Relational context]\n{}", parts.join("\n")),
        80,
    )))
}

pub fn delete_stated_goal(store: &TaskStore, id: i64) -> Result<()> {
    let conn = store.connect()?;
    conn.execute("DELETE FROM stated_goals WHERE id = ?1", params![id])
        .context("failed to delete stated goal")?;
    Ok(())
}

pub fn delete_struggle_pattern(store: &TaskStore, id: i64) -> Result<()> {
    let conn = store.connect()?;
    conn.execute("DELETE FROM struggle_patterns WHERE id = ?1", params![id])
        .context("failed to delete struggle pattern")?;
    Ok(())
}

pub fn clear_relational_profile(store: &TaskStore) -> Result<()> {
    let conn = store.connect()?;
    conn.execute("DELETE FROM stated_goals", [])
        .context("failed to clear stated goals")?;
    conn.execute("DELETE FROM struggle_patterns", [])
        .context("failed to clear struggle patterns")?;
    conn.execute("DELETE FROM collaboration_style_signals", [])
        .context("failed to clear collaboration style")?;
    conn.execute("DELETE FROM trust_metrics", [])
        .context("failed to clear trust metrics")?;
    ensure_style_defaults(&conn)?;
    ensure_trust_metrics(&conn)?;
    Ok(())
}

fn increment_trust_metric(store: &TaskStore, column: &str) -> Result<()> {
    let conn = store.connect()?;
    ensure_trust_metrics(&conn)?;
    let sql = format!(
        "UPDATE trust_metrics
         SET {column} = {column} + 1,
             updated_at = ({SQLITE_NOW_EXPR})
         WHERE id = 1"
    );
    conn.execute(&sql, [])
        .with_context(|| format!("failed to increment trust metric {column}"))?;
    Ok(())
}

fn update_style_signal(store: &TaskStore, key: &str, target: f32) -> Result<()> {
    let conn = store.connect()?;
    ensure_style_defaults(&conn)?;
    let current = conn
        .query_row(
            "SELECT value FROM collaboration_style_signals WHERE key = ?1",
            params![key],
            |row| row.get::<_, f32>(0),
        )
        .optional()
        .context("failed to read collaboration style signal")?
        .unwrap_or(0.5);
    let next =
        ((current * (1.0 - EMA_ALPHA)) + (target.clamp(0.0, 1.0) * EMA_ALPHA)).clamp(0.0, 1.0);
    conn.execute(
        &format!(
            "INSERT INTO collaboration_style_signals (key, value, updated_at)
             VALUES (?1, ?2, ({now}))
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = ({now})",
            now = SQLITE_NOW_EXPR,
        ),
        params![key, next],
    )
    .context("failed to update collaboration style signal")?;
    Ok(())
}

fn ensure_style_defaults(conn: &rusqlite::Connection) -> Result<()> {
    for key in STYLE_KEYS {
        conn.execute(
            "INSERT OR IGNORE INTO collaboration_style_signals (key, value)
             VALUES (?1, 0.5)",
            params![key],
        )
        .with_context(|| format!("failed to ensure style default {key}"))?;
    }
    Ok(())
}

fn ensure_trust_metrics(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO trust_metrics
         (id, times_accepted_opinion, times_pushed_back, times_asked_for_more)
         VALUES (1, 0, 0, 0)",
        [],
    )
    .context("failed to ensure trust metrics row")?;
    Ok(())
}

fn list_goals_for_task_status(
    conn: &rusqlite::Connection,
    task_id: i64,
    status: &str,
) -> Result<Vec<StatedGoal>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, task_id, goal_text, stated_at, status, updated_at
             FROM stated_goals
             WHERE task_id = ?1 AND status = ?2
             ORDER BY updated_at DESC, id DESC",
        )
        .context("failed to prepare goals query")?;
    let rows = stmt
        .query_map(params![task_id, status], stated_goal_from_row)
        .context("failed to query goals")?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to collect goals")
}

fn stated_goal_from_row(row: &Row<'_>) -> rusqlite::Result<StatedGoal> {
    let status: String = row.get(4)?;
    Ok(StatedGoal {
        id: row.get(0)?,
        task_id: row.get(1)?,
        goal_text: row.get(2)?,
        stated_at: row.get(3)?,
        status: GoalStatus::from_db(&status),
        updated_at: row.get(5)?,
    })
}

fn struggle_pattern_from_row(row: &Row<'_>) -> rusqlite::Result<StrugglePattern> {
    Ok(StrugglePattern {
        id: row.get(0)?,
        pattern_text: row.get(1)?,
        task_ids_json: row.get(2)?,
        first_seen: row.get(3)?,
        last_seen: row.get(4)?,
        occurrence_count: row.get(5)?,
    })
}

#[allow(dead_code)]
fn parse_task_ids(raw: &str) -> Vec<i64> {
    serde_json::from_str::<Vec<i64>>(raw).unwrap_or_default()
}

fn is_similar_text(left: &str, right: &str) -> bool {
    let left = normalize_text(left);
    let right = normalize_text(right);
    if left.is_empty() || right.is_empty() {
        return false;
    }
    if left == right {
        return true;
    }
    if left.len().min(right.len()) < 8 {
        return false;
    }
    left.contains(&right) || right.contains(&left)
}

fn normalize_text(text: &str) -> String {
    text.to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_default_style(style: &CollaborationStyle) -> bool {
    (style.prefers_opinions - 0.5).abs() < 0.001
        && (style.wants_explanations - 0.5).abs() < 0.001
        && (style.delegation_comfort - 0.5).abs() < 0.001
        && (style.interruption_tolerance - 0.5).abs() < 0.001
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    value.chars().take(max_chars).collect::<String>()
}

fn cap_words(value: &str, max_words: usize) -> String {
    let words = value.split_whitespace().collect::<Vec<_>>();
    if words.len() <= max_words {
        return value.to_string();
    }
    format!("{}...", words[..max_words].join(" "))
}

#[allow(dead_code)]
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> (tempfile::TempDir, TaskStore) {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let store = TaskStore::initialize(dir.path()).expect("store");
        (dir, store)
    }

    #[test]
    fn goal_detected_in_im_working_on_pattern() {
        assert_eq!(
            extract_goal_from_text("I'm working on the introduction").as_deref(),
            Some("the introduction")
        );
    }

    #[test]
    fn collaboration_style_initialized_at_defaults() {
        let (_dir, store) = test_store();
        let style = get_collaboration_style(&store).unwrap();
        assert!((style.prefers_opinions - 0.5).abs() < 0.001);
        assert!((style.wants_explanations - 0.5).abs() < 0.001);
        assert!((style.delegation_comfort - 0.5).abs() < 0.001);
        assert!((style.interruption_tolerance - 0.5).abs() < 0.001);
    }

    #[test]
    fn build_relational_context_returns_none_with_no_signals() {
        let (_dir, store) = test_store();
        assert!(build_relational_context(&store).unwrap().is_none());
    }

    #[test]
    fn prefers_opinions_decreases_after_pushback() {
        let (_dir, store) = test_store();
        for _ in 0..5 {
            record_opinion_pushback(&store).unwrap();
        }
        let style = get_collaboration_style(&store).unwrap();
        assert!(style.prefers_opinions < 0.3, "{style:?}");
    }

    #[test]
    fn record_goal_stated_deduplicates_active_goal() {
        let (_dir, store) = test_store();
        let task = store.create_task("Essay").unwrap();
        record_goal_stated(&store, task.id, "finish the intro").unwrap();
        record_goal_stated(&store, task.id, "finish the intro").unwrap();
        let profile = get_relational_profile(&store).unwrap();
        assert_eq!(profile.stated_goals.len(), 1);
    }

    #[test]
    fn build_relational_context_includes_active_goal() {
        let (_dir, store) = test_store();
        let task = store.create_task("Essay").unwrap();
        record_goal_stated(&store, task.id, "finish the intro before midnight").unwrap();
        let context = build_relational_context(&store).unwrap().unwrap();
        assert!(context.contains("finish the intro"));
    }
}
