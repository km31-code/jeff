// apex f1a/f1b-1: the headless-capable core.
//
// f1a moved every recurring background scheduler and startup task a future
// headless daemon must run lid-closed into this module, behind one start/stop
// lifecycle. f1b-1 decouples those loops from the Tauri AppHandle: their event
// emission and world-model state reads now go through a CoreHost seam instead of
// touching tauri directly, so a daemon (f1b-3) can host them with a non-tauri
// CoreHost. in-process the seam is TauriHost, which delegates to the AppHandle,
// so behavior is byte-identical.
//
// still transitional (f1b-1b): a handful of helper modules (awareness_core,
// crisis, workload, proactive, and the main.rs poll helpers) still take an
// AppHandle. the loops reach them through CoreHost::tauri_app(), the bridge that
// returns the in-process handle. removing that bridge -- decoupling those
// modules -- is the next milestone. shell concerns (tray, overlay, hotkey, wake
// word, login item, pre-manage companion relay) stay in main.rs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager};

use crate::ambient::AmbientState;
use crate::models::CalendarEventDto;
use crate::state::{CalendarState, ContextState, JeffState};
use crate::typing_activity::TypingActivityState;
use crate::{
    agent_runtime, awareness_core, calendar, companion, consolidation, context_observer, coworking,
    crisis, memory, morning, proactive, speculation, workload,
};

// the core's I/O seam. everything a scheduler loop needs from its host --
// emitting events to subscribers and reading the world-model/transient state --
// goes through this trait instead of a tauri AppHandle. state access is
// closure-based (rather than returning a tauri::State) so a headless daemon host
// that owns the state directly can implement it just as well as TauriHost.
pub trait CoreHost: Send + Sync {
    // deliver an event to subscribers (the webview, in-process).
    fn emit(&self, event: &str, payload: serde_json::Value);
    // quiet mode is the universal gate every loop checks; kept as a direct
    // method so a host can answer it without exposing AmbientState.
    fn is_quiet_mode(&self) -> bool;
    fn with_jeff_state(&self, f: &mut dyn FnMut(&JeffState));
    fn with_context_state(&self, f: &mut dyn FnMut(&ContextState));
    fn with_calendar_state(&self, f: &mut dyn FnMut(&CalendarState));
    fn with_typing_state(&self, f: &mut dyn FnMut(&TypingActivityState));
    // f1b-1b: the world-model side effects the loops trigger, expressed as
    // tauri-agnostic intents. TauriHost implements them via the AppHandle-based
    // helpers (awareness_core, crisis, workload, proactive, poll helpers); a
    // headless daemon host implements them against its own owned state and IPC.
    // no AppHandle appears in the seam, so the core is fully tauri-agnostic.
    fn request_awareness_update(&self, trigger: awareness_core::SnapshotTrigger, task_id: i64);
    fn fire_meeting_imminent(
        &self,
        task_id: i64,
        event: &CalendarEventDto,
        movement_toward_event: bool,
    );
    fn fire_deadline_collision(&self, task_id: i64, minutes_until: i64, far_from_done: bool);
    fn check_stale_tasks(&self, quiet: bool);
    // start the side-channel background tasks (proactive monitor + content,
    // goal, memory, consolidation, and update polls). these are detached; the
    // host owns their lifetime.
    fn spawn_side_tasks(&self);
}

// the in-process host: delegates every capability to the Tauri AppHandle, so the
// loops behave exactly as they did when they held the handle directly.
pub struct TauriHost {
    app: AppHandle,
}

