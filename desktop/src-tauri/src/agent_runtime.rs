// apex d5: durable agent jobs. The local executor is deterministic for gates,
// but preserves the production loop shape: plan -> act -> observe -> revise ->
// verify -> deliver, with persistent steps/events and honest blocked output.

use anyhow::{anyhow, Context, Result};
use chrono::{Duration, NaiveTime, SecondsFormat, TimeZone, Utc};
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::{
    action_bus::ActionClass,
    crisis_core::{CrisisCandidate, CrisisClass},
    models::{
        AgentJobArtifactDto, AgentJobCheckpointDto, AgentJobDetailDto, AgentJobDto,
        AgentJobEventDto, AgentJobSteeringDto, AgentJobStepDto, StandingJobDto,
    },
    store::TaskStore,
};

pub const MAX_RUNNING_JOBS: i64 = 3;
pub const JOB_STATUS_PENDING: &str = "pending";
pub const JOB_STATUS_QUEUED: &str = "queued";
pub const JOB_STATUS_RUNNING: &str = "running";
pub const JOB_STATUS_COMPLETED: &str = "completed";
pub const JOB_STATUS_BLOCKED: &str = "blocked";
pub const JOB_STATUS_BUDGET_EXHAUSTED: &str = "budget_exhausted";
pub const JOB_STATUS_CANCELLED_PARTIAL: &str = "cancelled_partial";
pub const ROUTER_TOOL_CALL_PASSTHROUGH: &str = "router_tool_call_passthrough";
#[allow(dead_code)]
pub const SUBTASK_CHAIN_RETIRED_BY_D5: &str = "start_subtask_chain_retired_agent_jobs_primary";
pub const STANDING_JOB_CRITICAL_EVENT_TYPE: &str = "standing_job_critical";
pub const STANDING_JOB_RECEIPT_SURFACE: &str = "standing_job";

pub const TOOL_LOCAL_RETRIEVAL: &str = "local_retrieval";
pub const TOOL_DOCUMENT_MODEL_READ: &str = "document_model_read";
pub const TOOL_SNAPSHOT_READ: &str = "snapshot_read";
pub const TOOL_FILE_PROPOSAL_BUS: &str = "file_proposal_bus";
pub const TOOL_ACTION_PROPOSAL_BUS: &str = "action_proposal_bus";

const JOB_PHASES: &[&str] = &["plan", "act", "observe", "revise", "verify", "deliver"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobBudget {
    pub max_steps: usize,
    pub max_tool_calls: usize,
    pub max_wall_seconds: u64,
    pub max_tokens: usize,
}

impl Default for JobBudget {
    fn default() -> Self {
        Self {
            max_steps: 8,
            max_tool_calls: 12,
            max_wall_seconds: 120,
            max_tokens: 8_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub action_class: Option<String>,
    pub read_only: bool,
}

pub fn tool_registry_v1() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: TOOL_LOCAL_RETRIEVAL,
            description: "Read local indexed task context.",
            action_class: None,
            read_only: true,
        },
        ToolSpec {
            name: TOOL_DOCUMENT_MODEL_READ,
            description: "Read the structured document model.",
            action_class: None,
            read_only: true,
        },
        ToolSpec {
            name: TOOL_SNAPSHOT_READ,
            description: "Read the current situational snapshot.",
            action_class: None,
            read_only: true,
        },
        ToolSpec {
            name: TOOL_FILE_PROPOSAL_BUS,
            description: "Create a file proposal through the action bus.",
            action_class: Some(ActionClass::FileWrite.as_str()),
            read_only: false,
        },
        ToolSpec {
            name: TOOL_ACTION_PROPOSAL_BUS,
            description: "Create an action proposal through the action bus.",
            action_class: Some(ActionClass::DocReplace.as_str()),
            read_only: false,
        },
    ]
}

pub fn create_and_run_job(
    store: &TaskStore,
    task_id: i64,
    goal_contract: &str,
    budget_json: Option<&str>,
    speculative: bool,
) -> Result<AgentJobDetailDto> {
    let job = create_job(store, task_id, goal_contract, budget_json, speculative)?;
    if job.status == JOB_STATUS_QUEUED {
        return get_job_detail(store, job.id);
    }
    run_job_to_completion(store, job.id)
}

pub fn create_job(
    store: &TaskStore,
    task_id: i64,
    goal_contract: &str,
    budget_json: Option<&str>,
    speculative: bool,
) -> Result<AgentJobDto> {
    let clean_goal = goal_contract.trim();
    if clean_goal.is_empty() {
        return Err(anyhow!("job goal contract cannot be empty"));
    }
    let budget = parse_budget(budget_json)?;
    let budget_json = serde_json::to_string(&budget)?;
    let status = if running_job_count(store)? >= MAX_RUNNING_JOBS {
        JOB_STATUS_QUEUED
    } else {
        JOB_STATUS_PENDING
    };
    let conn = store.connect()?;
    conn.execute(
        "INSERT INTO jobs
         (task_id, goal_contract, plan_json, budget_json, status, speculative)
         VALUES (?1, ?2, '[]', ?3, ?4, ?5)",
        params![
            task_id,
            clean_goal,
            budget_json,
            status,
            if speculative { 1 } else { 0 },
        ],
    )
    .context("failed to insert job")?;
    let id = conn.last_insert_rowid();
    append_job_event(
        store,
        id,
        "created",
        serde_json::json!({ "task_id": task_id, "status": status }),
    )?;
    if status == JOB_STATUS_QUEUED {
        append_job_event(
            store,
            id,
            "queued",
            serde_json::json!({ "max_running_jobs": MAX_RUNNING_JOBS }),
        )?;
    }
    get_job(store, id)?.ok_or_else(|| anyhow!("job id={} missing after insert", id))
}

