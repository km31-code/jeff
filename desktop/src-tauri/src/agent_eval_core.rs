// apex d7: agent eval suite. runs job contracts against the deterministic
// agent runtime and asserts delivery-contract adherence -- assessment present,
// honest capability requests on blocked/impossible tasks, budget-exhaustion
// partial delivery, and steering reflected in the job's steps. llm-quality
// grounding against real fixture workspaces (drafting that reads seeded notes,
// citations that resolve) is env-gated (craft tier + e2 web tools); this suite
// proves the runtime honors its delivery contract deterministically.

use anyhow::Result;
use serde::Deserialize;

use crate::{
    agent_runtime::{create_and_run_job, create_job, enqueue_job_steering, run_job_to_completion},
    store::TaskStore,
};

#[derive(Debug, Clone, Deserialize)]
pub struct AgentEvalContract {
    pub id: String,
    pub category: String,
    pub goal_contract: String,
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
    pub require_assessment: bool,
    #[serde(default)]
    pub require_verification: bool,
    #[serde(default)]
    pub require_capability_request: bool,
    #[serde(default)]
    pub require_steering_reflected: bool,
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
    let task = store.create_task(&contract.id)?;
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
    let mut failures: Vec<String> = Vec::new();

    if job.status != contract.expect_status {
        failures.push(format!(
            "status '{}' != expected '{}'",
            job.status, contract.expect_status
        ));
    }
    for needle in &contract.must_contain {
        if !deliverable.contains(needle.as_str()) {
            failures.push(format!("deliverable missing '{needle}'"));
        }
    }
    if contract.require_assessment && !deliverable.contains("assessment") {
        failures.push("deliverable missing assessment".to_string());
    }
    if contract.require_verification {
        let transcript = job.verification_transcript.clone().unwrap_or_default();
        if !transcript.contains("fresh-context Craft verification") {
            failures.push("missing fresh-context verification transcript".to_string());
        }
    }
    if contract.require_capability_request {
        let present = job
            .capability_request_json
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
            || deliverable.contains("capability_request");
        if !present {
            failures.push("missing structured capability request".to_string());
        }
    }
    if contract.require_steering_reflected {
        let reflected = contract.steering.iter().all(|message| {
            detail
                .steps
                .iter()
                .any(|step| step.input_json.contains(message.as_str()))
        });
        if !reflected {
            failures.push("steering not reflected in job steps".to_string());
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
            gated: false,
            budget: None,
            steering: Vec::new(),
            expect_status: expect_status.to_string(),
            must_contain: Vec::new(),
            require_assessment: false,
            require_verification: false,
            require_capability_request: false,
            require_steering_reflected: false,
        }
    }

    #[test]
    fn d7_completed_contract_passes_delivery_contract() {
        let (_dir, store) = store();
        let mut c = contract("d7-draft", "Draft a summary from the local notes.", "completed");
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
        let mut c = contract("d7-steer", "Draft a short note from the local notes.", "completed");
        c.steering = vec!["Make it two paragraphs.".to_string()];
        c.require_steering_reflected = true;
        let outcome = evaluate_agent_contract(&store, &c).unwrap();
        assert!(outcome.passed, "reason: {}", outcome.reason);
    }

    #[test]
    fn d7_detects_contract_violation() {
        let (_dir, store) = store();
        // a completed job that we wrongly expect to be blocked must fail.
        let c = contract("d7-neg", "Draft a summary from the local notes.", "blocked");
        let outcome = evaluate_agent_contract(&store, &c).unwrap();
        assert!(!outcome.passed);
        assert!(outcome.reason.contains("status"));
    }
}
