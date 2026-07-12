// apex d7: fixture-backed agent eval suite. Contracts seed real task artifacts,
// run the local runtime, and assert grounded output, source ledgers, delivery
// structure, honest blocking, budget exhaustion, and steering in the delivered
// artifact. Only the five web-research contracts remain E2-gated.

use std::{fs, path::Path};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::{
    agent_runtime::{create_and_run_job, create_job, enqueue_job_steering, run_job_to_completion},
    store::{ChunkEmbeddingInput, TaskStore},
};

#[derive(Debug, Clone, Deserialize)]
pub struct AgentEvalContract {
    pub id: String,
    pub category: String,
    pub goal_contract: String,
    #[serde(default)]
    pub fixture_workspace: Option<String>,
    // e2-gated web-research contracts are skipped until the tool bus lands.
    #[serde(default)]
    pub gated: bool,
    // optional serialized JobBudget object (e.g. { "max_steps": 2, ... }).
    #[serde(default)]
    pub budget: Option<serde_json::Value>,
    #[serde(default)]
    pub steering: Vec<String>,
    pub expect_status: String,
    #[serde(default)]
    pub must_contain: Vec<String>,
    #[serde(default)]
    pub must_not_contain: Vec<String>,
    #[serde(default)]
    pub expected_source_files: Vec<String>,
    #[serde(default)]
    pub expected_paragraphs: Option<usize>,
    #[serde(default)]
    pub require_grounded_output: bool,
    #[serde(default)]
    pub require_assessment: bool,
    #[serde(default)]
    pub require_verification: bool,
    #[serde(default)]
    pub require_capability_request: bool,
    #[serde(default)]
    pub require_steering_reflected: bool,
    #[serde(default)]
    pub require_zero_fabricated_citations: bool,
}

#[derive(Debug, Clone)]
pub struct ContractOutcome {
    pub id: String,
    pub passed: bool,
    pub reason: String,
}

pub fn evaluate_agent_contract(
    store: &TaskStore,
    contract: &AgentEvalContract,
) -> Result<ContractOutcome> {
    evaluate_agent_contract_in_workspace(store, contract, Path::new("eval/agent_eval"))
}