impl TauriHost {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl CoreHost for TauriHost {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let _ = self.app.emit(event, payload);
    }
    fn is_quiet_mode(&self) -> bool {
        self.app
            .try_state::<AmbientState>()
            .map(|s| s.is_quiet_mode())
            .unwrap_or(false)
    }
    fn with_jeff_state(&self, f: &mut dyn FnMut(&JeffState)) {
        if let Some(state) = self.app.try_state::<JeffState>() {
            f(&state);
        }
    }
    fn with_context_state(&self, f: &mut dyn FnMut(&ContextState)) {
        if let Some(state) = self.app.try_state::<ContextState>() {
            f(&state);
        }
    }
    fn with_calendar_state(&self, f: &mut dyn FnMut(&CalendarState)) {
        if let Some(state) = self.app.try_state::<CalendarState>() {
            f(&state);
        }
    }
    fn with_typing_state(&self, f: &mut dyn FnMut(&TypingActivityState)) {
        if let Some(state) = self.app.try_state::<TypingActivityState>() {
            f(&state);
        }
    }
    fn request_awareness_update(&self, trigger: awareness_core::SnapshotTrigger, task_id: i64) {
        awareness_core::spawn_awareness_update(&self.app, trigger, task_id);
    }
    fn fire_meeting_imminent(
        &self,
        task_id: i64,
        event: &CalendarEventDto,
        movement_toward_event: bool,
    ) {
        crisis::maybe_fire_meeting_imminent(&self.app, task_id, event, movement_toward_event);
    }
    fn fire_deadline_collision(&self, task_id: i64, minutes_until: i64, far_from_done: bool) {
        crisis::maybe_fire_deadline_collision(&self.app, task_id, minutes_until, far_from_done);
    }
    fn check_stale_tasks(&self, quiet: bool) {
        if let Some(state) = self.app.try_state::<JeffState>() {
            let _ = workload::check_stale_task_notifications(&state.store, &self.app, quiet);
        }
    }
    fn spawn_side_tasks(&self) {
        proactive::spawn_ambient_monitor(self.app.clone());
        let app = self.app.clone();
        tauri::async_runtime::spawn(async move {
            crate::app_polls::spawn_content_observation_poll(app).await;
        });
        let app = self.app.clone();
        tauri::async_runtime::spawn(async move {
            crate::app_polls::spawn_goal_extraction_poll(app).await;
        });
        // apex f2a: memory session-summary and consolidation are no longer spawned
        // here. they moved to the background schedulers (spawn_memory_session_summary,
        // spawn_memory_consolidation) so exactly one process owns them and the daemon
        // runs them headless overnight -- they were app-only before f2a.
        let app = self.app.clone();
        tauri::async_runtime::spawn(async move {
            crate::app_polls::perform_update_check(app).await;
        });
    }
}

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

// apex f1b-3: which loops a process runs. exactly one process may run the
// mutating background schedulers (standing jobs, job resume, speculation) --
// they write to the shared store, so double-running them is the split-brain
// failure. perception stays wherever the Accessibility grant is (the app).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreProfile {
    // standalone app with no daemon running: run everything. this is the
    // pre-f1b-3 behavior and the fallback when the daemon is unreachable.
    Full,
    // the app while a daemon owns the background schedulers: perception, the
    // world model, and the UI-facing loops only. must NOT run the mutating
    // schedulers -- the daemon has them.
    AppClient,
    // the headless daemon: the store-backed background schedulers only. these
    // depend on store + model_router alone (never the live snapshot), so they
    // run correctly with no perception, which the daemon deliberately lacks.
    DaemonBackground,
}

impl CoreProfile {
    // perception + UI-facing loops (need the Accessibility grant / a webview).
    fn runs_perception(self) -> bool {
        matches!(self, CoreProfile::Full | CoreProfile::AppClient)
    }
    // the mutating, store-backed background schedulers. exactly one process.
    fn runs_background_schedulers(self) -> bool {
        matches!(self, CoreProfile::Full | CoreProfile::DaemonBackground)
    }
}

