// apex f1a: the headless-capable core.
//
// every recurring background scheduler and startup task that a future headless
// daemon (f1b) must run lid-closed lives here behind a single start/stop
// lifecycle, instead of being inlined in the tauri setup closure. this is the
// "re-homing, not a rewrite" seam: f1a consolidates the loops in-process with
// no behavior change; f1b re-homes this module into a separate process. shell
// concerns (tray, overlay, hotkey, wake word, login item, the subtask companion
// relay wired before state is managed) stay in main.rs -- a daemon does not run
// them. the loop bodies below are moved verbatim from main.rs; the only
// additions are the cooperative shutdown check at the top of each loop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager};

use crate::ambient::AmbientState;
use crate::state::{CalendarState, ContextState, JeffState};
use crate::typing_activity::TypingActivityState;
use crate::{
    agent_runtime, awareness_core, calendar, context_observer, coworking, crisis, proactive,
    speculation, workload,
};

// a cooperative shutdown signal shared with every core loop. loops check it at
// the top of each iteration and exit; CoreHandle::stop also aborts the spawned
// tasks so a loop parked on a long sleep terminates promptly.
#[derive(Clone, Default)]
pub struct CoreShutdown(Arc<AtomicBool>);

impl CoreShutdown {
    pub fn is_stopped(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
    pub fn stop(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

// lifecycle handle for the core. holding it keeps ownership of the scheduler
// task handles; dropping it detaches them so they keep running for the process
// lifetime, matching the pre-f1a inlined behavior. stop() signals cooperative
// shutdown and aborts every core task -- exercised by tests today and by the
// f1b headless daemon on teardown.
pub struct CoreHandle {
    shutdown: CoreShutdown,
    joins: Vec<JoinHandle<()>>,
}

impl CoreHandle {
    #[allow(dead_code)]
    pub fn stop(self) {
        self.shutdown.stop();
        for join in self.joins {
            join.abort();
        }
    }
}

// start every recurring background scheduler and startup task. called once from
// the tauri setup closure after state is managed. behavior is identical to the
// pre-f1a inlined loops.
pub fn start(app: &AppHandle) -> CoreHandle {
    let shutdown = CoreShutdown::default();
    let mut joins: Vec<JoinHandle<()>> = Vec::new();

    joins.push(spawn_active_window_context_poll(
        app.clone(),
        shutdown.clone(),
    ));
    joins.push(spawn_typing_activity_sync(app.clone(), shutdown.clone()));
    joins.push(spawn_calendar_poll(app.clone(), shutdown.clone()));
    joins.push(spawn_stale_task_check(app.clone(), shutdown.clone()));

    // phase 27: the proactive ambient monitor self-spawns its own task.
    proactive::spawn_ambient_monitor(app.clone());

    joins.push(spawn_content_observation(app.clone()));
    joins.push(spawn_goal_extraction(app.clone()));
    joins.push(spawn_memory_session_summary(app.clone()));
    joins.push(spawn_memory_consolidation(app.clone()));
    joins.push(spawn_update_check(app.clone(), shutdown.clone()));
    joins.push(spawn_job_resume(app.clone(), shutdown.clone()));
    joins.push(spawn_standing_job_scheduler(app.clone(), shutdown.clone()));
    joins.push(spawn_speculation_scheduler(app.clone(), shutdown.clone()));

    CoreHandle { shutdown, joins }
}

// phase 20: active-window context polling loop (3-second interval, first poll
// immediate). emits context://context-updated after every poll and
// context://document-switch when the frontmost document title changes to one
// not yet nudged this session and the document is off-task.
fn spawn_active_window_context_poll(app: AppHandle, shutdown: CoreShutdown) -> JoinHandle<()> {
    let poll_handle = app;
    tauri::async_runtime::spawn(async move {
        loop {
            if shutdown.is_stopped() {
                break;
            }
            // skip polling when quiet mode is active.
            let quiet = poll_handle
                .try_state::<AmbientState>()
                .map(|s| s.is_quiet_mode())
                .unwrap_or(false);

            if quiet {
                // still emit a null context so the frontend clears stale display.
                let _ = poll_handle.emit("context://context-updated", serde_json::Value::Null);
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                continue;
            }

            let active_context_allowed = poll_handle
                .try_state::<JeffState>()
                .map(|s| {
                    let onboarding_complete = s.store.get_onboarding_complete().unwrap_or(false);
                    let privacy_enabled = s
                        .store
                        .get_privacy_active_window_context_enabled()
                        .unwrap_or(true);
                    onboarding_complete && privacy_enabled
                })
                .unwrap_or(false);

            if !active_context_allowed {
                if let Some(ctx_state) = poll_handle.try_state::<ContextState>() {
                    ctx_state.update(None);
                }
                let _ = poll_handle.emit("context://context-updated", serde_json::Value::Null);
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                continue;
            }

            let new_ctx = context_observer::poll_active_window();

            let Some(ctx_state) = poll_handle.try_state::<ContextState>() else {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                continue;
            };

            let prior_title = ctx_state.current().map(|ctx| ctx.document_title);
            let new_title = new_ctx.as_ref().map(|ctx| ctx.document_title.clone());
            let document_title_changed = matches!(
                (prior_title.as_deref(), new_title.as_deref()),
                (Some(previous), Some(next)) if !next.is_empty() && previous != next
            );

            // fire the document-switch nudge before updating state so we compare
            // the incoming title against the last-known one.
            if context_observer::is_accessibility_trusted() {
                if let Some(ref ctx) = new_ctx {
                    let title = &ctx.document_title;
                    if !title.is_empty() && ctx_state.should_nudge_for_switch(title) {
                        // suppress nudge when the document title matches the
                        // active task title (user is on-task).
                        let task_title = poll_handle
                            .try_state::<JeffState>()
                            .and_then(|s| s.store.get_active_task().ok().flatten())
                            .map(|t| t.title);
                        let off_task = task_title
                            .as_deref()
                            .map_or(true, |t| crate::document_is_off_task(title, t));
                        if off_task {
                            let _ = poll_handle.emit(
                                "context://document-switch",
                                serde_json::json!({
                                    "app_name": ctx.app_name,
                                    "document_title": ctx.document_title,
                                }),
                            );
                            ctx_state.mark_nudged(title.clone());
                        }
                    }
                }
            }

            ctx_state.update(new_ctx);
            if document_title_changed {
                if let Some(task) = poll_handle
                    .try_state::<JeffState>()
                    .and_then(|s| s.store.get_active_task().ok().flatten())
                {
                    awareness_core::spawn_awareness_update(
                        &poll_handle,
                        awareness_core::SnapshotTrigger::WindowSwitch,
                        task.id,
                    );
                }
            }

            // emit context-updated so the frontend tracks current state without
            // needing its own polling interval.
            let context_payload = ctx_state.current().map(|ctx| {
                serde_json::json!({
                    "app_name": ctx.app_name,
                    "document_title": ctx.document_title,
                    "captured_at": ctx.captured_at,
                })
            });
            let _ = poll_handle.emit(
                "context://context-updated",
                context_payload.unwrap_or(serde_json::Value::Null),
            );

            // sleep at end so the first poll runs immediately on startup.
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
    })
}

// phase 22: keep rate-only typing state in sync with privacy and expose only a
// boolean event for the frontend tts queue.
fn spawn_typing_activity_sync(app: AppHandle, shutdown: CoreShutdown) -> JoinHandle<()> {
    let typing_handle = app;
    tauri::async_runtime::spawn(async move {
        let mut last_typing: Option<bool> = None;
        loop {
            if shutdown.is_stopped() {
                break;
            }
            let Some(typing_state) = typing_handle.try_state::<TypingActivityState>() else {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                continue;
            };

            let enabled = typing_handle
                .try_state::<JeffState>()
                .and_then(|state| state.store.get_privacy_typing_activity_enabled().ok())
                .unwrap_or(true);
            typing_state.set_enabled(enabled);
            let is_typing = enabled && typing_state.is_typing();

            if last_typing != Some(is_typing) {
                last_typing = Some(is_typing);
                let _ = typing_handle.emit(
                    "typing://activity-changed",
                    serde_json::json!({
                        "is_typing": is_typing,
                        "rate_only": true,
                        "monitor_available": typing_state.monitor_available(),
                        "last_error": typing_state.last_error(),
                    }),
                );
                if let Some(state) = typing_handle.try_state::<JeffState>() {
                    if let Ok(now) = coworking::unix_now_seconds() {
                        if let Ok(mut runtime) = state.coworking.lock() {
                            let _ = runtime.set_user_typing(is_typing, now);
                        }
                    }
                }
            }

            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    })
}

// phase 23: calendar poll task (60-second interval). polls EventKit for the next
// upcoming event when the privacy gate is enabled and the OS permission has been
// granted, and drives the meeting-imminent / deadline-collision crisis classes.
fn spawn_calendar_poll(app: AppHandle, shutdown: CoreShutdown) -> JoinHandle<()> {
    let cal_poll_handle = app;
    tauri::async_runtime::spawn(async move {
        loop {
            if shutdown.is_stopped() {
                break;
            }
            let quiet = cal_poll_handle
                .try_state::<AmbientState>()
                .map(|s| s.is_quiet_mode())
                .unwrap_or(false);
            if quiet {
                if let Some(cs) = cal_poll_handle.try_state::<CalendarState>() {
                    cs.update(None);
                }
                let _ = cal_poll_handle.emit("calendar://event-updated", serde_json::Value::Null);
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                continue;
            }

            let cal_enabled = cal_poll_handle
                .try_state::<JeffState>()
                .and_then(|s| s.store.get_privacy_calendar_context_enabled().ok())
                .unwrap_or(false);

            if !cal_enabled {
                // clear stale event if feature is turned off mid-session
                if let Some(cs) = cal_poll_handle.try_state::<CalendarState>() {
                    cs.update(None);
                }
                let _ = cal_poll_handle.emit("calendar://event-updated", serde_json::Value::Null);
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                continue;
            }

            let next_event = calendar::fetch_next_event(8);

            if let Some(cs) = cal_poll_handle.try_state::<CalendarState>() {
                cs.update(next_event.clone());
            }

            let movement_toward_event = next_event
                .as_ref()
                .and_then(|event| {
                    cal_poll_handle
                        .try_state::<ContextState>()
                        .and_then(|ctx| ctx.current())
                        .map(|ctx| {
                            crate::crisis_event_matches_context(&event.title, &ctx.document_title)
                        })
                })
                .unwrap_or(false);

            let _ = cal_poll_handle.emit(
                "calendar://event-updated",
                serde_json::to_value(&next_event).unwrap_or(serde_json::Value::Null),
            );
            if let Some(task) = cal_poll_handle
                .try_state::<JeffState>()
                .and_then(|s| s.store.get_active_task().ok().flatten())
            {
                awareness_core::spawn_awareness_update(
                    &cal_poll_handle,
                    awareness_core::SnapshotTrigger::CalendarEvent,
                    task.id,
                );
                if let Some(event) = next_event.as_ref() {
                    crisis::maybe_fire_meeting_imminent(
                        &cal_poll_handle,
                        task.id,
                        event,
                        movement_toward_event,
                    );
                    let far_from_done = cal_poll_handle
                        .try_state::<JeffState>()
                        .map(|state| {
                            state
                                .awareness_core
                                .snapshot_immediate()
                                .work_understanding
                                .as_ref()
                                .map(|understanding| {
                                    !understanding.weak_points.is_empty()
                                        || understanding
                                            .stuck_signal
                                            .as_deref()
                                            .map(|value| !value.trim().is_empty())
                                            .unwrap_or(false)
                                })
                                .unwrap_or(false)
                        })
                        .unwrap_or(false);
                    crisis::maybe_fire_deadline_collision(
                        &cal_poll_handle,
                        task.id,
                        event.minutes_until,
                        far_from_done,
                    );
                }
            }

            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    })
}

// phase 23: stale-task notification check at startup.
fn spawn_stale_task_check(app: AppHandle, shutdown: CoreShutdown) -> JoinHandle<()> {
    let stale_check_handle = app;
    tauri::async_runtime::spawn(async move {
        // give the app a few seconds to finish startup before checking
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        if shutdown.is_stopped() {
            return;
        }
        if let Some(jeff_state) = stale_check_handle.try_state::<JeffState>() {
            let quiet = stale_check_handle
                .try_state::<AmbientState>()
                .map(|s| s.is_quiet_mode())
                .unwrap_or(false);
            let _ = workload::check_stale_task_notifications(
                &jeff_state.store,
                &stale_check_handle,
                quiet,
            );
        }
    })
}

// phase 31: content observation poll -- every 10 seconds. reads the active
// document text via AXUIElement when the per-task privacy toggle is enabled.
fn spawn_content_observation(app: AppHandle) -> JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        crate::spawn_content_observation_poll(app).await;
    })
}

// apex b2: goal extraction loop on conversation lulls or task switches.
fn spawn_goal_extraction(app: AppHandle) -> JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        crate::spawn_goal_extraction_poll(app).await;
    })
}