pub fn evaluate_agent_contract_in_workspace(
    store: &TaskStore,
    contract: &AgentEvalContract,
    eval_root: &Path,
) -> Result<ContractOutcome> {
    let task = store.create_task(&contract.id)?;
    let fixture_texts = seed_fixture_workspace(store, task.id, contract, eval_root)?;
    let budget_json = contract.budget.as_ref().map(|value| value.to_string());

    let detail = if contract.steering.is_empty() {
        create_and_run_job(
            store,
            task.id,
            &contract.goal_contract,
            budget_json.as_deref(),
            false,
        )?
    } else {
        let job = create_job(
            store,
            task.id,
            &contract.goal_contract,
            budget_json.as_deref(),
            false,
        )?;
        for message in &contract.steering {
            enqueue_job_steering(store, job.id, message)?;
        }
        run_job_to_completion(store, job.id)?
    };

    let job = &detail.job;
    let deliverable = job.deliverable_json.clone().unwrap_or_default();
    let deliverable_json =
        serde_json::from_str::<serde_json::Value>(&deliverable).unwrap_or(serde_json::Value::Null);
    let artifact_text = deliverable_json
        .get("deliverable")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let mut failures: Vec<String> = Vec::new();

    if job.status != contract.expect_status {
        failures.push(format!(
            "status '{}' != expected '{}'",
            job.status, contract.expect_status
        ));
    }
    for needle in &contract.must_contain {
        if !artifact_text.contains(needle.as_str()) && !deliverable.contains(needle.as_str()) {
            failures.push(format!("deliverable missing '{needle}'"));
        }
    }
    for needle in &contract.must_not_contain {
        if artifact_text.contains(needle.as_str()) {
            failures.push(format!("deliverable unexpectedly contains '{needle}'"));
        }
    }
    if contract.require_assessment
        && deliverable_json
            .get("assessment")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
    {
        failures.push("deliverable missing assessment".to_string());
    }
    if contract.require_verification {
        let transcript = job.verification_transcript.clone().unwrap_or_default();
        if !transcript.contains("fresh-context deterministic verification") {
            failures.push("missing fresh-context verification transcript".to_string());
        }
    }
    if contract.require_capability_request {
        let present = job
            .capability_request_json
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
            || deliverable_json
                .get("capability_request")
                .map(|value| !value.is_null())
                .unwrap_or(false);
        if !present {
            failures.push("missing structured capability request".to_string());
        }
    }
    if contract.require_steering_reflected {
        let reflected = contract.steering.iter().all(|message| {
            artifact_text.contains(message)
                || (message.to_ascii_lowercase().contains("two paragraph")
                    && artifact_text.matches("\n\n").count() == 1)
        });
        if !reflected {
            failures.push("steering not reflected in delivered artifact".to_string());
        }
    }
    if let Some(expected) = contract.expected_paragraphs {
        let actual = artifact_text
            .split("\n\n")
            .filter(|paragraph| !paragraph.trim().is_empty())
            .count();
        if actual != expected {
            failures.push(format!(
                "deliverable has {actual} paragraphs, expected {expected}"
            ));
        }
    }
    for expected_file in &contract.expected_source_files {
        let found = deliverable_json
            .get("source_ledger")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items.iter().any(|item| {
                    item.get("file_name").and_then(serde_json::Value::as_str)
                        == Some(expected_file.as_str())
                })
            })
            .unwrap_or(false);
        if !found {
            failures.push(format!("source ledger missing '{expected_file}'"));
        }
    }
    if contract.require_zero_fabricated_citations {
        let ledger_urls = deliverable_json
            .get("source_ledger")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("file_name").and_then(serde_json::Value::as_str))
                    .filter(|source| source.starts_with("https://"))
                    .collect::<std::collections::HashSet<_>>()
            })
            .unwrap_or_default();
        let cited_urls = artifact_text
            .split_whitespace()
            .map(|token| token.trim_matches(|ch: char| "[](),.;".contains(ch)))
            .filter(|token| token.starts_with("https://"))
            .collect::<std::collections::HashSet<_>>();
        if cited_urls.is_empty() {
            failures.push("web deliverable contains no URL citation".to_string());
        }
        for cited in cited_urls {
            if !ledger_urls.contains(cited) {
                failures.push(format!(
                    "fabricated citation '{cited}' is absent from source ledger"
                ));
            }
        }
    }
    if contract.require_grounded_output && !fixture_texts.is_empty() {
        let grounded = fixture_texts.iter().any(|text| {
            text.split_terminator(['.', '!', '?'])
                .map(str::trim)
                .filter(|sentence| sentence.len() >= 24)
                .any(|sentence| artifact_text.contains(sentence))
        });
        if !grounded {
            failures.push("deliverable is not grounded in a fixture sentence".to_string());
        }
    }

    let passed = failures.is_empty();
    let reason = if passed {
        "ok".to_string()
    } else {
        failures.join("; ")
    };
    Ok(ContractOutcome {
        id: contract.id.clone(),
        passed,
        reason,
    })
}

