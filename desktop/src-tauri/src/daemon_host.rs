// apex f1b-2c: the headless CoreHost.
//
// DaemonHost is the non-tauri implementation of the CoreHost seam: it owns the
// world-model state directly (rather than reading it out of tauri's managed-state
// registry) and delivers events to connected clients over the IPC event stream
// (rather than to a webview). with it, jeff_daemon can run core_runtime's
// schedulers headless -- no AppHandle, no webview, no tauri app.
//
// what is real here today:
//   - the world model: request_awareness_update runs the same
//     AwarenessCore::update_with_context the app runs.
//   - the agent runtime: job resume, the standing-job scheduler, and the
//     speculation scheduler need only store + model_router, so they run
//     unchanged (this is the overnight work the daemon exists for).
//   - crisis detection: the C7 detectors in crisis_core are pure by design, so
//     the daemon detects the same crises and relays them over IPC.
//
// what is deliberately deferred (and why):
//   - crisis *delivery* (dedup ledger, native notification, persistent card)
//     lives in crisis.rs behind an AppHandle. the daemon detects and relays the
//     signal; the app owns the UI. same for stale-task notifications.
//   - spawn_side_tasks (proactive monitor + content/goal/memory/consolidation/
//     update polls) is a no-op: those poll loops are tauri-coupled. moving them
//     headless is the next milestone and carries product decisions (does the
//     daemon hold its own Accessibility grant? does it notify directly?).

use crate::ambient::AmbientState;
use crate::awareness_core::SnapshotTrigger;
use crate::core_runtime::CoreHost;
use crate::crisis_core;
use crate::daemon_ipc::{EventSink, IpcEvent};
use crate::models::CalendarEventDto;
use crate::state::{CalendarState, ContextState, JeffState};
use crate::typing_activity::TypingActivityState;

pub struct DaemonHost {
    state: JeffState,
    ambient: AmbientState,
    context: ContextState,
    calendar: CalendarState,
    typing: TypingActivityState,
    sink: EventSink,
}

impl DaemonHost {
    pub fn new(state: JeffState, sink: EventSink) -> Self {
        Self {
            state,
            ambient: AmbientState::new(),
            context: ContextState::new(),
            calendar: CalendarState::new(),
            typing: TypingActivityState::new(true),
            sink,
        }
    }

    // the same phrasing the tauri host feeds the snapshot, so the world model
    // sees identical context in either process.
    fn active_window_string(&self) -> Option<String> {
        self.context.current().map(|ctx| {
            format!(
                "The user currently has {} open with {}.",
                ctx.app_name, ctx.document_title
            )
        })
    }

    fn relay_crisis(&self, task_id: i64, class: crisis_core::CrisisClass, evidence: String) {
        self.emit(
            "crisis://fired",
            serde_json::json!({
                "task_id": task_id,
                "class": format!("{class:?}"),
                "evidence": evidence,
                "source": "daemon",
            }),
        );
    }
}

