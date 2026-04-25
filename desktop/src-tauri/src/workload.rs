// workload.rs — cross-task workload summary and stale notification logic
//
// computes active/stale task lists with pending item counts.
// 5-minute in-memory cache handled at the command layer (cached_at not stored
// here to keep this module pure and testable).

use anyhow::Result;

use crate::{
    models::{WorkloadSummaryDto, WorkloadTaskDto},
    store::TaskStore,
};

const ACTIVE_WINDOW_DAYS: i64 = 14;

/// compute the current workload summary. caller is responsible for caching.
pub fn compute_workload_summary(store: &TaskStore) -> Result<WorkloadSummaryDto> {
    let all_tasks = store.list_tasks()?;
    let mut active_tasks: Vec<WorkloadTaskDto> = Vec::new();
    let mut stale_tasks: Vec<WorkloadTaskDto> = Vec::new();

    for task in all_tasks {
        let last_focus = store.get_last_task_focus(task.id)?;
        let days_since = last_focus
            .as_ref()
            .map(|ts| days_since_iso(ts))
            .unwrap_or(i64::MAX);
        let pending = count_pending_items(store, task.id)?;

        let dto = WorkloadTaskDto {
            id: task.id,
            title: task.title.clone(),
            last_focused_at: last_focus.clone(),
            days_since_focus: if days_since == i64::MAX {
                None
            } else {
                Some(days_since)
            },
            pending_item_count: pending,
            is_active: task.is_active,
        };

        if days_since <= ACTIVE_WINDOW_DAYS {
            active_tasks.push(dto);
        } else {
            stale_tasks.push(dto);
        }
    }

    // sort active tasks: most recently focused first
    active_tasks.sort_by(|a, b| {
        b.last_focused_at
            .as_deref()
            .cmp(&a.last_focused_at.as_deref())
    });
    // sort stale tasks: least recently focused first
    stale_tasks.sort_by(|a, b| {
        a.last_focused_at
            .as_deref()
            .cmp(&b.last_focused_at.as_deref())
    });

    Ok(WorkloadSummaryDto {
        active_tasks,
        stale_tasks,
    })
}

/// count pending items for a task: pending write proposals + running subtasks
/// + unreviewed speculative subtask results.
fn count_pending_items(store: &TaskStore, task_id: i64) -> Result<i64> {
    store.count_pending_items_for_task(task_id)
}

/// check for stale tasks with unreviewed subtask results and fire notifications.
/// this is called at app startup and on every record_task_focus event.
/// suppressed when quiet mode is active.
pub fn check_stale_task_notifications_noop(store: &TaskStore) -> Result<()> {
    // notification firing requires app_handle (Tauri runtime). the noop variant
    // is called from synchronous command context; actual notification dispatch
    // happens from the background poll task in main.rs where app_handle is available.
    let _ = store;
    Ok(())
}

/// check for stale tasks and fire one notification per task per 24h.
/// called from the background poll task in main.rs.
pub fn check_stale_task_notifications<R: tauri::Runtime>(
    store: &TaskStore,
    app: &tauri::AppHandle<R>,
    quiet_mode: bool,
) -> Result<()> {
    if quiet_mode {
        return Ok(());
    }
    let all_tasks = store.list_tasks()?;
    for task in all_tasks {
        let last_focus = store.get_last_task_focus(task.id)?;
        let days_since = last_focus
            .as_ref()
            .map(|ts| days_since_iso(ts))
            .unwrap_or(i64::MAX);
        if days_since <= ACTIVE_WINDOW_DAYS {
            continue;
        }
        // check for unreviewed speculative subtask results
        let has_unreviewed = store.task_has_unreviewed_speculative_results(task.id)?;
        if !has_unreviewed {
            continue;
        }
        // throttle: at most once per 24h per task
        let setting_key = format!("stale_notify_{}", task.id);
        let last_notified = store.get_app_setting(&setting_key)?;
        let should_notify = match last_notified.as_deref() {
            None => true,
            Some(ts) => days_since_iso(ts) >= 1,
        };
        if !should_notify {
            continue;
        }
        // fire notification
        let _ = crate::ambient::dispatch_notification(
            app,
            crate::ambient::NotificationPayload {
                title: format!("Unreviewed work in \"{}\"", task.title),
                body: format!(
                    "You left work unreviewed in \"{}\". Last focused {} days ago.",
                    task.title, days_since
                ),
                context_kind: Some("stale_task".to_string()),
                context_id: Some(task.id),
            },
        );
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        let _ = store.set_app_setting(&setting_key, &now);
    }
    Ok(())
}

fn days_since_iso(iso: &str) -> i64 {
    use chrono::{DateTime, Utc};
    let parsed = iso
        .parse::<DateTime<Utc>>()
        .ok()
        .or_else(|| {
            // try with space separator
            iso.replace(' ', "T")
                .parse::<DateTime<Utc>>()
                .ok()
        });
    match parsed {
        Some(dt) => {
            let now = Utc::now();
            (now - dt).num_days()
        }
        None => i64::MAX,
    }
}

// -------------------------------------------------------------------------
// tests
// -------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TaskStore;
    use tempfile::TempDir;

    fn test_store() -> (TempDir, TaskStore) {
        let dir = TempDir::new().expect("tempdir");
        let store = TaskStore::initialize(dir.path()).expect("store");
        (dir, store)
    }

    #[test]
    fn workload_summary_returns_empty_for_new_store() {
        let (_dir, store) = test_store();
        let summary = compute_workload_summary(&store).unwrap();
        assert!(summary.active_tasks.is_empty());
        assert!(summary.stale_tasks.is_empty());
    }

    #[test]
    fn task_with_no_focus_appears_in_stale() {
        let (_dir, store) = test_store();
        store.create_task("Unfocused Task").unwrap();
        let summary = compute_workload_summary(&store).unwrap();
        // no focus logged, so it counts as stale (days_since = MAX)
        assert_eq!(summary.stale_tasks.len(), 1);
        assert!(summary.active_tasks.is_empty());
    }

    #[test]
    fn task_with_recent_focus_appears_in_active() {
        let (_dir, store) = test_store();
        let task = store.create_task("Active Task").unwrap();
        store.record_task_focus(task.id).unwrap();
        let summary = compute_workload_summary(&store).unwrap();
        assert_eq!(summary.active_tasks.len(), 1);
        assert!(summary.stale_tasks.is_empty());
    }
}
