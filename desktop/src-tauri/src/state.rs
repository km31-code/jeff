use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use crate::context_observer::{ActiveWindowContext, ContentObservationState};

use crate::{
    awareness_core::AwarenessCore,
    coworking::CoworkingRuntime,
    embedding::EmbeddingProvider,
    model_router::{ModelRouter, Tier},
    providers::VoiceProvider,
    reasoning::ReasoningProvider,
    store::TaskStore,
    streaming::SharedRegistry,
    subtask::SubTaskRunner,
    watcher::WatcherState,
};

#[derive(Clone)]
pub struct JeffState {
    pub store: TaskStore,
    pub embeddings: Arc<dyn EmbeddingProvider>,
    // apex a1: the model router owns tier→model resolution for every llm call.
    pub model_router: Arc<ModelRouter>,
    // conversation-tier handle kept under the legacy field name so existing
    // call sites and their tests keep compiling; craft/judgment call sites
    // use the explicit tier helpers below.
    pub reasoning: Arc<dyn ReasoningProvider>,
    pub voice: Arc<dyn VoiceProvider>,
    pub interaction_epoch: Arc<AtomicU64>,
    pub coworking: Arc<Mutex<CoworkingRuntime>>,
    pub subtasks: Arc<SubTaskRunner>,
    // phase 12: registry of active streaming turns for cancellation.
    pub interactions: SharedRegistry,
    // phase 13: filesystem watcher state per task.
    pub watcher: Arc<Mutex<WatcherState>>,
    pub awareness_core: Arc<AwarenessCore>,
    // phase 31: content observation state — raw text never leaves context_observer.
    pub content_observation: Arc<Mutex<ContentObservationState>>,
}

impl JeffState {
    pub fn new(
        store: TaskStore,
        embeddings: Arc<dyn EmbeddingProvider>,
        model_router: Arc<ModelRouter>,
        voice: Arc<dyn VoiceProvider>,
    ) -> Self {
        let proactive_mode = store
            .get_app_setting_bool("proactive_mode")
            .ok()
            .flatten()
            .unwrap_or(true);
        let reasoning = model_router.handle(Tier::Conversation);
        Self {
            store,
            embeddings,
            model_router,
            reasoning,
            voice,
            interaction_epoch: Arc::new(AtomicU64::new(0)),
            coworking: Arc::new(Mutex::new(CoworkingRuntime::with_proactive_mode(
                proactive_mode,
            ))),
            subtasks: Arc::new(SubTaskRunner::new()),
            interactions: crate::streaming::new_shared_registry(),
            watcher: Arc::new(Mutex::new(WatcherState::new())),
            awareness_core: Arc::new(AwarenessCore::new()),
            content_observation: Arc::new(Mutex::new(ContentObservationState::default())),
        }
    }

    // apex a1: explicit tier handles. craft = drafting/revision/subtask work;
    // judgment = proactive synthesis, reorientation, drift evaluation.
    pub fn craft_reasoning(&self) -> Arc<dyn ReasoningProvider> {
        self.model_router.handle(Tier::Craft)
    }

    pub fn judgment_reasoning(&self) -> Arc<dyn ReasoningProvider> {
        self.model_router.handle(Tier::Judgment)
    }