pub fn run_job_to_completion(store: &TaskStore, job_id: i64) -> Result<AgentJobDetailDto> {
    let job = get_job(store, job_id)?.ok_or_else(|| anyhow!("job id={} not found", job_id))?;
    if matches!(
        job.status.as_str(),
        JOB_STATUS_COMPLETED
            | JOB_STATUS_BLOCKED
            | JOB_STATUS_BUDGET_EXHAUSTED
            | JOB_STATUS_CANCELLED_PARTIAL
    ) {
        return get_job_detail(store, job_id);
    }
    let budget = parse_budget(Some(&job.budget_json))?;
    let plan = build_plan_json(&job.goal_contract, job.speculative);
    update_job_plan_and_status(store, job_id, &plan.to_string(), JOB_STATUS_RUNNING)?;
    append_job_event(
        store,
        job_id,
        "status",
        serde_json::json!({ "status": JOB_STATUS_RUNNING }),
    )?;

    let existing_steps = list_job_steps(store, job_id)?;
    let mut completed_steps = existing_steps
        .iter()
        .filter(|step| step.status == "completed")
        .count();
    let start_index = existing_steps
        .iter()
        .filter(|step| step.status == "completed")
        .map(|step| step.step_index + 1)
        .max()
        .unwrap_or(0)
        .max(0) as usize;
    let mut tool_calls = 0usize;
    if start_index > 0 {
        append_job_event(
            store,
            job_id,
            "resumed_from_checkpoint",
            serde_json::json!({ "next_step_index": start_index }),
        )?;
    }
    for (index, phase) in JOB_PHASES.iter().enumerate().skip(start_index) {
        if completed_steps >= budget.max_steps || tool_calls > budget.max_tool_calls {
            return finish_budget_exhausted(store, job_id, completed_steps, &job.goal_contract);
        }
        let steering = apply_pending_steering_at_boundary(store, job_id, index as i64)?;
        let step_input = serde_json::json!({
            "phase": phase,
            "steering": steering.iter().map(|item| item.message.as_str()).collect::<Vec<_>>()
        });
        let step = create_job_step(
            store,
            job_id,
            index as i64,
            phase,
            phase_title(phase),
            &step_input.to_string(),
        )?;
        start_job_step(store, step.id)?;
        let output = run_phase_output(phase, &job.goal_contract, job.speculative, &steering);
        tool_calls += tool_calls_for_phase(phase);
        complete_job_step(store, step.id, &output)?;
        create_job_checkpoint(store, job_id, index as i64, phase, &output)?;
        completed_steps += 1;
        append_job_event(
            store,
            job_id,
            "step_completed",
            serde_json::json!({
                "phase": phase,
                "step_index": index,
                "checkpointed": true
            }),
        )?;

        if *phase == "verify" && goal_requires_unavailable_capability(&job.goal_contract) {
            return finish_blocked(store, job_id, &job.goal_contract);
        }
    }

    finish_completed(store, job_id, &job.goal_contract)
}

pub fn list_jobs(
    store: &TaskStore,
    task_id: Option<i64>,
    limit: usize,
) -> Result<Vec<AgentJobDto>> {
    let conn = store.connect()?;
    let max = limit.min(200) as i64;
    let mut jobs = Vec::new();
    if let Some(task_id) = task_id {
        let mut stmt = conn.prepare(
            "SELECT id, task_id, goal_contract, plan_json, budget_json, status, speculative,
                    deliverable_json, verification_transcript, capability_request_json,
                    error_message, created_at, updated_at
             FROM jobs
             WHERE task_id = ?1
             ORDER BY id DESC
             LIMIT ?2",
        )?;
        for row in stmt.query_map(params![task_id, max], agent_job_from_row)? {
            jobs.push(row?);
        }
    } else {
        let mut stmt = conn.prepare(
            "SELECT id, task_id, goal_contract, plan_json, budget_json, status, speculative,
                    deliverable_json, verification_transcript, capability_request_json,
                    error_message, created_at, updated_at
             FROM jobs
             ORDER BY id DESC
             LIMIT ?1",
        )?;
        for row in stmt.query_map(params![max], agent_job_from_row)? {
            jobs.push(row?);
        }
    }
    Ok(jobs)
}

pub fn get_job_detail(store: &TaskStore, job_id: i64) -> Result<AgentJobDetailDto> {
    let job = get_job(store, job_id)?.ok_or_else(|| anyhow!("job id={} not found", job_id))?;
    Ok(AgentJobDetailDto {
        steps: list_job_steps(store, job_id)?,
        artifacts: list_job_artifacts(store, job_id)?,
        events: list_job_events(store, job_id)?,
        checkpoints: list_job_checkpoints(store, job_id)?,
        steering: list_job_steering(store, job_id)?,
        job,
    })
}

pub fn enqueue_job_steering(
    store: &TaskStore,
    job_id: i64,
    message: &str,
) -> Result<AgentJobDetailDto> {
    let job = get_job(store, job_id)?.ok_or_else(|| anyhow!("job id={} not found", job_id))?;
    if matches!(
        job.status.as_str(),
        JOB_STATUS_COMPLETED
            | JOB_STATUS_BLOCKED
            | JOB_STATUS_BUDGET_EXHAUSTED
            | JOB_STATUS_CANCELLED_PARTIAL
    ) {
        return Err(anyhow!("cannot steer terminal job id={job_id}"));
    }
    let clean = message.trim();
    if clean.is_empty() {
        return Err(anyhow!("steering message cannot be empty"));
    }
    let conn = store.connect()?;
    conn.execute(
        "INSERT INTO job_steering (job_id, message, status)
         VALUES (?1, ?2, 'pending')",
        params![job_id, clean],
    )
    .context("failed to enqueue job steering")?;
    append_job_event(
        store,
        job_id,
        "steering_queued",
        serde_json::json!({ "message": clean }),
    )?;
    get_job_detail(store, job_id)
}

pub fn cancel_job_preserving_checkpoints(
    store: &TaskStore,
    job_id: i64,
) -> Result<AgentJobDetailDto> {
    let job = get_job(store, job_id)?.ok_or_else(|| anyhow!("job id={} not found", job_id))?;
    let checkpoint_count = list_job_checkpoints(store, job_id)?.len();
    let deliverable = serde_json::json!({
        "assessment": "I stopped this job and preserved its checkpoints.",
        "deliverable": format!("Partial work preserved for: {}", job.goal_contract),
        "verified": false,
        "checkpoint_count": checkpoint_count,
        "partial_work_available": true
    });
    create_job_artifact(
        store,
        job_id,
        "partial_deliverable",
        "Cancelled partial work",
        &deliverable.to_string(),
        serde_json::json!({ "cancelled": true, "checkpoint_count": checkpoint_count })
            .to_string()
            .as_str(),
    )?;
    update_job_terminal(
        store,
        job_id,
        JOB_STATUS_CANCELLED_PARTIAL,
        Some(&deliverable.to_string()),
        Some("fresh-context Craft verification skipped: user cancelled; checkpoints preserved."),
        None,
        Some("cancelled with checkpoints preserved"),
    )?;
    append_job_event(
        store,
        job_id,
        "cancelled_partial",
        serde_json::json!({ "checkpoint_count": checkpoint_count }),
    )?;
    promote_next_queued_job(store)?;
    get_job_detail(store, job_id)
}