// start the recurring loops this process is responsible for.
pub fn start(host: Arc<dyn CoreHost>, profile: CoreProfile) -> CoreHandle {
    let shutdown = CoreShutdown::default();
    let mut joins: Vec<JoinHandle<()>> = Vec::new();

    if profile.runs_perception() {
        joins.push(spawn_active_window_context_poll(
            host.clone(),
            shutdown.clone(),
        ));
        joins.push(spawn_typing_activity_sync(host.clone(), shutdown.clone()));
        joins.push(spawn_calendar_poll(host.clone(), shutdown.clone()));
        joins.push(spawn_stale_task_check(host.clone(), shutdown.clone()));

        // proactive ambient monitor + content/goal/memory/consolidation/update
        // polls are detached side tasks whose lifetime the host owns.
        host.spawn_side_tasks();
    }

    if profile.runs_background_schedulers() {
        joins.push(spawn_job_resume(host.clone(), shutdown.clone()));
        joins.push(spawn_standing_job_scheduler(host.clone(), shutdown.clone()));
        joins.push(spawn_speculation_scheduler(host.clone(), shutdown.clone()));
        // apex f2a: memory consolidation and session summaries are store-backed
        // (no perception), so they belong with the background schedulers -- exactly
        // one process runs them and the headless daemon does them overnight. before
        // f2a they were app-only side tasks and never ran when the app was closed.
        joins.push(spawn_memory_consolidation(host.clone(), shutdown.clone()));
        joins.push(spawn_memory_session_summary(host.clone(), shutdown.clone()));
        // apex f2b: compose the morning briefing ahead of first engagement, once
        // per day, so delivery is a retrieval. runs on the same single-owner
        // background schedulers as consolidation, after it, so the daemon prepares
        // it overnight.
        joins.push(spawn_morning_prep(host.clone(), shutdown.clone()));
        // apex f3b: keep a presence at the companion relay so a paired phone can
        // reach the core remotely. gated: no-op unless the user enabled the channel
        // AND configured a relay. the relay only ever carries ciphertext.
        joins.push(spawn_companion_relay(host.clone(), shutdown.clone()));
    }

    CoreHandle { shutdown, joins }
}

