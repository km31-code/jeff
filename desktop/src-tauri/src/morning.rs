// apex f2b: morning readiness -- the briefing is prepared ahead of first
// engagement, not composed on demand.
//
// F1c made the daemon survive the night; F2a moved consolidation onto the daemon
// so the day's episodes are distilled overnight. this module closes the loop: a
// background scheduler composes the day's briefing -- folding in the overnight
// work the daemon actually finished (completed jobs, standing-job runs) and
// yesterday's consolidated takeaways -- and persists it. delivery (briefing.rs)
// then retrieves it instead of composing, so the morning message is ready the
// moment you sit down and costs no model call at that moment.
//
// store-backed only, by design: the prepared briefing carries the substance the
// daemon can know without perception. the live, time-sensitive calendar line is
// left to the delivery path (which runs in the app, with EventKit) and to the
// meeting-imminent crisis channel.

use anyhow::Result;

use crate::briefing::{self, BriefingInputs};
use crate::models::PreparedBriefingDto;
use crate::state::JeffState;
use crate::workload;

// completed work newer than this (seconds) counts as "overnight" for the morning
// summary. a wide window so an evening standing-job run still surfaces at 8am.
const OVERNIGHT_WINDOW_SECONDS: i64 = 18 * 60 * 60;
const MAX_OVERNIGHT_ITEMS: usize = 3;

// prepare today's briefing if it has not been prepared yet. idempotent per local
// day: exactly one composition per day, so an every-few-minutes scheduler cannot
// run up overnight spend. returns the prepared briefing when a new one was
// composed, None when there was nothing to prepare or today was already prepared.
pub fn prepare_todays_briefing(state: &JeffState, now: i64) -> Result<Option<PreparedBriefingDto>> {
    let Some(task) = state.store.get_active_task()? else {
        return Ok(None);
    };
    let today = briefing::date_of(now);

    // one briefing per day: if today's is already prepared, do nothing (even if it
    // was already delivered -- re-preparing would let a stale scheduler tick
    // resurrect a delivered briefing).
    if state.store.get_prepared_briefing(&today)?.is_some() {
        return Ok(None);
    }

    let workload = workload::compute_workload_summary(&state.store)
        .map(|summary| {
            format!(
                "{} active task(s), {} stale",
                summary.active_tasks.len(),
                summary.stale_tasks.len()
            )
        })
        .unwrap_or_else(|_| "workload unavailable".to_string());

    let facts = if state
        .store
        .get_privacy_user_profile_memory_enabled()
        .unwrap_or(false)
    {
        briefing::memory_takeaways_for_date(&state.store, task.id, &briefing::date_of(yesterday(now)))
    } else {
        Vec::new()
    };

    let overnight = gather_overnight_work(state, task.id, now);
    let pending_approvals = briefing::pending_approvals_count(&state.store, task.id);

    let inputs = BriefingInputs {
        // calendar is live and time-sensitive; the delivery path and the
        // meeting-imminent crisis own it, not the overnight-prepared text.
        calendar: None,
        workload,
        facts: facts.clone(),
        pending_approvals,
        overnight: overnight.clone(),
    };
    let text = briefing::compose_briefing(&state.model_router, &inputs);
    if text.trim().is_empty() {
        return Ok(None);
    }

    let source = serde_json::json!({
        "overnight_items": overnight.len(),
        "facts": facts.len(),
        "pending_approvals": pending_approvals,
    })
    .to_string();
    state
        .store
        .upsert_prepared_briefing(&today, task.id, &text, &source, now)?;

    Ok(Some(PreparedBriefingDto {
        date: today,
        task_id: task.id,
        text,
        source_json: source,
        prepared_at: now,
        delivered: false,
    }))
}