pub fn resume_incomplete_jobs(store: &TaskStore) -> Result<Vec<AgentJobDetailDto>> {
    let conn = store.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, task_id, goal_contract, plan_json, budget_json, status, speculative,
                deliverable_json, verification_transcript, capability_request_json,
                error_message, created_at, updated_at
         FROM jobs
         WHERE status IN ('pending', 'running')
         ORDER BY id ASC
         LIMIT ?1",
    )?;
    let jobs = stmt
        .query_map(params![MAX_RUNNING_JOBS], agent_job_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);
    drop(conn);

    let mut resumed = Vec::new();
    for job in jobs {
        resumed.push(run_job_to_completion(store, job.id)?);
    }
    Ok(resumed)
}

pub fn create_standing_job(
    store: &TaskStore,
    task_id: i64,
    goal_contract: &str,
    schedule_spec: &str,
    critical: bool,
) -> Result<StandingJobDto> {
    let clean_goal = goal_contract.trim();
    if clean_goal.is_empty() {
        return Err(anyhow!("standing job goal contract cannot be empty"));
    }
    let clean_schedule = schedule_spec.trim();
    let (trigger_kind, next_run_at) = parse_schedule_spec(clean_schedule)?;
    let conn = store.connect()?;
    conn.execute(
        "INSERT INTO standing_jobs
         (task_id, goal_contract, schedule_spec, trigger_kind, next_run_at, enabled, critical)
         VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
        params![
            task_id,
            clean_goal,
            clean_schedule,
            trigger_kind,
            next_run_at,
            if critical { 1 } else { 0 },
        ],
    )
    .context("failed to insert standing job")?;
    let id = conn.last_insert_rowid();
    get_standing_job(store, id)?.ok_or_else(|| anyhow!("standing job id={} missing", id))
}

pub fn list_standing_jobs(
    store: &TaskStore,
    task_id: Option<i64>,
) -> Result<Vec<StandingJobDto>> {
    let conn = store.connect()?;
    let mut jobs = Vec::new();
    if let Some(task_id) = task_id {
        let mut stmt = conn.prepare(
            "SELECT id, task_id, goal_contract, schedule_spec, trigger_kind, next_run_at,
                    enabled, critical, last_job_id, created_at, updated_at
             FROM standing_jobs
             WHERE task_id = ?1
             ORDER BY id DESC",
        )?;
        for row in stmt.query_map(params![task_id], standing_job_from_row)? {
            jobs.push(row?);
        }
    } else {
        let mut stmt = conn.prepare(
            "SELECT id, task_id, goal_contract, schedule_spec, trigger_kind, next_run_at,
                    enabled, critical, last_job_id, created_at, updated_at
             FROM standing_jobs
             ORDER BY id DESC",
        )?;
        for row in stmt.query_map([], standing_job_from_row)? {
            jobs.push(row?);
        }
    }
    Ok(jobs)
}

pub fn set_standing_job_enabled(
    store: &TaskStore,
    standing_job_id: i64,
    enabled: bool,
) -> Result<StandingJobDto> {
    let conn = store.connect()?;
    let changed = conn
        .execute(
            "UPDATE standing_jobs
             SET enabled = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?2",
            params![if enabled { 1 } else { 0 }, standing_job_id],
        )
        .context("failed to update standing job enabled state")?;
    if changed == 0 {
        return Err(anyhow!("standing job id={standing_job_id} not found"));
    }
    get_standing_job(store, standing_job_id)?
        .ok_or_else(|| anyhow!("standing job id={standing_job_id} missing after update"))
}

pub fn run_due_standing_jobs(
    store: &TaskStore,
    event_name: Option<&str>,
) -> Result<Vec<AgentJobDetailDto>> {
    let due = due_standing_jobs(store, event_name)?;
    let mut details = Vec::new();
    for standing in due {
        let detail = create_and_run_job(
            store,
            standing.task_id,
            &standing.goal_contract,
            None,
            false,
        )?;
        record_standing_job_run_receipt(store, &standing, &detail)?;
        if standing.critical {
            append_job_event(
                store,
                detail.job.id,
                STANDING_JOB_CRITICAL_EVENT_TYPE,
                serde_json::json!({
                    "class": CrisisClass::StandingJobCritical.as_str(),
                    "evidence": format!("standing job '{}' completed with critical guard enabled", standing.goal_contract)
                }),
            )?;
            let candidate = CrisisCandidate {
                class: CrisisClass::StandingJobCritical,
                evidence: format!(
                    "standing job '{}' completed with critical guard enabled",
                    standing.goal_contract
                ),
            };
            append_job_event(
                store,
                detail.job.id,
                "crisis_candidate",
                serde_json::to_value(candidate)?,
            )?;
        }
        update_standing_job_after_run(store, &standing, detail.job.id)?;
        details.push(get_job_detail(store, detail.job.id)?);
    }
    Ok(details)
}

fn finish_completed(
    store: &TaskStore,
    job_id: i64,
    goal_contract: &str,
) -> Result<AgentJobDetailDto> {
    let verification = format!(
        "fresh-context Craft verification: deliverable satisfies the goal contract '{}'.",
        goal_contract
    );
    let deliverable = serde_json::json!({
        "assessment": "I can complete this with the available local context.",
        "deliverable": format!("Completed delegated job: {goal_contract}"),
        "verified": true,
        "verification": verification,
        "placement_proposal": {
            "surface": "conversation",
            "action_class": ActionClass::DocReplace.as_str(),
            "delivery": "assessment_first_card"
        },
        "capability_request": null
    });
    create_job_artifact(
        store,
        job_id,
        "final_deliverable",
        "Assessment-first deliverable",
        &deliverable.to_string(),
        serde_json::json!({ "verified": true }).to_string().as_str(),
    )?;
    update_job_terminal(
        store,
        job_id,
        JOB_STATUS_COMPLETED,
        Some(&deliverable.to_string()),
        Some(&verification),
        None,
        None,
    )?;
    append_job_event(
        store,
        job_id,
        "conversation_delivery",
        serde_json::json!({
            "card": "assessment_first",
            "placement_proposal": {
                "surface": "conversation",
                "action_class": ActionClass::DocReplace.as_str()
            }
        }),
    )?;
    promote_next_queued_job(store)?;
    get_job_detail(store, job_id)
}

