// apex b7: WorkUnderstanding comprehension pass.
//
// This is the one Apex path allowed to send raw observed document text to a
// model. Callers must enter only from the content-observation opt-in path. The
// output is structured, stored as a typed memory episode, and mirrored into the
// situational snapshot through an app_setting containing only the JSON result.

#![cfg_attr(test, allow(dead_code))]

use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};

#[cfg(not(test))]
use crate::model_router::{GenerateOptions, ModelRequest, Tier};
use crate::{
    ambient::AmbientState,
    cost_governor::{self, WORK_UNDERSTANDING_BUDGET_KEY},
    document_model::DocumentStateSummary,
    embedding::EmbeddingProvider,
    memory::{self, NewEpisode},
    model_router::{ModelRouter, ProviderKind},
    models::EpisodeDto,
    relational_model,
    state::JeffState,
    store::{LlmUsageLogInput, TaskStore},
};

pub const WORK_UNDERSTANDING_INTERVAL_SECONDS: i64 = 5 * 60;
pub const WORK_UNDERSTANDING_TIMEOUT_MS: u64 = 12_000;
pub const WORK_UNDERSTANDING_MAX_DOC_CHARS: usize = 12_000;
pub const WORK_UNDERSTANDING_LAST_RUN_KEY_PREFIX: &str = "work_understanding:last_run:";
pub const WORK_UNDERSTANDING_LATEST_KEY_PREFIX: &str = "work_understanding:latest:";

pub const WORK_UNDERSTANDING_SYSTEM_PROMPT: &str = "You are Jeff's WorkUnderstanding pass. \
Read the user's current work and return strict JSON only: \
{\"argument_summary\":\"plain summary\",\"weak_points\":[\"specific weakness with section/reason\"],\"stuck_signal\":null,\"candidate_observation\":null}. \
Do not flatter. Ground every weak point in the supplied document text, outline, churn, goal, or prior understanding.";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkUnderstanding {
    pub argument_summary: String,
    #[serde(default)]
    pub weak_points: Vec<String>,
    #[serde(default)]
    pub stuck_signal: Option<String>,
    #[serde(default)]
    pub candidate_observation: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WorkUnderstandingInput {
    pub task_id: i64,
    pub document_text: String,
    pub document_outline: Vec<String>,
    pub document_summary: Option<DocumentStateSummary>,
    pub current_goal: Option<String>,
    pub prior: Option<WorkUnderstanding>,
}

pub fn maybe_spawn_work_understanding<R: Runtime + 'static>(
    app: &AppHandle<R>,
    task_id: i64,
    document_text: String,
) {
    if document_text.trim().is_empty() {
        return;
    }
    if app
        .try_state::<AmbientState>()
        .map(|ambient| ambient.is_quiet_mode())
        .unwrap_or(false)
    {
        return;
    }
    let Some(jeff_state) = app.try_state::<JeffState>() else {
        return;
    };
    if !jeff_state
        .store
        .get_content_observation_enabled(task_id)
        .unwrap_or(false)
    {
        return;
    }
    let now = unix_now();
    if !should_run_work_understanding(&jeff_state.store, task_id, true, now).unwrap_or(false) {
        return;
    }
    if mark_work_understanding_ran(&jeff_state.store, task_id, now).is_err() {
        return;
    }

    let state = jeff_state.inner().clone();
    let app_handle = app.clone();
    thread::spawn(move || {
        let (document_outline, document_summary) = state
            .document_model
            .lock()
            .map(|dm| (dm.outline(task_id), dm.state(task_id)))
            .unwrap_or_default();
        let input = WorkUnderstandingInput {
            task_id,
            document_text,
            document_outline,
            document_summary,
            current_goal: relational_model::latest_active_goal_text(&state.store, task_id),
            prior: latest_from_store(&state.store, task_id).ok().flatten(),
        };
        if run_work_understanding_pass(
            &state.store,
            state.embeddings.as_ref(),
            &state.model_router,
            input,
        )
        .is_ok()
        {
            crate::awareness_core::spawn_awareness_update(
                &app_handle,
                crate::awareness_core::SnapshotTrigger::ContentObservation,
                task_id,
            );
        }
    });
}