// the concrete progress the daemon made while you were away: agent jobs that
// completed within the overnight window (standing-job runs land here too, since a
// standing job runs as a job). speculative jobs never count -- they are read-only
// precomputation, not delivered work.
fn gather_overnight_work(state: &JeffState, task_id: i64, now: i64) -> Vec<String> {
    let jobs = crate::agent_runtime::list_jobs(&state.store, Some(task_id), 40).unwrap_or_default();
    jobs.into_iter()
        .filter(|job| job.status == crate::agent_runtime::JOB_STATUS_COMPLETED && !job.speculative)
        .filter(|job| {
            crate::awareness_core::parse_sqlite_datetime_to_unix(&job.updated_at)
                .map(|completed_at| now.saturating_sub(completed_at) <= OVERNIGHT_WINDOW_SECONDS)
                .unwrap_or(false)
        })
        .map(|job| {
            let goal = job.goal_contract.trim();
            let goal = if goal.chars().count() > 80 {
                format!("{}...", goal.chars().take(80).collect::<String>())
            } else {
                goal.to_string()
            };
            format!("finished \"{goal}\"")
        })
        .take(MAX_OVERNIGHT_ITEMS)
        .collect()
}

fn yesterday(now: i64) -> i64 {
    now.saturating_sub(24 * 60 * 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_router::{ModelRouter, RouterConfig};
    use crate::retrieval::default_embeddings_provider;
    use crate::state::JeffState;
    use crate::store::TaskStore;
    use crate::voice::OpenAiVoiceProvider;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn test_state() -> (TempDir, JeffState) {
        let dir = TempDir::new().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        let local_runtime = Arc::new(crate::local_runtime::LocalRuntime::new(dir.path()));
        let embeddings = Arc::new(default_embeddings_provider(local_runtime.clone()));
        let router = Arc::new(ModelRouter::new(RouterConfig::default()));
        let voice = Arc::new(OpenAiVoiceProvider::from_env());
        let state = JeffState::new(store, embeddings, local_runtime, router, voice);
        (dir, state)
    }

    fn now() -> i64 {
        chrono::Utc::now().timestamp()
    }

    #[test]
    fn f2b_prepare_requires_an_active_task() {
        // no active task -> nothing to prepare.
        let (_dir, state) = test_state();
        assert!(prepare_todays_briefing(&state, now()).unwrap().is_none());
    }

    #[test]
    fn f2b_prepare_is_idempotent_per_day() {
        let (_dir, state) = test_state();
        state.store.create_task("Thesis").unwrap();
        let n = now();
        let first = prepare_todays_briefing(&state, n).unwrap();
        assert!(first.is_some(), "first prepare should compose a briefing");
        // a second pass the same day must not re-compose -- one briefing per day.
        assert!(
            prepare_todays_briefing(&state, n).unwrap().is_none(),
            "second same-day prepare must be a no-op"
        );
        // and it is retrievable for delivery.
        let stored = state
            .store
            .get_prepared_briefing(&briefing::date_of(n))
            .unwrap();
        assert!(stored.is_some());
        assert!(!stored.unwrap().delivered);
    }

    #[test]
    fn f2b_overnight_work_folds_completed_jobs_into_the_briefing() {
        let (_dir, state) = test_state();
        let task = state.store.create_task("Thesis").unwrap();
        // a completed, non-speculative job stamped now -> counts as overnight work.
        state
            .store
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO jobs (task_id, goal_contract, plan_json, budget_json, status, speculative, created_at, updated_at)
                 VALUES (?1, 'check the citations in chapter 2', '[]', '{}', 'completed', 0,
                         strftime('%Y-%m-%dT%H:%M:%fZ','now'), strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
                rusqlite::params![task.id],
            )
            .unwrap();
        let prepared = prepare_todays_briefing(&state, now()).unwrap().unwrap();
        assert!(
            prepared.text.to_lowercase().contains("citations"),
            "the prepared briefing must mention the overnight work: {}",
            prepared.text
        );
    }

    #[test]
    fn f2b_speculative_jobs_are_not_reported_as_overnight_work() {
        let (_dir, state) = test_state();
        let task = state.store.create_task("Thesis").unwrap();
        // a completed but speculative job -> read-only precompute, never "work done".
        state
            .store
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO jobs (task_id, goal_contract, plan_json, budget_json, status, speculative, created_at, updated_at)
                 VALUES (?1, 'speculative draft nobody asked for', '[]', '{}', 'completed', 1,
                         strftime('%Y-%m-%dT%H:%M:%fZ','now'), strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
                rusqlite::params![task.id],
            )
            .unwrap();
        let overnight = gather_overnight_work(&state, task.id, now());
        assert!(
            overnight.is_empty(),
            "speculative work must never be reported as delivered overnight work"
        );
    }
}