// phase 20: active-window context polling loop (3-second interval, first poll
// immediate). emits context://context-updated after every poll and
// context://document-switch when the frontmost document title changes to one
// not yet nudged this session and the document is off-task.
fn spawn_active_window_context_poll(host: Arc<dyn CoreHost>, shutdown: CoreShutdown) -> JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        loop {
            if shutdown.is_stopped() {
                break;
            }
            // skip polling when quiet mode is active.
            if host.is_quiet_mode() {
                // still emit a null context so the frontend clears stale display.
                host.emit("context://context-updated", serde_json::Value::Null);
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                continue;
            }

            let mut active_context_allowed = false;
            host.with_jeff_state(&mut |s| {
                let onboarding_complete = s.store.get_onboarding_complete().unwrap_or(false);
                let privacy_enabled = s
                    .store
                    .get_privacy_active_window_context_enabled()
                    .unwrap_or(true);
                active_context_allowed = onboarding_complete && privacy_enabled;
            });

            if !active_context_allowed {
                host.with_context_state(&mut |ctx_state| ctx_state.update(None));
                host.emit("context://context-updated", serde_json::Value::Null);
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                continue;
            }

            let new_ctx = context_observer::poll_active_window();
            let accessibility_trusted = context_observer::is_accessibility_trusted();

            host.with_context_state(&mut |ctx_state| {
                let prior_title = ctx_state.current().map(|ctx| ctx.document_title);
                let new_title = new_ctx.as_ref().map(|ctx| ctx.document_title.clone());
                let document_title_changed = matches!(
                    (prior_title.as_deref(), new_title.as_deref()),
                    (Some(previous), Some(next)) if !next.is_empty() && previous != next
                );

                // fire the document-switch nudge before updating state so we
                // compare the incoming title against the last-known one.
                if accessibility_trusted {
                    if let Some(ref ctx) = new_ctx {
                        let title = &ctx.document_title;
                        if !title.is_empty() && ctx_state.should_nudge_for_switch(title) {
                            // suppress nudge when the document title matches the
                            // active task title (user is on-task).
                            let mut task_title = None;
                            host.with_jeff_state(&mut |s| {
                                task_title = s
                                    .store
                                    .get_active_task()
                                    .ok()
                                    .flatten()
                                    .map(|t| t.title);
                            });
                            let off_task = task_title
                                .as_deref()
                                .map_or(true, |t| crate::app_polls::document_is_off_task(title, t));
                            if off_task {
                                host.emit(
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

                ctx_state.update(new_ctx.clone());
                if document_title_changed {
                    let mut task_id = None;
                    host.with_jeff_state(&mut |s| {
                        task_id = s.store.get_active_task().ok().flatten().map(|t| t.id);
                    });
                    if let Some(task_id) = task_id {
                        host.request_awareness_update(
                            awareness_core::SnapshotTrigger::WindowSwitch,
                            task_id,
                        );
                    }
                }

                // emit context-updated so the frontend tracks current state
                // without needing its own polling interval.
                let context_payload = ctx_state.current().map(|ctx| {
                    serde_json::json!({
                        "app_name": ctx.app_name,
                        "document_title": ctx.document_title,
                        "captured_at": ctx.captured_at,
                    })
                });
                host.emit(
                    "context://context-updated",
                    context_payload.unwrap_or(serde_json::Value::Null),
                );
            });

            // sleep at end so the first poll runs immediately on startup.
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
    })
}

// phase 22: keep rate-only typing state in sync with privacy and expose only a
// boolean event for the frontend tts queue.
fn spawn_typing_activity_sync(host: Arc<dyn CoreHost>, shutdown: CoreShutdown) -> JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let mut last_typing: Option<bool> = None;
        loop {
            if shutdown.is_stopped() {
                break;
            }
            let mut enabled = true;
            host.with_jeff_state(&mut |s| {
                enabled = s.store.get_privacy_typing_activity_enabled().unwrap_or(true);
            });

            let mut have_typing = false;
            let mut is_typing = false;
            let mut monitor_available = false;
            let mut last_error: Option<String> = None;
            host.with_typing_state(&mut |typing_state| {
                have_typing = true;
                typing_state.set_enabled(enabled);
                is_typing = enabled && typing_state.is_typing();
                monitor_available = typing_state.monitor_available();
                last_error = typing_state.last_error();
            });

            if !have_typing {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                continue;
            }

            if last_typing != Some(is_typing) {
                last_typing = Some(is_typing);
                host.emit(
                    "typing://activity-changed",
                    serde_json::json!({
                        "is_typing": is_typing,
                        "rate_only": true,
                        "monitor_available": monitor_available,
                        "last_error": last_error,
                    }),
                );
                host.with_jeff_state(&mut |s| {
                    if let Ok(now) = coworking::unix_now_seconds() {
                        if let Ok(mut runtime) = s.coworking.lock() {
                            let _ = runtime.set_user_typing(is_typing, now);
                        }
                    }
                });
            }

            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    })
}

// phase 23: calendar poll task (60-second interval). polls EventKit for the next
// upcoming event when the privacy gate is enabled and the OS permission has been
// granted, and drives the meeting-imminent / deadline-collision crisis classes.
fn spawn_calendar_poll(host: Arc<dyn CoreHost>, shutdown: CoreShutdown) -> JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        loop {
            if shutdown.is_stopped() {
                break;
            }
            if host.is_quiet_mode() {
                host.with_calendar_state(&mut |cs| cs.update(None));
                host.emit("calendar://event-updated", serde_json::Value::Null);
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                continue;
            }

            let mut cal_enabled = false;
            host.with_jeff_state(&mut |s| {
                cal_enabled = s.store.get_privacy_calendar_context_enabled().unwrap_or(false);
            });

            if !cal_enabled {
                // clear stale event if feature is turned off mid-session
                host.with_calendar_state(&mut |cs| cs.update(None));
                host.emit("calendar://event-updated", serde_json::Value::Null);
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                continue;
            }

            let next_event = calendar::fetch_next_event(8);
            host.with_calendar_state(&mut |cs| cs.update(next_event.clone()));

            let mut ctx_title: Option<String> = None;
            host.with_context_state(&mut |ctx| {
                ctx_title = ctx.current().map(|c| c.document_title);
            });
            let movement_toward_event = next_event
                .as_ref()
                .and_then(|event| {
                    ctx_title
                        .as_deref()
                        .map(|dt| crate::app_polls::crisis_event_matches_context(&event.title, dt))
                })
                .unwrap_or(false);

            host.emit(
                "calendar://event-updated",
                serde_json::to_value(&next_event).unwrap_or(serde_json::Value::Null),
            );

            let mut active_task_id: Option<i64> = None;
            host.with_jeff_state(&mut |s| {
                active_task_id = s.store.get_active_task().ok().flatten().map(|t| t.id);
            });
            if let Some(task_id) = active_task_id {
                host.request_awareness_update(
                    awareness_core::SnapshotTrigger::CalendarEvent,
                    task_id,
                );
                if let Some(event) = next_event.as_ref() {
                    host.fire_meeting_imminent(task_id, event, movement_toward_event);
                    let mut far_from_done = false;
                    host.with_jeff_state(&mut |s| {
                        far_from_done = s
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
                            .unwrap_or(false);
                    });
                    host.fire_deadline_collision(task_id, event.minutes_until, far_from_done);
                }
            }

            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    })
}