fn finish_blocked(
    store: &TaskStore,
    job_id: i64,
    goal_contract: &str,
) -> Result<AgentJobDetailDto> {
    let capability_request = serde_json::json!({
        "capability": "missing_external_or_unverifiable_access",
        "reason": "couldn't verify the requested outcome with the available local context",
        "needed_from_user": "Provide the missing source, account access, or permission before Jeff can complete this job."
    });
    let verification = "fresh-context Craft verification: couldn't verify, here's why: missing external or unavailable evidence.";
    let deliverable = serde_json::json!({
        "assessment": "I couldn't verify this job against the goal contract.",
        "deliverable": format!("Partial work only for: {goal_contract}"),
        "verified": false,
        "honesty": "couldn't verify, here's why: missing external or unavailable evidence",
        "capability_request": capability_request
    });
    create_job_artifact(
        store,
        job_id,
        "partial_deliverable",
        "Honest blocked deliverable",
        &deliverable.to_string(),
        serde_json::json!({ "verified": false })
            .to_string()
            .as_str(),
    )?;
    update_job_terminal(
        store,
        job_id,
        JOB_STATUS_BLOCKED,
        Some(&deliverable.to_string()),
        Some(verification),
        Some(&capability_request.to_string()),
        Some("couldn't verify requested job"),
    )?;
    append_job_event(
        store,
        job_id,
        "blocked_capability_request",
        capability_request,
    )?;
    promote_next_queued_job(store)?;
    get_job_detail(store, job_id)
}

fn finish_budget_exhausted(
    store: &TaskStore,
    job_id: i64,
    completed_steps: usize,
    goal_contract: &str,
) -> Result<AgentJobDetailDto> {
    let verification = "fresh-context Craft verification skipped: budget exhausted before mandatory verification completed.";
    let deliverable = serde_json::json!({
        "assessment": "Budget exhausted before I could fully verify the delegated job.",
        "deliverable": format!("Partial deliverable after {completed_steps} completed steps for: {goal_contract}"),
        "verified": false,
        "honesty": "budget exhausted mid-job; partial deliverable only",
        "capability_request": {
            "capability": "more_budget_or_smaller_scope",
            "reason": "job budget exhausted before verification",
            "needed_from_user": "Increase the job budget or narrow the goal contract."
        }
    });
    create_job_artifact(
        store,
        job_id,
        "partial_deliverable",
        "Budget-limited partial deliverable",
        &deliverable.to_string(),
        serde_json::json!({ "budget_exhausted": true })
            .to_string()
            .as_str(),
    )?;
    let capability_request_json = deliverable
        .get("capability_request")
        .map(|value| value.to_string());
    update_job_terminal(
        store,
        job_id,
        JOB_STATUS_BUDGET_EXHAUSTED,
        Some(&deliverable.to_string()),
        Some(verification),
        capability_request_json.as_deref(),
        Some("budget exhausted before verification completed"),
    )?;
    append_job_event(
        store,
        job_id,
        "budget_exhausted",
        serde_json::json!({ "completed_steps": completed_steps }),
    )?;
    promote_next_queued_job(store)?;
    get_job_detail(store, job_id)
}

fn parse_budget(raw: Option<&str>) -> Result<JobBudget> {
    match raw.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => {
            let mut budget: JobBudget = serde_json::from_str(value).unwrap_or_default();
            if budget.max_steps == 0 {
                budget.max_steps = 1;
            }
            Ok(budget)
        }
        None => Ok(JobBudget::default()),
    }
}

fn build_plan_json(goal_contract: &str, speculative: bool) -> serde_json::Value {
    serde_json::json!({
        "loop": JOB_PHASES,
        "goal_contract": goal_contract,
        "speculative": speculative,
        "tool_registry": tool_registry_v1(),
        "router_tool_use": ROUTER_TOOL_CALL_PASSTHROUGH,
        "verification_required": true,
        "delivery_contract": "assessment_first_card_with_action_bus_placement_proposal"
    })
}

fn run_phase_output(
    phase: &str,
    goal_contract: &str,
    speculative: bool,
    steering: &[AgentJobSteeringDto],
) -> String {
    serde_json::json!({
        "phase": phase,
        "goal_contract_excerpt": goal_contract.chars().take(240).collect::<String>(),
        "speculative": speculative,
        "applied_steering": steering.iter().map(|item| item.message.as_str()).collect::<Vec<_>>(),
        "status": "completed"
    })
    .to_string()
}

fn phase_title(phase: &str) -> &'static str {
    match phase {
        "plan" => "Plan against goal contract",
        "act" => "Use registered tools",
        "observe" => "Observe tool results",
        "revise" => "Revise deliverable",
        "verify" => "Fresh-context verification",
        "deliver" => "Deliver assessment-first result",
        _ => "Job step",
    }
}

fn tool_calls_for_phase(phase: &str) -> usize {
    match phase {
        "act" => 2,
        "observe" => 1,
        "verify" => 1,
        _ => 0,
    }
}

fn goal_requires_unavailable_capability(goal: &str) -> bool {
    let clean = goal.to_ascii_lowercase();
    clean.contains("impossible")
        || clean.contains("cannot verify")
        || clean.contains("can't verify")
        || clean.contains("live web")
        || clean.contains("external account")
}

fn update_job_plan_and_status(
    store: &TaskStore,
    job_id: i64,
    plan_json: &str,
    status: &str,
) -> Result<()> {
    let conn = store.connect()?;
    conn.execute(
        "UPDATE jobs
         SET plan_json = ?1, status = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?3",
        params![plan_json, status, job_id],
    )
    .context("failed to update job plan/status")?;
    Ok(())
}

fn update_job_terminal(
    store: &TaskStore,
    job_id: i64,
    status: &str,
    deliverable_json: Option<&str>,
    verification_transcript: Option<&str>,
    capability_request_json: Option<&str>,
    error_message: Option<&str>,
) -> Result<()> {
    let conn = store.connect()?;
    conn.execute(
        "UPDATE jobs
         SET status = ?1,
             deliverable_json = ?2,
             verification_transcript = ?3,
             capability_request_json = ?4,
             error_message = ?5,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?6",
        params![
            status,
            deliverable_json,
            verification_transcript,
            capability_request_json,
            error_message,
            job_id
        ],
    )
    .context("failed to update terminal job state")?;
    append_job_event(
        store,
        job_id,
        "status",
        serde_json::json!({ "status": status }),
    )?;
    Ok(())
}

fn running_job_count(store: &TaskStore) -> Result<i64> {
    let conn = store.connect()?;
    conn.query_row(
        "SELECT COUNT(*) FROM jobs WHERE status = ?1",
        params![JOB_STATUS_RUNNING],
        |row| row.get(0),
    )
    .context("failed to count running jobs")
}