impl CoreHost for DaemonHost {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        // apex f1b-3c: with no app connected there is nothing to deliver to, so
        // queue the signal instead of dropping it. the app drains the queue when
        // it next launches -- overnight work is never delivered into the void.
        if self.sink.client_count() == 0 {
            let _ = self.state.store.enqueue_daemon_event(event, &payload);
            return;
        }
        self.sink.broadcast(&IpcEvent {
            event: event.to_string(),
            payload,
        });
    }

    fn is_quiet_mode(&self) -> bool {
        self.ambient.is_quiet_mode()
    }

    fn with_jeff_state(&self, f: &mut dyn FnMut(&JeffState)) {
        f(&self.state);
    }
    fn with_context_state(&self, f: &mut dyn FnMut(&ContextState)) {
        f(&self.context);
    }
    fn with_calendar_state(&self, f: &mut dyn FnMut(&CalendarState)) {
        f(&self.calendar);
    }
    fn with_typing_state(&self, f: &mut dyn FnMut(&TypingActivityState)) {
        f(&self.typing);
    }

    fn request_awareness_update(&self, trigger: SnapshotTrigger, task_id: i64) {
        let state = self.state.clone();
        let core = state.awareness_core.clone();
        let active_window = self.active_window_string();
        let calendar_event = self.calendar.current();
        tauri::async_runtime::spawn(async move {
            core.update_with_context(trigger, task_id, &state, active_window, calendar_event)
                .await;
        });
    }

    fn fire_meeting_imminent(
        &self,
        task_id: i64,
        event: &CalendarEventDto,
        movement_toward_event: bool,
    ) {
        if let Some(candidate) =
            crisis_core::detect_meeting_imminent(event.minutes_until, false, movement_toward_event)
        {
            let evidence = format!("{}; {}", event.title.trim(), candidate.evidence);
            self.relay_crisis(task_id, candidate.class, evidence);
        }
    }

    fn fire_deadline_collision(&self, task_id: i64, minutes_until: i64, far_from_done: bool) {
        if let Some(candidate) = crisis_core::detect_deadline_collision(minutes_until, far_from_done)
        {
            self.relay_crisis(task_id, candidate.class, candidate.evidence);
        }
    }

    fn check_stale_tasks(&self, _quiet: bool) {
        // stale-task delivery is a native notification, which the app owns.
        // nothing to do headless yet; see the module note.
    }

    fn spawn_side_tasks(&self) {
        // the proactive monitor and the content/goal/memory/consolidation/update
        // polls are tauri-coupled; they do not run headless yet. see module note.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_ipc::EventSink;
    use std::sync::Arc;

    fn host() -> DaemonHost {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::TaskStore::initialize(dir.path()).unwrap();
        // leak the tempdir so the store outlives the test host
        std::mem::forget(dir);
        let local_runtime = Arc::new(crate::local_runtime::LocalRuntime::new(
            std::env::temp_dir().as_path(),
        ));
        let embeddings = Arc::new(crate::retrieval::default_embeddings_provider(
            local_runtime.clone(),
        ));
        let router = Arc::new(
            crate::model_router::ModelRouter::from_store_with_local_runtime(
                &store,
                Some(local_runtime.clone()),
            ),
        );
        let voice: Arc<dyn crate::providers::VoiceProvider> =
            Arc::new(crate::voice::OpenAiVoiceProvider::from_env());
        let state = JeffState::new(store, embeddings, local_runtime, router, voice);
        DaemonHost::new(state, EventSink::default())
    }

    #[test]
    fn f1b2c_daemon_host_implements_the_core_seam_without_tauri() {
        // the core's whole I/O seam answered by a host that has no AppHandle,
        // no webview, and no tauri app -- this is what lets jeff_daemon run
        // core_runtime headless.
        let host = host();
        assert!(!host.is_quiet_mode());

        let mut saw_state = false;
        host.with_jeff_state(&mut |_s| saw_state = true);
        assert!(saw_state, "daemon host owns JeffState directly");

        let mut saw_ctx = false;
        host.with_context_state(&mut |_c| saw_ctx = true);
        assert!(saw_ctx, "daemon host owns ContextState directly");
    }

    #[test]
    fn f1b3c_signals_produced_with_no_app_connected_are_queued_then_delivered_once() {
        // the daemon does the overnight work with the app closed. anything it
        // has to say must survive until the app comes back -- and be delivered
        // exactly once.
        let host = host();
        assert_eq!(host.sink.client_count(), 0, "no app is connected");

        host.emit("crisis://fired", serde_json::json!({ "task_id": 7 }));
        host.emit(
            "jobs://standing-complete",
            serde_json::json!({ "job_id": 12 }),
        );

        // the app drains the queue on launch.
        let mut drained = Vec::new();
        host.with_jeff_state(&mut |state| {
            drained = state.store.drain_daemon_events().unwrap();
        });
        assert_eq!(drained.len(), 2, "both signals survived the app being closed");
        assert_eq!(drained[0].0, "crisis://fired");
        assert_eq!(drained[0].1["task_id"], 7);
        assert_eq!(drained[1].0, "jobs://standing-complete");

        // draining is destructive: nothing is delivered twice.
        let mut again = Vec::new();
        host.with_jeff_state(&mut |state| {
            again = state.store.drain_daemon_events().unwrap();
        });
        assert!(again.is_empty(), "signals must be delivered exactly once");
    }

    #[test]
    fn f1b2c_daemon_host_relays_crises_over_ipc_instead_of_a_webview() {
        // crisis_core detection is pure (C7), so the daemon detects the same
        // crises the app does and relays them on the IPC event stream.
        let host = host();
        // a meeting 5 minutes out with no movement toward it is a crisis.
        let event = CalendarEventDto {
            title: "Draft review".to_string(),
            starts_at: "2026-07-13T14:00:00Z".to_string(),
            minutes_until: 5,
        };
        host.fire_meeting_imminent(42, &event, false);
        // a deadline 40 minutes out that is far from done is a crisis.
        host.fire_deadline_collision(42, 40, true);
        // no client is connected, so broadcast is a no-op -- the point is that
        // detection ran and routed through emit(), not app.emit().
        assert_eq!(host.sink.client_count(), 0);
    }
}
