use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

use crate::{
    coworking::CoworkingRuntime, embedding::EmbeddingProvider, providers::VoiceProvider,
    reasoning::ReasoningProvider, store::TaskStore, streaming::SharedRegistry,
    subtask::SubTaskRunner, watcher::WatcherState,
};

#[derive(Clone)]
pub struct JeffState {
    pub store: TaskStore,
    pub embeddings: Arc<dyn EmbeddingProvider>,
    pub reasoning: Arc<dyn ReasoningProvider>,
    pub voice: Arc<dyn VoiceProvider>,
    pub interaction_epoch: Arc<AtomicU64>,
    pub coworking: Arc<Mutex<CoworkingRuntime>>,
    pub subtasks: Arc<SubTaskRunner>,
    // phase 12: registry of active streaming turns for cancellation.
    pub interactions: SharedRegistry,
    // phase 13: filesystem watcher state per task.
    pub watcher: Arc<Mutex<WatcherState>>,
}

impl JeffState {
    pub fn new(
        store: TaskStore,
        embeddings: Arc<dyn EmbeddingProvider>,
        reasoning: Arc<dyn ReasoningProvider>,
        voice: Arc<dyn VoiceProvider>,
    ) -> Self {
        let proactive_mode = store
            .get_app_setting_bool("proactive_mode")
            .ok()
            .flatten()
            .unwrap_or(true);
        Self {
            store,
            embeddings,
            reasoning,
            voice,
            interaction_epoch: Arc::new(AtomicU64::new(0)),
            coworking: Arc::new(Mutex::new(CoworkingRuntime::with_proactive_mode(
                proactive_mode,
            ))),
            subtasks: Arc::new(SubTaskRunner::new()),
            interactions: crate::streaming::new_shared_registry(),
            watcher: Arc::new(Mutex::new(WatcherState::new())),
        }
    }

    pub fn next_interaction_epoch(&self) -> u64 {
        self.interaction_epoch.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn current_interaction_epoch(&self) -> u64 {
        self.interaction_epoch.load(Ordering::SeqCst)
    }
}