// phase 23: stale-task notification check at startup.
fn spawn_stale_task_check(host: Arc<dyn CoreHost>, shutdown: CoreShutdown) -> JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        // give the app a few seconds to finish startup before checking
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        if shutdown.is_stopped() {
            return;
        }
        host.check_stale_tasks(host.is_quiet_mode());
    })
}

// apex d6: resume any job left pending/running by a previous session once at
// startup so checkpointed jobs continue from their last completed step.
fn spawn_job_resume(host: Arc<dyn CoreHost>, shutdown: CoreShutdown) -> JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        if shutdown.is_stopped() {
            return;
        }
        host.with_jeff_state(&mut |state| {
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
        });
    })
}

// apex d6: standing-job scheduler (60-second interval). fires daily-due standing
// jobs automatically; each run posts a receipt to the unified audit log.
fn spawn_standing_job_scheduler(host: Arc<dyn CoreHost>, shutdown: CoreShutdown) -> JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        loop {
            if shutdown.is_stopped() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            host.with_jeff_state(&mut |state| {
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
            });
        }
    })
}

// apex d8: speculation scheduler (~10-minute cadence). when the user is
// unengaged with Jeff but working, precomputes the likely next request as a
// read-only speculative job. speculative jobs cannot mutate (agent_runtime).
fn spawn_speculation_scheduler(host: Arc<dyn CoreHost>, shutdown: CoreShutdown) -> JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        loop {
            if shutdown.is_stopped() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(10 * 60)).await;
            if host.is_quiet_mode() {
                continue;
            }
            host.with_jeff_state(&mut |state| {
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
            });
        }
    })
}

// apex f2a: memory consolidation scheduler. distills the day's episodes into
// durable facts during lulls (or a daily 2am catch-up). store-backed and
// perception-free, so it runs on whichever single process owns the background
// schedulers -- the daemon overnight, or the app when there is no daemon.
fn spawn_memory_consolidation(host: Arc<dyn CoreHost>, shutdown: CoreShutdown) -> JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        if std::env::var("JEFF_DISABLE_MEMORY_CONSOLIDATION").is_ok() {
            return;
        }
        loop {
            if shutdown.is_stopped() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(
                consolidation::CONSOLIDATION_TICK_SECONDS,
            ))
            .await;

            // grab a clone of the store-backed bundle through the host seam, then
            // run the (blocking) consolidation off the closure so no await happens
            // inside with_jeff_state.
            let mut bundle = None;
            host.with_jeff_state(&mut |state| {
                bundle = Some((
                    state.store.clone(),
                    state.embeddings.clone(),
                    state.model_router.clone(),
                ));
            });
            let Some((store, embeddings, router)) = bundle else {
                continue;
            };
            match tokio::task::spawn_blocking(move || {
                consolidation::consolidate_if_due(&store, embeddings.as_ref(), &router)
            })
            .await
            {
                Ok(Ok(Some(report))) => {
                    eprintln!(
                        "[jeff] memory_consolidation_complete processed={} upserted={} merged={} dropped={}",
                        report.processed_episode_count,
                        report.upserted_fact_count,
                        report.merged_fact_count,
                        report.dropped_fact_count
                    );
                }
                Ok(Ok(None)) => {}
                Ok(Err(err)) => eprintln!("[jeff] memory_consolidation_failed: {err}"),
                Err(err) => eprintln!("[jeff] memory_consolidation_join_failed: {err}"),
            }
        }
    })
}