fn seed_fixture_workspace(
    store: &TaskStore,
    task_id: i64,
    contract: &AgentEvalContract,
    eval_root: &Path,
) -> Result<Vec<String>> {
    let Some(relative) = contract.fixture_workspace.as_deref() else {
        return Ok(Vec::new());
    };
    let workspace = eval_root.join(relative);
    let mut entries = fs::read_dir(&workspace)
        .with_context(|| format!("missing fixture workspace {}", workspace.display()))?
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut fixture_texts = Vec::new();
    for entry in entries {
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        let raw_content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read fixture {}", path.display()))?;
        let (source_name, content) = if contract.category == "web_research" {
            let mut lines = raw_content.lines();
            let source = lines
                .next()
                .and_then(|line| line.strip_prefix("URL:"))
                .map(str::trim)
                .filter(|url| url.starts_with("https://"))
                .ok_or_else(|| anyhow::anyhow!("web fixture must begin with an HTTPS URL line"))?;
            (
                source.to_string(),
                lines.collect::<Vec<_>>().join("\n").trim().to_string(),
            )
        } else {
            (entry.file_name().to_string_lossy().to_string(), raw_content)
        };
        let chunks = content
            .split("\n\n")
            .map(str::trim)
            .filter(|chunk| !chunk.is_empty())
            .enumerate()
            .map(|(index, chunk)| ChunkEmbeddingInput {
                chunk_text: chunk.to_string(),
                position_index: index as i64,
                embedding: Vec::new(),
                embedding_model: "agent-eval-fixture".to_string(),
            })
            .collect::<Vec<_>>();
        let source_path = if contract.category == "web_research" {
            source_name.clone()
        } else {
            path.to_string_lossy().to_string()
        };
        store.insert_artifact_with_chunks(
            task_id,
            &source_name,
            path.extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("txt"),
            &source_path,
            &source_path,
            &chunks,
        )?;
        fixture_texts.push(content);
    }
    Ok(fixture_texts)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn store() -> (TempDir, TaskStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        (dir, store)
    }

    fn contract(id: &str, goal: &str, expect_status: &str) -> AgentEvalContract {
        AgentEvalContract {
            id: id.to_string(),
            category: "test".to_string(),
            goal_contract: goal.to_string(),
            fixture_workspace: None,
            gated: false,
            budget: None,
            steering: Vec::new(),
            expect_status: expect_status.to_string(),
            must_contain: Vec::new(),
            must_not_contain: Vec::new(),
            expected_source_files: Vec::new(),
            expected_paragraphs: None,
            require_grounded_output: false,
            require_assessment: false,
            require_verification: false,
            require_capability_request: false,
            require_steering_reflected: false,
            require_zero_fabricated_citations: false,
        }
    }

    #[test]
    fn d7_completed_contract_passes_delivery_contract() {
        let (_dir, store) = store();
        let mut c = contract("d7-draft", "Assess the current task.", "completed");
        c.require_assessment = true;
        c.require_verification = true;
        c.must_contain = vec!["placement_proposal".to_string()];
        let outcome = evaluate_agent_contract(&store, &c).unwrap();
        assert!(outcome.passed, "reason: {}", outcome.reason);
    }

    #[test]
    fn d7_impossible_contract_requires_honesty_and_capability_request() {
        let (_dir, store) = store();
        let mut c = contract(
            "d7-impossible",
            "Impossible task: verify an external account without access.",
            "blocked",
        );
        c.require_capability_request = true;
        c.must_contain = vec!["couldn't verify".to_string()];
        let outcome = evaluate_agent_contract(&store, &c).unwrap();
        assert!(outcome.passed, "reason: {}", outcome.reason);
    }

    #[test]
    fn d7_budget_contract_reports_partial_delivery() {
        let (_dir, store) = store();
        let mut c = contract(
            "d7-budget",
            "Draft a long deliverable with verification from the local notes.",
            "budget_exhausted",
        );
        c.budget = Some(serde_json::json!({
            "max_steps": 2,
            "max_tool_calls": 12,
            "max_wall_seconds": 120,
            "max_tokens": 8000
        }));
        c.require_capability_request = true;
        c.must_contain = vec!["Partial deliverable".to_string()];
        let outcome = evaluate_agent_contract(&store, &c).unwrap();
        assert!(outcome.passed, "reason: {}", outcome.reason);
    }

    #[test]
    fn d7_steering_contract_is_reflected_in_steps() {
        let (_dir, store) = store();
        let mut c = contract(
            "d7-steer",
            "Draft a short note about the current task.",
            "completed",
        );
        c.steering = vec!["Emphasize constraints.".to_string()];
        c.require_steering_reflected = true;
        let outcome = evaluate_agent_contract(&store, &c).unwrap();
        assert!(outcome.passed, "reason: {}", outcome.reason);
    }

    #[test]
    fn d7_detects_contract_violation() {
        let (_dir, store) = store();
        // an impossible task blocks; wrongly expecting "completed" must be
        // detected as a status mismatch.
        let c = contract(
            "d7-neg",
            "Verify an external account that is not connected.",
            "completed",
        );
        let outcome = evaluate_agent_contract(&store, &c).unwrap();
        assert!(!outcome.passed);
        assert!(outcome.reason.contains("status"));
    }
}