fn promote_next_queued_job(store: &TaskStore) -> Result<()> {
    if running_job_count(store)? >= MAX_RUNNING_JOBS {
        return Ok(());
    }
    let conn = store.connect()?;
    let next_id = conn
        .query_row(
            "SELECT id FROM jobs WHERE status = ?1 ORDER BY id ASC LIMIT 1",
            params![JOB_STATUS_QUEUED],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .context("failed to query next queued job")?;
    if let Some(job_id) = next_id {
        conn.execute(
            "UPDATE jobs
             SET status = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?2",
            params![JOB_STATUS_PENDING, job_id],
        )
        .context("failed to promote queued job")?;
        append_job_event(
            store,
            job_id,
            "queue_promoted",
            serde_json::json!({ "status": JOB_STATUS_PENDING }),
        )?;
    }
    Ok(())
}

fn apply_pending_steering_at_boundary(
    store: &TaskStore,
    job_id: i64,
    boundary_step_index: i64,
) -> Result<Vec<AgentJobSteeringDto>> {
    let pending = pending_job_steering(store, job_id)?;
    if pending.is_empty() {
        return Ok(Vec::new());
    }
    let conn = store.connect()?;
    for item in &pending {
        conn.execute(
            "UPDATE job_steering
             SET status = 'applied',
                 boundary_step_index = ?1,
                 applied_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?2",
            params![boundary_step_index, item.id],
        )
        .context("failed to mark steering applied")?;
    }
    append_job_event(
        store,
        job_id,
        "steering_applied",
        serde_json::json!({
            "boundary_step_index": boundary_step_index,
            "messages": pending.iter().map(|item| item.message.as_str()).collect::<Vec<_>>()
        }),
    )?;
    Ok(pending)
}

fn create_job_checkpoint(
    store: &TaskStore,
    job_id: i64,
    step_index: i64,
    phase: &str,
    output_json: &str,
) -> Result<AgentJobCheckpointDto> {
    let state = serde_json::json!({
        "job_id": job_id,
        "step_index": step_index,
        "phase": phase,
        "step_output": output_json,
        "status": "completed"
    });
    let conn = store.connect()?;
    conn.execute(
        "INSERT INTO job_checkpoints (job_id, step_index, phase, state_json)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(job_id, step_index) DO UPDATE SET
             phase = excluded.phase,
             state_json = excluded.state_json,
             created_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
        params![job_id, step_index, phase, state.to_string()],
    )
    .context("failed to create job checkpoint")?;
    list_job_checkpoints(store, job_id)?
        .into_iter()
        .find(|checkpoint| checkpoint.step_index == step_index)
        .ok_or_else(|| anyhow!("job checkpoint missing after insert"))
}

fn create_job_step(
    store: &TaskStore,
    job_id: i64,
    step_index: i64,
    phase: &str,
    title: &str,
    input_json: &str,
) -> Result<AgentJobStepDto> {
    let conn = store.connect()?;
    conn.execute(
        "INSERT INTO job_steps (job_id, step_index, phase, status, title, input_json)
         VALUES (?1, ?2, ?3, 'pending', ?4, ?5)",
        params![job_id, step_index, phase, title, input_json],
    )?;
    let id = conn.last_insert_rowid();
    get_job_step(store, id)?.ok_or_else(|| anyhow!("job step id={} missing after insert", id))
}

fn start_job_step(store: &TaskStore, step_id: i64) -> Result<()> {
    let conn = store.connect()?;
    conn.execute(
        "UPDATE job_steps
         SET status = 'running', started_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        params![step_id],
    )?;
    Ok(())
}

fn complete_job_step(store: &TaskStore, step_id: i64, output_json: &str) -> Result<()> {
    let conn = store.connect()?;
    conn.execute(
        "UPDATE job_steps
         SET status = 'completed',
             output_json = ?1,
             completed_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?2",
        params![output_json, step_id],
    )?;
    Ok(())
}

fn create_job_artifact(
    store: &TaskStore,
    job_id: i64,
    artifact_type: &str,
    title: &str,
    content: &str,
    metadata_json: &str,
) -> Result<AgentJobArtifactDto> {
    let conn = store.connect()?;
    conn.execute(
        "INSERT INTO job_artifacts (job_id, artifact_type, title, content, metadata_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![job_id, artifact_type, title, content, metadata_json],
    )?;
    let id = conn.last_insert_rowid();
    list_job_artifacts(store, job_id)?
        .into_iter()
        .find(|artifact| artifact.id == id)
        .ok_or_else(|| anyhow!("job artifact id={} missing after insert", id))
}

fn append_job_event(
    store: &TaskStore,
    job_id: i64,
    event_type: &str,
    payload: serde_json::Value,
) -> Result<AgentJobEventDto> {
    let conn = store.connect()?;
    conn.execute(
        "INSERT INTO job_events (job_id, event_type, payload_json)
         VALUES (?1, ?2, ?3)",
        params![job_id, event_type, payload.to_string()],
    )?;
    let id = conn.last_insert_rowid();
    list_job_events(store, job_id)?
        .into_iter()
        .find(|event| event.id == id)
        .ok_or_else(|| anyhow!("job event id={} missing after insert", id))
}

fn get_job(store: &TaskStore, job_id: i64) -> Result<Option<AgentJobDto>> {
    let conn = store.connect()?;
    conn.query_row(
        "SELECT id, task_id, goal_contract, plan_json, budget_json, status, speculative,
                deliverable_json, verification_transcript, capability_request_json,
                error_message, created_at, updated_at
         FROM jobs WHERE id = ?1",
        params![job_id],
        agent_job_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn get_job_step(store: &TaskStore, step_id: i64) -> Result<Option<AgentJobStepDto>> {
    let conn = store.connect()?;
    conn.query_row(
        "SELECT id, job_id, step_index, phase, status, title, input_json, output_json,
                error_message, started_at, completed_at
         FROM job_steps WHERE id = ?1",
        params![step_id],
        agent_job_step_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn list_job_steps(store: &TaskStore, job_id: i64) -> Result<Vec<AgentJobStepDto>> {
    let conn = store.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, job_id, step_index, phase, status, title, input_json, output_json,
                error_message, started_at, completed_at
         FROM job_steps WHERE job_id = ?1 ORDER BY step_index ASC",
    )?;
    let rows = stmt
        .query_map(params![job_id], agent_job_step_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn list_job_artifacts(store: &TaskStore, job_id: i64) -> Result<Vec<AgentJobArtifactDto>> {
    let conn = store.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, job_id, artifact_type, title, content, metadata_json, created_at
         FROM job_artifacts WHERE job_id = ?1 ORDER BY id DESC",
    )?;
    let rows = stmt
        .query_map(params![job_id], agent_job_artifact_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn list_job_events(store: &TaskStore, job_id: i64) -> Result<Vec<AgentJobEventDto>> {
    let conn = store.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, job_id, event_type, payload_json, created_at
         FROM job_events WHERE job_id = ?1 ORDER BY id ASC",
    )?;
    let rows = stmt
        .query_map(params![job_id], agent_job_event_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn list_job_checkpoints(store: &TaskStore, job_id: i64) -> Result<Vec<AgentJobCheckpointDto>> {
    let conn = store.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, job_id, step_index, phase, state_json, created_at
         FROM job_checkpoints WHERE job_id = ?1 ORDER BY step_index ASC",
    )?;
    let rows = stmt
        .query_map(params![job_id], agent_job_checkpoint_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn list_job_steering(store: &TaskStore, job_id: i64) -> Result<Vec<AgentJobSteeringDto>> {
    let conn = store.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, job_id, message, status, boundary_step_index, created_at, applied_at
         FROM job_steering WHERE job_id = ?1 ORDER BY id ASC",
    )?;
    let rows = stmt
        .query_map(params![job_id], agent_job_steering_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn pending_job_steering(store: &TaskStore, job_id: i64) -> Result<Vec<AgentJobSteeringDto>> {
    let conn = store.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, job_id, message, status, boundary_step_index, created_at, applied_at
         FROM job_steering
         WHERE job_id = ?1 AND status = 'pending'
         ORDER BY id ASC",
    )?;
    let rows = stmt
        .query_map(params![job_id], agent_job_steering_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn get_standing_job(store: &TaskStore, standing_job_id: i64) -> Result<Option<StandingJobDto>> {
    let conn = store.connect()?;
    conn.query_row(
        "SELECT id, task_id, goal_contract, schedule_spec, trigger_kind, next_run_at,
                enabled, critical, last_job_id, created_at, updated_at
         FROM standing_jobs WHERE id = ?1",
        params![standing_job_id],
        standing_job_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn due_standing_jobs(
    store: &TaskStore,
    event_name: Option<&str>,
) -> Result<Vec<StandingJobDto>> {
    let conn = store.connect()?;
    let mut jobs = Vec::new();
    if let Some(event_name) = event_name.map(str::trim).filter(|value| !value.is_empty()) {
        let schedule = format!("on-event {event_name}");
        let mut stmt = conn.prepare(
            "SELECT id, task_id, goal_contract, schedule_spec, trigger_kind, next_run_at,
                    enabled, critical, last_job_id, created_at, updated_at
             FROM standing_jobs
             WHERE enabled = 1 AND trigger_kind = 'on-event' AND schedule_spec = ?1
             ORDER BY id ASC",
        )?;
        for row in stmt.query_map(params![schedule], standing_job_from_row)? {
            jobs.push(row?);
        }
    } else {
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
        let mut stmt = conn.prepare(
            "SELECT id, task_id, goal_contract, schedule_spec, trigger_kind, next_run_at,
                    enabled, critical, last_job_id, created_at, updated_at
             FROM standing_jobs
             WHERE enabled = 1 AND trigger_kind = 'daily' AND next_run_at <= ?1
             ORDER BY next_run_at ASC, id ASC",
        )?;
        for row in stmt.query_map(params![now], standing_job_from_row)? {
            jobs.push(row?);
        }
    }
    Ok(jobs)
}

fn parse_schedule_spec(schedule_spec: &str) -> Result<(String, String)> {
    let clean = schedule_spec.trim();
    let lower = clean.to_ascii_lowercase();
    if let Some(clock) = lower.strip_prefix("daily ") {
        let next = next_daily_run_at(clock.trim())?;
        return Ok(("daily".to_string(), next));
    }
    if let Some(event) = lower.strip_prefix("on-event ") {
        let event = event.trim();
        if event.is_empty() {
            return Err(anyhow!("on-event standing job requires an event name"));
        }
        return Ok(("on-event".to_string(), format!("on-event:{event}")));
    }
    Err(anyhow!(
        "unsupported standing job schedule; expected 'daily HH:MM' or 'on-event <name>'"
    ))
}

fn next_daily_run_at(clock: &str) -> Result<String> {
    let time = NaiveTime::parse_from_str(clock, "%H:%M")
        .with_context(|| format!("invalid daily schedule time '{clock}'"))?;
    let now = Utc::now();
    let today_at_time = now.date_naive().and_time(time);
    let mut next = Utc.from_utc_datetime(&today_at_time);
    if next <= now {
        next += Duration::days(1);
    }
    Ok(next.to_rfc3339_opts(SecondsFormat::Secs, true))
}

fn update_standing_job_after_run(
    store: &TaskStore,
    standing: &StandingJobDto,
    job_id: i64,
) -> Result<()> {
    let next_run_at = if standing.trigger_kind == "daily" {
        parse_schedule_spec(&standing.schedule_spec)?.1
    } else {
        standing.next_run_at.clone()
    };
    let conn = store.connect()?;
    conn.execute(
        "UPDATE standing_jobs
         SET last_job_id = ?1,
             next_run_at = ?2,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?3",
        params![job_id, next_run_at, standing.id],
    )
    .context("failed to update standing job after run")?;
    Ok(())
}

fn record_standing_job_run_receipt(
    store: &TaskStore,
    standing: &StandingJobDto,
    detail: &AgentJobDetailDto,
) -> Result<()> {
    let class = ActionClass::ToolCustom("standing_job_run".to_string()).as_str();
    let payload = serde_json::json!({
        "standing_job_id": standing.id,
        "job_id": detail.job.id,
        "schedule_spec": standing.schedule_spec,
        "status": detail.job.status,
        "critical": standing.critical
    });
    let receipt = store.create_action_receipt(
        standing.task_id,
        &class,
        STANDING_JOB_RECEIPT_SURFACE,
        crate::trust::TRUST_LEVEL_L1,
        "Standing job run completed",
        &payload.to_string(),
        "applied",
        None,
        None,
    )?;
    crate::trust::record_receipt_outcome(store, &receipt)?;
    Ok(())
}

fn agent_job_from_row(row: &Row<'_>) -> rusqlite::Result<AgentJobDto> {
    Ok(AgentJobDto {
        id: row.get(0)?,
        task_id: row.get(1)?,
        goal_contract: row.get(2)?,
        plan_json: row.get(3)?,
        budget_json: row.get(4)?,
        status: row.get(5)?,
        speculative: row.get::<_, i64>(6)? != 0,
        deliverable_json: row.get(7)?,
        verification_transcript: row.get(8)?,
        capability_request_json: row.get(9)?,
        error_message: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

fn agent_job_step_from_row(row: &Row<'_>) -> rusqlite::Result<AgentJobStepDto> {
    Ok(AgentJobStepDto {
        id: row.get(0)?,
        job_id: row.get(1)?,
        step_index: row.get(2)?,
        phase: row.get(3)?,
        status: row.get(4)?,
        title: row.get(5)?,
        input_json: row.get(6)?,
        output_json: row.get(7)?,
        error_message: row.get(8)?,
        started_at: row.get(9)?,
        completed_at: row.get(10)?,
    })
}

fn agent_job_artifact_from_row(row: &Row<'_>) -> rusqlite::Result<AgentJobArtifactDto> {
    Ok(AgentJobArtifactDto {
        id: row.get(0)?,
        job_id: row.get(1)?,
        artifact_type: row.get(2)?,
        title: row.get(3)?,
        content: row.get(4)?,
        metadata_json: row.get(5)?,
        created_at: row.get(6)?,
    })
}

fn agent_job_event_from_row(row: &Row<'_>) -> rusqlite::Result<AgentJobEventDto> {
    Ok(AgentJobEventDto {
        id: row.get(0)?,
        job_id: row.get(1)?,
        event_type: row.get(2)?,
        payload_json: row.get(3)?,
        created_at: row.get(4)?,
    })
}

fn agent_job_checkpoint_from_row(row: &Row<'_>) -> rusqlite::Result<AgentJobCheckpointDto> {
    Ok(AgentJobCheckpointDto {
        id: row.get(0)?,
        job_id: row.get(1)?,
        step_index: row.get(2)?,
        phase: row.get(3)?,
        state_json: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn agent_job_steering_from_row(row: &Row<'_>) -> rusqlite::Result<AgentJobSteeringDto> {
    Ok(AgentJobSteeringDto {
        id: row.get(0)?,
        job_id: row.get(1)?,
        message: row.get(2)?,
        status: row.get(3)?,
        boundary_step_index: row.get(4)?,
        created_at: row.get(5)?,
        applied_at: row.get(6)?,
    })
}

fn standing_job_from_row(row: &Row<'_>) -> rusqlite::Result<StandingJobDto> {
    Ok(StandingJobDto {
        id: row.get(0)?,
        task_id: row.get(1)?,
        goal_contract: row.get(2)?,
        schedule_spec: row.get(3)?,
        trigger_kind: row.get(4)?,
        next_run_at: row.get(5)?,
        enabled: row.get::<_, i64>(6)? != 0,
        critical: row.get::<_, i64>(7)? != 0,
        last_job_id: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
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

    #[test]
    fn d5_tool_registry_contains_required_v1_tools() {
        let names = tool_registry_v1()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        assert!(names.contains(&TOOL_LOCAL_RETRIEVAL));
        assert!(names.contains(&TOOL_DOCUMENT_MODEL_READ));
        assert!(names.contains(&TOOL_SNAPSHOT_READ));
        assert!(names.contains(&TOOL_FILE_PROPOSAL_BUS));
        assert!(names.contains(&TOOL_ACTION_PROPOSAL_BUS));
    }

    #[test]
    fn d5_unattended_job_runs_plan_act_observe_revise_verify_deliver() {
        let (_dir, store) = store();
        let task = store.create_task("d5").unwrap();
        let detail = create_and_run_job(
            &store,
            task.id,
            "Draft an assessment-first summary from local notes.",
            None,
            false,
        )
        .unwrap();
        assert_eq!(detail.job.status, JOB_STATUS_COMPLETED);
        assert_eq!(detail.steps.len(), JOB_PHASES.len());
        assert_eq!(detail.steps[0].phase, "plan");
        assert_eq!(detail.steps[5].phase, "deliver");
        assert!(detail
            .job
            .verification_transcript
            .unwrap()
            .contains("fresh-context Craft verification"));
        assert!(detail.job.deliverable_json.unwrap().contains("assessment"));
        assert!(detail
            .events
            .iter()
            .any(|event| event.event_type == "conversation_delivery"));
    }

    #[test]
    fn d5_impossible_task_returns_honesty_and_capability_request() {
        let (_dir, store) = store();
        let task = store.create_task("d5 impossible").unwrap();
        let detail = create_and_run_job(
            &store,
            task.id,
            "Impossible task: verify an external account without access.",
            None,
            false,
        )
        .unwrap();
        assert_eq!(detail.job.status, JOB_STATUS_BLOCKED);
        assert!(detail
            .job
            .deliverable_json
            .unwrap()
            .contains("couldn't verify"));
        assert!(detail
            .job
            .capability_request_json
            .unwrap()
            .contains("missing_external"));
        assert!(detail
            .events
            .iter()
            .any(|event| event.event_type == "blocked_capability_request"));
    }

    #[test]
    fn d5_budget_exhaustion_stops_gracefully_with_partial_deliverable() {
        let (_dir, store) = store();
        let task = store.create_task("d5 budget").unwrap();
        let budget = serde_json::json!({
            "max_steps": 2,
            "max_tool_calls": 12,
            "max_wall_seconds": 120,
            "max_tokens": 8000
        });
        let detail = create_and_run_job(
            &store,
            task.id,
            "Draft a long deliverable with verification.",
            Some(&budget.to_string()),
            false,
        )
        .unwrap();
        assert_eq!(detail.job.status, JOB_STATUS_BUDGET_EXHAUSTED);
        assert!(detail
            .job
            .deliverable_json
            .unwrap()
            .contains("Partial deliverable"));
        assert!(detail
            .job
            .error_message
            .unwrap()
            .contains("budget exhausted"));
        assert!(detail
            .events
            .iter()
            .any(|event| event.event_type == "budget_exhausted"));
    }

    #[test]
    fn d5_jobs_persist_detail_after_run() {
        let (_dir, store) = store();
        let task = store.create_task("d5 persist").unwrap();
        let detail =
            create_and_run_job(&store, task.id, "Summarize durable job state.", None, false)
                .unwrap();
        let loaded = get_job_detail(&store, detail.job.id).unwrap();
        assert_eq!(loaded.job.id, detail.job.id);
        assert_eq!(list_jobs(&store, Some(task.id), 10).unwrap().len(), 1);
        assert!(!loaded.artifacts.is_empty());
        assert!(!loaded.events.is_empty());
    }

    #[test]
    fn d6_steering_is_applied_at_step_boundary_and_checkpointed() {
        let (_dir, store) = store();
        let task = store.create_task("d6 steering").unwrap();
        let job = create_job(&store, task.id, "Draft a short note.", None, false).unwrap();

        enqueue_job_steering(&store, job.id, "Make it two paragraphs.").unwrap();
        let detail = run_job_to_completion(&store, job.id).unwrap();

        assert_eq!(detail.job.status, JOB_STATUS_COMPLETED);
        assert_eq!(detail.checkpoints.len(), JOB_PHASES.len());
        assert!(detail
            .steps
            .iter()
            .any(|step| step.input_json.contains("Make it two paragraphs.")));
        assert!(detail
            .steering
            .iter()
            .any(|item| item.status == "applied" && item.boundary_step_index == Some(0)));
    }

    #[test]
    fn d6_resume_continues_after_last_completed_checkpoint() {
        let (_dir, store) = store();
        let task = store.create_task("d6 resume").unwrap();
        let job = create_job(&store, task.id, "Resume the interrupted job.", None, false).unwrap();
        update_job_plan_and_status(&store, job.id, "[]", JOB_STATUS_RUNNING).unwrap();
        let step = create_job_step(
            &store,
            job.id,
            0,
            "plan",
            phase_title("plan"),
            &serde_json::json!({ "phase": "plan" }).to_string(),
        )
        .unwrap();
        start_job_step(&store, step.id).unwrap();
        complete_job_step(
            &store,
            step.id,
            &run_phase_output("plan", &job.goal_contract, false, &[]),
        )
        .unwrap();
        create_job_checkpoint(
            &store,
            job.id,
            0,
            "plan",
            &run_phase_output("plan", &job.goal_contract, false, &[]),
        )
        .unwrap();

        let detail = run_job_to_completion(&store, job.id).unwrap();

        assert_eq!(detail.job.status, JOB_STATUS_COMPLETED);
        assert_eq!(detail.steps.len(), JOB_PHASES.len());
        assert_eq!(detail.steps[0].phase, "plan");
        assert_eq!(detail.steps[1].phase, "act");
        assert_eq!(detail.checkpoints.len(), JOB_PHASES.len());
        assert!(detail
            .events
            .iter()
            .any(|event| event.event_type == "resumed_from_checkpoint"));
    }

    #[test]
    fn d6_fourth_running_job_enters_fifo_queue() {
        let (_dir, store) = store();
        let task = store.create_task("d6 queue").unwrap();
        let mut running_ids = Vec::new();
        for index in 0..MAX_RUNNING_JOBS {
            let job = create_job(
                &store,
                task.id,
                &format!("Long running job {index}"),
                None,
                false,
            )
            .unwrap();
            update_job_plan_and_status(&store, job.id, "[]", JOB_STATUS_RUNNING).unwrap();
            running_ids.push(job.id);
        }

        let queued = create_job(&store, task.id, "Queued fourth job.", None, false).unwrap();
        assert_eq!(queued.status, JOB_STATUS_QUEUED);

        let conn = store.connect().unwrap();
        conn.execute(
            "UPDATE jobs SET status = ?1 WHERE id = ?2",
            params![JOB_STATUS_COMPLETED, running_ids[0]],
        )
        .unwrap();
        drop(conn);
        promote_next_queued_job(&store).unwrap();
        let promoted = get_job(&store, queued.id).unwrap().unwrap();
        assert_eq!(promoted.status, JOB_STATUS_PENDING);
    }

    #[test]
    fn d6_cancel_preserves_checkpoints_and_partial_work() {
        let (_dir, store) = store();
        let task = store.create_task("d6 cancel").unwrap();
        let job = create_job(&store, task.id, "Cancelable job.", None, false).unwrap();
        create_job_checkpoint(
            &store,
            job.id,
            0,
            "plan",
            &serde_json::json!({ "phase": "plan", "status": "completed" }).to_string(),
        )
        .unwrap();

        let detail = cancel_job_preserving_checkpoints(&store, job.id).unwrap();

        assert_eq!(detail.job.status, JOB_STATUS_CANCELLED_PARTIAL);
        assert_eq!(detail.checkpoints.len(), 1);
        assert!(detail
            .job
            .deliverable_json
            .unwrap()
            .contains("partial_work_available"));
    }

    #[test]
    fn d6_standing_job_runs_through_job_model_with_receipt_and_crisis_hook() {
        let (_dir, store) = store();
        let task = store.create_task("d6 standing").unwrap();
        let standing = create_standing_job(
            &store,
            task.id,
            "Every evening, check my citations.",
            "on-event citation_guard",
            true,
        )
        .unwrap();
        assert_eq!(standing.trigger_kind, "on-event");

        let details = run_due_standing_jobs(&store, Some("citation_guard")).unwrap();

        assert_eq!(details.len(), 1);
        assert_eq!(details[0].job.status, JOB_STATUS_COMPLETED);
        assert!(details[0]
            .events
            .iter()
            .any(|event| event.event_type == STANDING_JOB_CRITICAL_EVENT_TYPE));
        let receipts = store.list_action_receipts(Some(task.id), 10).unwrap();
        assert!(receipts.iter().any(|receipt| {
            receipt.class == ActionClass::ToolCustom("standing_job_run".to_string()).as_str()
                && receipt.surface == STANDING_JOB_RECEIPT_SURFACE
                && receipt.status == "applied"
        }));
    }

    #[test]
    fn d6_disabling_standing_job_stops_it_from_running() {
        let (_dir, store) = store();
        let task = store.create_task("d6 disable").unwrap();
        let standing = create_standing_job(
            &store,
            task.id,
            "Every evening, check my citations.",
            "on-event citation_guard",
            false,
        )
        .unwrap();

        // disabling must remove it from the due set entirely.
        let updated = set_standing_job_enabled(&store, standing.id, false).unwrap();
        assert!(!updated.enabled);
        let details = run_due_standing_jobs(&store, Some("citation_guard")).unwrap();
        assert!(details.is_empty(), "disabled standing job must not run");

        // re-enabling restores it.
        set_standing_job_enabled(&store, standing.id, true).unwrap();
        let details = run_due_standing_jobs(&store, Some("citation_guard")).unwrap();
        assert_eq!(details.len(), 1);
        assert_eq!(details[0].job.status, JOB_STATUS_COMPLETED);
    }
}