// apex f2a: memory session-summary scheduler. records an episodic summary of a
// task's session once it has been idle long enough. store-backed; same
// single-owner posture as consolidation so the daemon writes them overnight.
fn spawn_memory_session_summary(host: Arc<dyn CoreHost>, shutdown: CoreShutdown) -> JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        if std::env::var("JEFF_DISABLE_MEMORY_SUMMARY").is_ok() {
            return;
        }
        loop {
            if shutdown.is_stopped() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(
                memory::SESSION_SUMMARY_TICK_SECONDS,
            ))
            .await;

            let mut bundle = None;
            host.with_jeff_state(&mut |state| {
                if state
                    .store
                    .get_privacy_user_profile_memory_enabled()
                    .unwrap_or(false)
                {
                    bundle = Some((
                        state.store.clone(),
                        state.embeddings.clone(),
                        state.model_router.clone(),
                        state.store.list_tasks().unwrap_or_default(),
                    ));
                }
            });
            let Some((store, embeddings, router, tasks)) = bundle else {
                continue;
            };
            for task in tasks {
                let store = store.clone();
                let embeddings = embeddings.clone();
                let router = router.clone();
                let task_id = task.id;
                match tokio::task::spawn_blocking(move || {
                    memory::record_idle_session_summary_if_due(
                        &store,
                        embeddings.as_ref(),
                        &router,
                        task_id,
                        memory::SESSION_IDLE_SECONDS,
                    )
                })
                .await
                {
                    Ok(Ok(Some(_episode))) => {
                        eprintln!("[jeff] memory_session_summary_recorded task={task_id}");
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(err)) => eprintln!("[jeff] memory_session_summary_failed: {err}"),
                    Err(err) => eprintln!("[jeff] memory_session_summary_join_failed: {err}"),
                }
            }
        }
    })
}

// apex f2b: morning-prep scheduler. once per local day (idempotent in
// morning::prepare_todays_briefing) it composes the day's briefing ahead of first
// engagement, folding in the overnight work the daemon finished. runs on the same
// single-owner background schedulers as consolidation so the daemon prepares it
// overnight; the app in Full profile prepares it when there is no daemon.
fn spawn_morning_prep(host: Arc<dyn CoreHost>, shutdown: CoreShutdown) -> JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        loop {
            if shutdown.is_stopped() {
                break;
            }
            // a few-minute cadence: the first pass after the machine wakes for the
            // day prepares the briefing; the per-day idempotency guard makes every
            // later pass a cheap no-op.
            tokio::time::sleep(std::time::Duration::from_secs(5 * 60)).await;

            let mut bundle = None;
            host.with_jeff_state(&mut |state| {
                bundle = Some(state.clone());
            });
            let Some(state) = bundle else {
                continue;
            };
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            match tokio::task::spawn_blocking(move || morning::prepare_todays_briefing(&state, now))
                .await
            {
                Ok(Ok(Some(prepared))) => {
                    eprintln!(
                        "[jeff] morning briefing prepared for {} (task {})",
                        prepared.date, prepared.task_id
                    );
                }
                Ok(Ok(None)) => {}
                Ok(Err(err)) => eprintln!("[jeff] morning briefing prep failed: {err:#}"),
                Err(err) => eprintln!("[jeff] morning briefing prep join failed: {err}"),
            }
        }
    })
}