// apex b3: session-summary episodes on long idle lulls.
fn spawn_memory_session_summary(app: AppHandle) -> JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        crate::spawn_memory_session_summary_poll(app).await;
    })
}

// apex b4: consolidate typed episodes into durable facts on idle windows or the
// once-daily 02:00 maintenance window.
fn spawn_memory_consolidation(app: AppHandle) -> JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        crate::spawn_memory_consolidation_poll(app).await;
    })
}

// phase 24: background update check -- delayed 2 seconds so it does not compete
// with tray-ready or session-restore on startup.
fn spawn_update_check(app: AppHandle, shutdown: CoreShutdown) -> JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        if shutdown.is_stopped() {
            return;
        }
        crate::perform_update_check(app).await;
    })
}

// apex d6: resume any job left pending/running by a previous session once at
// startup so checkpointed jobs continue from their last completed step.
fn spawn_job_resume(app: AppHandle, shutdown: CoreShutdown) -> JoinHandle<()> {
    let resume_handle = app;
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        if shutdown.is_stopped() {
            return;
        }
        if let Some(state) = resume_handle.try_state::<JeffState>() {
            match agent_runtime::resume_incomplete_jobs_with_router(
                &state.store,
                &state.model_router,
            ) {
                Ok(resumed) if !resumed.is_empty() => {
                    eprintln!(
                        "[jeff] resumed {} incomplete agent job(s) at startup",
                        resumed.len()
                    );
                }
                Ok(_) => {}
                Err(err) => {
                    eprintln!("[jeff] startup job resume failed: {err:#}");
                }
            }
        }
    })
}