pub fn should_run_work_understanding(
    store: &TaskStore,
    task_id: i64,
    content_changed: bool,
    now: i64,
) -> Result<bool> {
    if !content_changed {
        return Ok(false);
    }
    let key = last_run_key(task_id);
    let last = store
        .get_app_setting(&key)?
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .unwrap_or(0);
    Ok(now.saturating_sub(last) >= WORK_UNDERSTANDING_INTERVAL_SECONDS)
}

pub fn mark_work_understanding_ran(store: &TaskStore, task_id: i64, now: i64) -> Result<()> {
    store.set_app_setting(&last_run_key(task_id), &now.to_string())
}

pub fn run_work_understanding_pass(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    router: &ModelRouter,
    input: WorkUnderstandingInput,
) -> Result<WorkUnderstanding> {
    let understanding = match run_work_understanding_model(router, &input) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("[jeff] work_understanding_fallback reason={err}");
            record_fallback_budget_touch(store)?;
            deterministic_work_understanding(&input)
        }
    };
    persist_work_understanding(store, embeddings, input.task_id, &understanding)?;
    Ok(understanding)
}

pub fn latest_from_store(store: &TaskStore, task_id: i64) -> Result<Option<WorkUnderstanding>> {
    let Some(raw) = store.get_app_setting(&latest_key(task_id))? else {
        return Ok(None);
    };
    Ok(serde_json::from_str(&raw).ok())
}

#[cfg(test)]
fn run_work_understanding_model(
    _router: &ModelRouter,
    _input: &WorkUnderstandingInput,
) -> Result<WorkUnderstanding> {
    Err(anyhow!("test fallback"))
}

#[cfg(not(test))]
fn run_work_understanding_model(
    router: &ModelRouter,
    input: &WorkUnderstandingInput,
) -> Result<WorkUnderstanding> {
    let mut request = ModelRequest::new(
        Tier::Judgment,
        WORK_UNDERSTANDING_SYSTEM_PROMPT,
        build_work_understanding_prompt(input),
    )
    .with_options(GenerateOptions {
        temperature: 0.0,
        max_tokens: Some(550),
        json_object: true,
        timeout_ms: Some(WORK_UNDERSTANDING_TIMEOUT_MS),
    })
    .with_budget_key(WORK_UNDERSTANDING_BUDGET_KEY);
    request.purpose = Some("work_understanding".to_string());
    let raw = router.route(request)?.text;
    parse_work_understanding_json(&raw)
}