    pub fn next_interaction_epoch(&self) -> u64 {
        self.interaction_epoch.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn current_interaction_epoch(&self) -> u64 {
        self.interaction_epoch.load(Ordering::SeqCst)
    }
}

// ---- phase 20: active window context state ----------------------------------

// hard cap on the number of document titles remembered as nudged per session.
// prevents unbounded growth during very long sessions with many document switches.
const MAX_NUDGED_TITLES: usize = 200;

struct ContextStateInner {
    current: Option<ActiveWindowContext>,
    // tracks every document title nudged in this session so returning to the
    // same off-task document does not repeat the prompt.
    nudged_titles: HashSet<String>,
}

pub struct ContextState {
    inner: Mutex<ContextStateInner>,
}

impl ContextState {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(ContextStateInner {
                current: None,
                nudged_titles: HashSet::new(),
            }),
        }
    }

    pub fn update(&self, ctx: Option<ActiveWindowContext>) {
        if let Ok(mut g) = self.inner.lock() {
            g.current = ctx;
        }
    }

    pub fn current(&self) -> Option<ActiveWindowContext> {
        self.inner.lock().ok()?.current.clone()
    }

    // returns true if we have not yet emitted a nudge for this document title.
    #[cfg(test)]
    pub fn should_nudge(&self, title: &str) -> bool {
        let normalized = normalize_document_title(title);
        if normalized.is_empty() {
            return false;
        }
        self.inner
            .lock()
            .ok()
            .map_or(false, |g| !g.nudged_titles.contains(&normalized))
    }

    // returns true only for a real switch from the previous observed document
    // to a not-yet-nudged document. first observation is context, not a switch.
    pub fn should_nudge_for_switch(&self, title: &str) -> bool {
        let normalized = normalize_document_title(title);
        if normalized.is_empty() {
            return false;
        }
        self.inner.lock().ok().map_or(false, |g| {
            let previous = g
                .current
                .as_ref()
                .map(|ctx| normalize_document_title(&ctx.document_title))
                .filter(|value| !value.is_empty());
            matches!(previous, Some(prev) if prev != normalized)
                && !g.nudged_titles.contains(&normalized)
        })
    }

    pub fn mark_nudged(&self, title: String) {
        let normalized = normalize_document_title(&title);
        if normalized.is_empty() {
            return;
        }
        if let Ok(mut g) = self.inner.lock() {
            // evict an arbitrary entry when the set reaches the cap to prevent
            // unbounded growth during very long sessions (many document switches).
            if g.nudged_titles.len() >= MAX_NUDGED_TITLES {
                if let Some(evicted) = g.nudged_titles.iter().next().cloned() {
                    g.nudged_titles.remove(&evicted);
                }
            }
            g.nudged_titles.insert(normalized);
        }
    }
}

fn normalize_document_title(title: &str) -> String {
    title.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod context_state_tests {
    use super::*;

    #[test]
    fn context_state_should_nudge() {
        let cs = ContextState::new();
        // no nudge recorded yet → should nudge
        assert!(cs.should_nudge("my-doc.md"));
        cs.mark_nudged("my-doc.md".to_string());
        // same title → should not nudge again
        assert!(!cs.should_nudge("my-doc.md"));
        // different title → should nudge
        assert!(cs.should_nudge("other-doc.md"));
        cs.mark_nudged("other-doc.md".to_string());
        assert!(!cs.should_nudge("other-doc.md"));
        // returning to a previously nudged title still stays suppressed.
        assert!(!cs.should_nudge("my-doc.md"));
    }

    #[test]
    fn context_state_update_and_current() {
        use crate::context_observer::ActiveWindowContext;
        let cs = ContextState::new();
        assert!(cs.current().is_none());
        cs.update(Some(ActiveWindowContext {
            app_name: "Xcode".to_string(),
            document_title: "main.swift".to_string(),
            captured_at: 0,
        }));
        let ctx = cs.current().unwrap();
        assert_eq!(ctx.app_name, "Xcode");
        assert_eq!(ctx.document_title, "main.swift");
        cs.update(None);
        assert!(cs.current().is_none());
    }

    #[test]
    fn context_state_nudges_only_on_real_unseen_switches() {
        use crate::context_observer::ActiveWindowContext;
        let cs = ContextState::new();

        assert!(!cs.should_nudge_for_switch("notes.md"));
        cs.update(Some(ActiveWindowContext {
            app_name: "TextEdit".to_string(),
            document_title: "notes.md".to_string(),
            captured_at: 0,
        }));
        assert!(!cs.should_nudge_for_switch("notes.md"));
        assert!(cs.should_nudge_for_switch("outline.md"));
        cs.mark_nudged("outline.md".to_string());
        cs.update(Some(ActiveWindowContext {
            app_name: "TextEdit".to_string(),
            document_title: "outline.md".to_string(),
            captured_at: 0,
        }));
        cs.update(Some(ActiveWindowContext {
            app_name: "TextEdit".to_string(),
            document_title: "notes.md".to_string(),
            captured_at: 0,
        }));
        assert!(!cs.should_nudge_for_switch("outline.md"));
    }
}

// ---- phase 23: calendar state -----------------------------------------------

use std::time::Instant;

use crate::models::CalendarEventDto;

pub struct CalendarState {
    inner: std::sync::Mutex<CalendarStateInner>,
}

struct CalendarStateInner {
    pub next_event: Option<CalendarEventDto>,
    pub last_polled: Option<Instant>,
}

impl CalendarState {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(CalendarStateInner {
                next_event: None,
                last_polled: None,
            }),
        }
    }

    pub fn update(&self, event: Option<CalendarEventDto>) {
        if let Ok(mut g) = self.inner.lock() {
            g.next_event = event;
            g.last_polled = Some(Instant::now());
        }
    }

    pub fn current(&self) -> Option<CalendarEventDto> {
        self.inner.lock().ok()?.next_event.clone()
    }
}