// apex d6: standing-job scheduler (60-second interval). fires daily-due standing
// jobs automatically; each run posts a receipt to the unified audit log.
fn spawn_standing_job_scheduler(app: AppHandle, shutdown: CoreShutdown) -> JoinHandle<()> {
    let standing_handle = app;
    tauri::async_runtime::spawn(async move {
        loop {
            if shutdown.is_stopped() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            if let Some(state) = standing_handle.try_state::<JeffState>() {
                match agent_runtime::run_due_standing_jobs_with_router(
                    &state.store,
                    &state.model_router,
                    None,
                ) {
                    Ok(ran) if !ran.is_empty() => {
                        eprintln!("[jeff] ran {} due standing job(s)", ran.len());
                    }
                    Ok(_) => {}
                    Err(err) => {
                        eprintln!("[jeff] standing-job scheduler tick failed: {err:#}");
                    }
                }
            }
        }
    })
}

// apex d8: speculation scheduler (~10-minute cadence). when the user is
// unengaged with Jeff but working, precomputes the likely next request as a
// read-only speculative job. speculative jobs cannot mutate (agent_runtime).
fn spawn_speculation_scheduler(app: AppHandle, shutdown: CoreShutdown) -> JoinHandle<()> {
    let speculation_handle = app;
    tauri::async_runtime::spawn(async move {
        loop {
            if shutdown.is_stopped() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(10 * 60)).await;
            let quiet = speculation_handle
                .try_state::<AmbientState>()
                .map(|s| s.is_quiet_mode())
                .unwrap_or(false);
            if quiet {
                continue;
            }
            if let Some(state) = speculation_handle.try_state::<JeffState>() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                match speculation::maybe_run_for_active_task(&state.store, &state.model_router, now)
                {
                    Ok(Some(cache_id)) => {
                        eprintln!("[jeff] speculation precomputed cache entry {cache_id}");
                    }
                    Ok(None) => {}
                    Err(err) => {
                        eprintln!("[jeff] speculation tick failed: {err:#}");
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::time::Duration;

    #[test]
    fn f1a_core_shutdown_signals_a_loop_to_stop() {
        // the cooperative-shutdown contract every core loop relies on: a loop
        // watching CoreShutdown must exit once stop() is called. proven with a
        // real thread so join() blocks until the loop actually returns.
        let shutdown = CoreShutdown::default();
        assert!(!shutdown.is_stopped());
        let loop_shutdown = shutdown.clone();
        let ticks = Arc::new(AtomicU64::new(0));
        let loop_ticks = ticks.clone();
        let worker = std::thread::spawn(move || {
            while !loop_shutdown.is_stopped() {
                loop_ticks.fetch_add(1, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(1));
            }
        });

        std::thread::sleep(Duration::from_millis(20));
        assert!(!shutdown.is_stopped());
        shutdown.stop();
        worker.join().expect("core loop thread panicked");

        assert!(shutdown.is_stopped());
        assert!(ticks.load(Ordering::Relaxed) > 0, "loop never ran");
    }

    #[test]
    fn f1a_core_shutdown_is_shared_across_clones() {
        // start() hands each loop a clone of the same signal; stopping one must
        // stop them all.
        let signal = CoreShutdown::default();
        let cloned = signal.clone();
        assert!(!cloned.is_stopped());
        signal.stop();
        assert!(cloned.is_stopped(), "clones must share the shutdown flag");
    }
}