pub fn build_work_understanding_prompt(input: &WorkUnderstandingInput) -> String {
    let outline = if input.document_outline.is_empty() {
        "<none>".to_string()
    } else {
        input
            .document_outline
            .iter()
            .take(24)
            .enumerate()
            .map(|(index, line)| format!("{}. {}", index + 1, line))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let summary = input
        .document_summary
        .as_ref()
        .map(|summary| {
            format!(
                "paragraphs={} words={} structure_changed={} added_last={} removed_last={} rewritten_last={} max_churn={} churn_hotspots={}",
                summary.paragraph_count,
                summary.word_count,
                summary.structure_changed,
                summary.added_last,
                summary.removed_last,
                summary.rewritten_last,
                summary.max_churn,
                summary.churn_hotspot_count
            )
        })
        .unwrap_or_else(|| "<none>".to_string());
    let prior = input
        .prior
        .as_ref()
        .map(|prior| {
            format!(
                "summary={}\nweak_points={}",
                prior.argument_summary,
                prior.weak_points.join("; ")
            )
        })
        .unwrap_or_else(|| "<none>".to_string());
    let doc = truncate_chars(&input.document_text, WORK_UNDERSTANDING_MAX_DOC_CHARS);
    format!(
        "Current goal:\n{}\n\nDocument outline:\n{}\n\nRecent deltas and churn:\n{}\n\nPrior WorkUnderstanding:\n{}\n\nDocument text from content observation opt-in:\n{}",
        input.current_goal.as_deref().unwrap_or("<none>"),
        outline,
        summary,
        prior,
        doc
    )
}

pub fn parse_work_understanding_json(raw: &str) -> Result<WorkUnderstanding> {
    let json = extract_json_object(raw).ok_or_else(|| anyhow!("no JSON object in response"))?;
    let parsed: WorkUnderstanding =
        serde_json::from_str(json).context("failed to parse WorkUnderstanding JSON")?;
    if parsed.argument_summary.trim().is_empty() {
        return Err(anyhow!(
            "WorkUnderstanding argument_summary cannot be empty"
        ));
    }
    Ok(WorkUnderstanding {
        argument_summary: clean_line(&parsed.argument_summary),
        weak_points: parsed
            .weak_points
            .into_iter()
            .map(|point| clean_line(&point))
            .filter(|point| !point.is_empty())
            .take(8)
            .collect(),
        stuck_signal: parsed
            .stuck_signal
            .map(|value| clean_line(&value))
            .filter(|value| !value.is_empty()),
        candidate_observation: parsed
            .candidate_observation
            .map(|value| clean_line(&value))
            .filter(|value| !value.is_empty()),
    })
}

fn deterministic_work_understanding(input: &WorkUnderstandingInput) -> WorkUnderstanding {
    let lower = input.document_text.to_ascii_lowercase();
    let mut weak_points = Vec::new();
    if looks_circular(&lower) {
        weak_points.push(
            "Thesis/body: the reasoning is circular because the claim is restated as its proof."
                .to_string(),
        );
    }
    if let Some(summary) = input.document_summary.as_ref() {
        if summary.max_churn >= 2 {
            weak_points.push(format!(
                "Revision hotspot: one section has churn {} and still needs a stable claim.",
                summary.max_churn
            ));
        }
        if summary.structure_changed && summary.paragraph_count < 3 {
            weak_points.push(
                "Structure: the outline is still thin, so the argument lacks enough sections to carry the claim."
                    .to_string(),
            );
        }
    }
    if weak_points.is_empty() && lower.split_whitespace().count() < 80 {
        weak_points.push(
            "Development: the draft is short, so the weakest point is missing evidence and explanation."
                .to_string(),
        );
    }
    let argument_summary = summarize_document(input);
    let stuck_signal = input.document_summary.as_ref().and_then(|summary| {
        (summary.max_churn >= 2).then(|| "revision churn is concentrated".to_string())
    });
    let candidate_observation = weak_points.first().cloned();
    WorkUnderstanding {
        argument_summary,
        weak_points,
        stuck_signal,
        candidate_observation,
    }
}

fn persist_work_understanding(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    task_id: i64,
    understanding: &WorkUnderstanding,
) -> Result<EpisodeDto> {
    let json = serde_json::to_string(understanding)?;
    store.set_app_setting(&latest_key(task_id), &json)?;
    memory::record_episode(
        store,
        embeddings,
        &NewEpisode::new(
            task_id,
            memory::KIND_WORK_UNDERSTANDING,
            work_understanding_episode_text(understanding),
            "work_understanding",
        )
        .with_salience(0.84),
    )
}

fn record_fallback_budget_touch(store: &TaskStore) -> Result<()> {
    store.append_llm_usage_log(&LlmUsageLogInput {
        tier: WORK_UNDERSTANDING_BUDGET_KEY.to_string(),
        model: "local-work-understanding".to_string(),
        purpose: "work_understanding".to_string(),
        input_tokens: 0,
        output_tokens: 0,
        cached_tokens: 0,
        est_cost_usd: cost_governor::estimate_cost_usd(
            ProviderKind::Local,
            "local-work-understanding",
            crate::model_router::LlmUsage::default(),
        ),
    })
}

fn work_understanding_episode_text(understanding: &WorkUnderstanding) -> String {
    let weak = if understanding.weak_points.is_empty() {
        "none".to_string()
    } else {
        understanding.weak_points.join("; ")
    };
    format!(
        "WorkUnderstanding: {} Weak points: {}",
        understanding.argument_summary, weak
    )
}

fn summarize_document(input: &WorkUnderstandingInput) -> String {
    if let Some(goal) = input
        .current_goal
        .as_deref()
        .filter(|goal| !goal.trim().is_empty())
    {
        return format!("The work is trying to {}", goal.trim());
    }
    if let Some(first) = input.document_outline.first() {
        return format!("The work centers on {}", first.trim());
    }
    let words = input
        .document_text
        .split_whitespace()
        .take(24)
        .collect::<Vec<_>>()
        .join(" ");
    if words.is_empty() {
        "The work is not developed enough to summarize yet.".to_string()
    } else {
        format!("The draft argues: {words}")
    }
}

fn looks_circular(lower: &str) -> bool {
    lower.contains("because it is true")
        || lower.contains("because it is fair")
        || lower.contains("proves the policy is fair")
        || (lower.matches("because").count() >= 2 && lower.matches("therefore").count() >= 1)
}

fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    (end > start).then_some(&raw[start..=end])
}