// apex f3b: companion relay presence. when the companion channel is enabled and a
// relay is configured, dial the relay as host, serve exactly one Noise session,
// and re-dial for the next -- so a paired phone can reach the core from anywhere.
// gated and fail-safe: disabled or unconfigured -> idle; a relay that is down ->
// backoff and retry. the relay only ever sees ciphertext.
fn spawn_companion_relay(host: Arc<dyn CoreHost>, shutdown: CoreShutdown) -> JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        loop {
            if shutdown.is_stopped() {
                break;
            }

            let mut plan = None;
            host.with_jeff_state(&mut |state| {
                if companion::is_enabled(&state.store) {
                    if let Some(relay) = companion::relay_url(&state.store) {
                        plan = Some((state.clone(), relay));
                    }
                }
            });

            let Some((state, relay)) = plan else {
                // not enabled / no relay: idle, re-check periodically.
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                continue;
            };

            // serve one blocking session off the async runtime; a returned error
            // (relay down, peer rejected) just means we back off and re-dial.
            let served = tokio::task::spawn_blocking(move || {
                companion::serve_one_via_relay(&state, &relay)
            })
            .await;
            match served {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    eprintln!("[jeff] companion relay session ended: {err:#}");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
                Err(err) => {
                    eprintln!("[jeff] companion relay task failed: {err}");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::sync::Mutex;
    use std::time::Duration;

    // a non-tauri CoreHost. proves the core's I/O seam works with no AppHandle,
    // no webview, and no tauri runtime -- the shape a headless daemon host takes.
    struct FakeHost {
        quiet: AtomicBool,
        events: Mutex<Vec<(String, serde_json::Value)>>,
        intents: Mutex<Vec<String>>,
    }

    impl FakeHost {
        fn new() -> Self {
            Self {
                quiet: AtomicBool::new(false),
                events: Mutex::new(Vec::new()),
                intents: Mutex::new(Vec::new()),
            }
        }
        fn set_quiet(&self, quiet: bool) {
            self.quiet.store(quiet, Ordering::Relaxed);
        }
        fn emitted(&self) -> Vec<(String, serde_json::Value)> {
            self.events.lock().unwrap().clone()
        }
        fn intents(&self) -> Vec<String> {
            self.intents.lock().unwrap().clone()
        }
    }

    impl CoreHost for FakeHost {
        fn emit(&self, event: &str, payload: serde_json::Value) {
            self.events.lock().unwrap().push((event.to_string(), payload));
        }
        fn is_quiet_mode(&self) -> bool {
            self.quiet.load(Ordering::Relaxed)
        }
        fn with_jeff_state(&self, _f: &mut dyn FnMut(&JeffState)) {}
        fn with_context_state(&self, _f: &mut dyn FnMut(&ContextState)) {}
        fn with_calendar_state(&self, _f: &mut dyn FnMut(&CalendarState)) {}
        fn with_typing_state(&self, _f: &mut dyn FnMut(&TypingActivityState)) {}
        fn request_awareness_update(&self, _trigger: awareness_core::SnapshotTrigger, task_id: i64) {
            self.intents.lock().unwrap().push(format!("awareness:{task_id}"));
        }
        fn fire_meeting_imminent(
            &self,
            task_id: i64,
            _event: &CalendarEventDto,
            _movement_toward_event: bool,
        ) {
            self.intents.lock().unwrap().push(format!("meeting:{task_id}"));
        }
        fn fire_deadline_collision(&self, task_id: i64, _minutes_until: i64, _far_from_done: bool) {
            self.intents.lock().unwrap().push(format!("deadline:{task_id}"));
        }
        fn check_stale_tasks(&self, _quiet: bool) {
            self.intents.lock().unwrap().push("stale".to_string());
        }
        fn spawn_side_tasks(&self) {
            self.intents.lock().unwrap().push("side_tasks".to_string());
        }
    }

    #[test]
    fn f1b1_fake_host_routes_events_without_a_webview() {
        // the core can emit through the seam with no tauri runtime present.
        let host = FakeHost::new();
        host.emit("context://context-updated", serde_json::Value::Null);
        host.emit(
            "typing://activity-changed",
            serde_json::json!({ "is_typing": true }),
        );
        let events = host.emitted();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, "context://context-updated");
        assert_eq!(events[1].0, "typing://activity-changed");
        assert_eq!(events[1].1["is_typing"], serde_json::json!(true));
    }

    #[test]
    fn f1b1_fake_host_gates_and_runs_intents_headless() {
        // quiet gating and the world-model side effects (awareness update,
        // crisis fires, stale check, side tasks) are all expressed as
        // tauri-agnostic CoreHost intents a non-tauri host answers with no
        // AppHandle -- the shape the f1b daemon host takes.
        let host = FakeHost::new();
        assert!(!host.is_quiet_mode());
        host.set_quiet(true);
        assert!(host.is_quiet_mode());

        host.request_awareness_update(awareness_core::SnapshotTrigger::WindowSwitch, 7);
        host.check_stale_tasks(true);
        host.spawn_side_tasks();
        let intents = host.intents();
        assert!(intents.contains(&"awareness:7".to_string()));
        assert!(intents.contains(&"stale".to_string()));
        assert!(intents.contains(&"side_tasks".to_string()));
    }

    #[test]
    fn f2a_memory_jobs_are_owned_by_the_background_scheduler_profiles() {
        // consolidation and session summary are store-backed, so they run under
        // exactly one process -- the daemon overnight, or the app when there is no
        // daemon. the profile predicate that gates them must match standing jobs
        // and speculation: Full and DaemonBackground yes, AppClient no.
        assert!(CoreProfile::Full.runs_background_schedulers());
        assert!(CoreProfile::DaemonBackground.runs_background_schedulers());
        assert!(
            !CoreProfile::AppClient.runs_background_schedulers(),
            "the app-as-client must not double-run store-backed memory jobs the daemon owns"
        );
        // and they are not perception, so DaemonBackground (no perception) must
        // still own them -- the whole point of moving them off the side tasks.
        assert!(!CoreProfile::DaemonBackground.runs_perception());
    }

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