fn clean_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        text.chars().take(max_chars).collect()
    }
}

fn last_run_key(task_id: i64) -> String {
    format!("{WORK_UNDERSTANDING_LAST_RUN_KEY_PREFIX}{task_id}")
}

fn latest_key(task_id: i64) -> String {
    format!("{WORK_UNDERSTANDING_LATEST_KEY_PREFIX}{task_id}")
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
    use crate::{
        model_router::{ProviderKind, RouterConfig, TierConfig},
        providers::local::hash_embedding,
    };
    use tempfile::TempDir;

    struct TestEmbeddingProvider;

    impl EmbeddingProvider for TestEmbeddingProvider {
        fn embed_text(&self, input: &str) -> Result<Vec<f32>> {
            Ok(hash_embedding(input))
        }
    }

    fn test_store() -> (TempDir, TaskStore, i64) {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        let task = store.create_task("WorkUnderstanding Test").unwrap();
        (dir, store, task.id)
    }

    fn test_router(store: &TaskStore) -> ModelRouter {
        let _ = store;
        ModelRouter::new(RouterConfig {
            reflex: TierConfig {
                provider: ProviderKind::Local,
                model: "test-local".to_string(),
            },
            conversation: TierConfig {
                provider: ProviderKind::Local,
                model: "test-local".to_string(),
            },
            judgment: TierConfig {
                provider: ProviderKind::Local,
                model: "test-local".to_string(),
            },
            craft: TierConfig {
                provider: ProviderKind::Local,
                model: "test-local".to_string(),
            },
        })
    }

    #[test]
    fn b7_parses_work_understanding_json() {
        let parsed = parse_work_understanding_json(
            r#"{"argument_summary":"Argues X","weak_points":["Section 2 lacks evidence"],"stuck_signal":"churn","candidate_observation":"ask about evidence"}"#,
        )
        .unwrap();
        assert_eq!(parsed.argument_summary, "Argues X");
        assert_eq!(parsed.weak_points[0], "Section 2 lacks evidence");
        assert_eq!(parsed.stuck_signal.as_deref(), Some("churn"));
    }

    #[test]
    fn b7_seeded_circular_document_produces_weak_point_and_episode() {
        let (_dir, store, task_id) = test_store();
        let embeddings = TestEmbeddingProvider;
        let router = test_router(&store);
        let input = WorkUnderstandingInput {
            task_id,
            document_text: "The policy is fair because it is fair. This proves the policy is fair."
                .to_string(),
            document_outline: vec!["Thesis".to_string(), "Body".to_string()],
            document_summary: Some(DocumentStateSummary {
                paragraph_count: 2,
                word_count: 14,
                structure_changed: false,
                max_churn: 0,
                churn_hotspot_count: 0,
                added_last: 0,
                removed_last: 0,
                rewritten_last: 1,
            }),
            current_goal: Some("make a persuasive argument".to_string()),
            prior: None,
        };

        let understanding =
            run_work_understanding_pass(&store, &embeddings, &router, input).unwrap();
        assert!(understanding
            .weak_points
            .iter()
            .any(|point| point.contains("circular")));
        assert!(latest_from_store(&store, task_id).unwrap().is_some());
        let episodes = memory::list_episodes(&store, task_id, 10).unwrap();
        assert!(episodes
            .iter()
            .any(|episode| episode.kind == memory::KIND_WORK_UNDERSTANDING));
        let conn = store.connect().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_usage_log WHERE tier = ?1 AND purpose = ?2",
                rusqlite::params![WORK_UNDERSTANDING_BUDGET_KEY, "work_understanding"],
                |row| row.get(0),
            )
            .unwrap();
        assert!(count >= 1);
    }

    #[test]
    fn b7_content_observation_off_or_unchanged_blocks_trigger() {
        let (_dir, store, task_id) = test_store();
        assert!(!should_run_work_understanding(&store, task_id, false, 1_000).unwrap());
        assert!(should_run_work_understanding(&store, task_id, true, 1_000).unwrap());
        mark_work_understanding_ran(&store, task_id, 1_000).unwrap();
        assert!(!should_run_work_understanding(&store, task_id, true, 1_120).unwrap());
        assert!(should_run_work_understanding(&store, task_id, true, 1_301).unwrap());
    }
}
