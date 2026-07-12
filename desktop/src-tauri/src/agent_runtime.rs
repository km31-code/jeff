// apex d5: durable agent jobs. production uses the craft-tier model router for
// drafting inside a persisted plan -> act -> observe -> revise -> verify ->
// deliver loop. deterministic composition is only the no-provider fallback.

use std::{collections::HashSet, time::Instant};

use anyhow::{anyhow, Context, Result};
use chrono::{Duration, Local, NaiveTime, SecondsFormat, TimeZone, Utc};
use rusqlite::{params, OptionalExtension, Row, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::{
    action_bus::ActionClass,
    crisis_core::{CrisisCandidate, CrisisClass},
    message_kind::MessageKind,
    model_router::{GenerateOptions, ModelRouter, Tier},
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

const MAX_EVIDENCE_CHUNKS: usize = 6;
const MAX_EVIDENCE_CHARS: usize = 1_200;
const MAX_RECENT_MESSAGES: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct EvidenceChunk {
    chunk_id: i64,
    artifact_id: i64,
    file_name: String,
    position_index: i64,
    text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
struct RuntimeState {
    completed_steps: usize,
    tool_calls: usize,
    tokens_used: usize,
    elapsed_ms: u64,
    evidence: Vec<EvidenceChunk>,
    observations: Vec<String>,
    task_snapshot: Option<String>,
    applied_steering: Vec<String>,
    draft: Option<String>,
    verification_findings: Vec<String>,
    verification_passed: bool,
    capability_request: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
struct LocalContext {
    task_summary: String,
    recent_messages: Vec<String>,
    evidence: Vec<EvidenceChunk>,
    artifact_names: Vec<String>,
}

#[derive(Debug, Clone)]
struct PhaseExecution {
    output: serde_json::Value,
    next_state: RuntimeState,
    tool_calls: usize,
    token_cost: usize,
}

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

// apex d8: the read-only tool set carried by speculative jobs. speculation is
// preparation for predicted requests, never autonomous action -- a speculative
// job may only read.
pub fn speculative_tool_registry() -> Vec<ToolSpec> {
    tool_registry_v1()
        .into_iter()
        .filter(|tool| tool.read_only)
        .collect()
}

// apex d8 invariant (Part IV): nothing running under the speculation scheduler
// may issue an Action Bus mutation, at any trust level. This is the runtime
// enforcement boundary -- callers that would route a job's action through the
// bus must clear it here first. Speculative jobs are rejected unconditionally.
pub fn guard_speculative_action(job: &AgentJobDto, action_class: &str) -> Result<()> {
    if job.speculative {
        return Err(anyhow!(
            "speculative jobs are read-only: action '{}' rejected for job {}",
            action_class,
            job.id
        ));
    }
    Ok(())
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

pub fn create_and_run_job_with_router(
    store: &TaskStore,
    router: &ModelRouter,
    task_id: i64,
    goal_contract: &str,
    budget_json: Option<&str>,
    speculative: bool,
) -> Result<AgentJobDetailDto> {
    let job = create_job(store, task_id, goal_contract, budget_json, speculative)?;
    if job.status == JOB_STATUS_QUEUED {
        return get_job_detail(store, job.id);
    }
    run_job_to_completion_with_router(store, router, job.id)
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
    // Reserve a runtime slot in the same immediate transaction as the insert.
    // Pending jobs count as reserved: otherwise four concurrent creators can all
    // observe zero running jobs and bypass the three-job cap.
    let mut conn = store.connect()?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("failed to reserve agent runtime slot")?;
    let active_count: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM jobs WHERE status IN (?1, ?2)",
            params![JOB_STATUS_PENDING, JOB_STATUS_RUNNING],
            |row| row.get(0),
        )
        .context("failed to count active jobs")?;
    let status = if active_count >= MAX_RUNNING_JOBS {
        JOB_STATUS_QUEUED
    } else {
        JOB_STATUS_PENDING
    };
    tx.execute(
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
    let id = tx.last_insert_rowid();
    tx.commit().context("failed to commit agent job")?;
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
    run_job_to_completion_internal(store, None, job_id)
}

pub fn run_job_to_completion_with_router(
    store: &TaskStore,
    router: &ModelRouter,
    job_id: i64,
) -> Result<AgentJobDetailDto> {
    run_job_to_completion_internal(store, Some(router), job_id)
}

fn run_job_to_completion_internal(
    store: &TaskStore,
    router: Option<&ModelRouter>,
    job_id: i64,
) -> Result<AgentJobDetailDto> {
    let mut job = get_job(store, job_id)?.ok_or_else(|| anyhow!("job id={} not found", job_id))?;
    if matches!(
        job.status.as_str(),
        JOB_STATUS_COMPLETED
            | JOB_STATUS_BLOCKED
            | JOB_STATUS_BUDGET_EXHAUSTED
            | JOB_STATUS_CANCELLED_PARTIAL
    ) {
        return get_job_detail(store, job_id);
    }
    if job.status == JOB_STATUS_QUEUED {
        return get_job_detail(store, job_id);
    }
    let budget = parse_budget(Some(&job.budget_json))?;
    let plan = if job.plan_json.trim() == "[]" || job.plan_json.trim().is_empty() {
        build_plan_json(store, &job.goal_contract, job.speculative)
    } else {
        serde_json::from_str(&job.plan_json)
            .unwrap_or_else(|_| build_plan_json(store, &job.goal_contract, job.speculative))
    };
    claim_job_for_run(store, job_id, &plan.to_string())?;
    job = get_job(store, job_id)?.ok_or_else(|| anyhow!("job id={} disappeared", job_id))?;
    if job.status == JOB_STATUS_QUEUED {
        return get_job_detail(store, job_id);
    }
    append_job_event(
        store,
        job_id,
        "status",
        serde_json::json!({ "status": JOB_STATUS_RUNNING }),
    )?;

    let (mut runtime, start_index) = load_runtime_state(store, job_id)?;
    let persisted_elapsed_ms = runtime.elapsed_ms;
    let call_started = Instant::now();
    if start_index > 0 {
        append_job_event(
            store,
            job_id,
            "resumed_from_checkpoint",
            serde_json::json!({ "next_step_index": start_index }),
        )?;
    }
    for (index, phase) in JOB_PHASES.iter().enumerate().skip(start_index) {
        let current =
            get_job(store, job_id)?.ok_or_else(|| anyhow!("job id={} disappeared", job_id))?;
        if current.status == JOB_STATUS_CANCELLED_PARTIAL {
            return get_job_detail(store, job_id);
        }
        runtime.elapsed_ms =
            persisted_elapsed_ms.saturating_add(call_started.elapsed().as_millis() as u64);
        if let Some(reason) = budget_exhausted_reason(&budget, &runtime, 0, 0) {
            return finish_budget_exhausted(store, job_id, &runtime, &job.goal_contract, &reason);
        }
        let steering = apply_pending_steering_at_boundary(store, job_id, index as i64)?;
        let step_input = serde_json::json!({
            "phase": phase,
            "steering": steering.iter().map(|item| item.message.as_str()).collect::<Vec<_>>()
        });
        let step = recover_or_create_job_step(
            store,
            job_id,
            index as i64,
            phase,
            phase_title(phase),
            &step_input.to_string(),
        )?;
        start_job_step(store, step.id)?;
        let execution = execute_phase(store, router, &job, phase, &runtime, &steering)?;
        let elapsed_after_phase =
            persisted_elapsed_ms.saturating_add(call_started.elapsed().as_millis() as u64);
        let mut next_state = execution.next_state;
        next_state.elapsed_ms = elapsed_after_phase;
        if let Some(reason) = budget_exhausted_reason(
            &budget,
            &runtime,
            execution.tool_calls,
            execution.token_cost,
        ) {
            fail_job_step(store, step.id, &reason)?;
            return finish_budget_exhausted(store, job_id, &runtime, &job.goal_contract, &reason);
        }
        next_state.completed_steps = runtime.completed_steps + 1;
        next_state.tool_calls = runtime.tool_calls + execution.tool_calls;
        next_state.tokens_used = runtime.tokens_used + execution.token_cost;
        let output = execution.output.to_string();
        complete_job_step(store, step.id, &output)?;
        create_job_checkpoint(store, job_id, index as i64, phase, &output, &next_state)?;
        runtime = next_state;
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

        if *phase == "verify" {
            if let Some(capability) = runtime.capability_request.clone() {
                return finish_blocked(store, job_id, &job.goal_contract, &runtime, capability);
            }
            if !runtime.verification_passed {
                let capability = serde_json::json!({
                    "capability": "additional_local_evidence",
                    "reason": runtime.verification_findings.join("; "),
                    "needed_from_user": "Add the missing source material or narrow the goal contract."
                });
                return finish_blocked(store, job_id, &job.goal_contract, &runtime, capability);
            }
        }
    }

    if get_job(store, job_id)?
        .map(|current| current.status == JOB_STATUS_CANCELLED_PARTIAL)
        .unwrap_or(false)
    {
        return get_job_detail(store, job_id);
    }
    finish_completed(store, job_id, &job.goal_contract, &runtime, job.speculative)
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
    if job.status == JOB_STATUS_CANCELLED_PARTIAL {
        return get_job_detail(store, job_id);
    }
    if matches!(
        job.status.as_str(),
        JOB_STATUS_COMPLETED | JOB_STATUS_BLOCKED | JOB_STATUS_BUDGET_EXHAUSTED
    ) {
        return Err(anyhow!("cannot cancel terminal job id={job_id}"));
    }
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
        Some("fresh-context deterministic verification skipped: user cancelled; checkpoints preserved."),
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

#[allow(dead_code)]
pub fn resume_incomplete_jobs(store: &TaskStore) -> Result<Vec<AgentJobDetailDto>> {
    resume_incomplete_jobs_internal(store, None)
}

pub fn resume_incomplete_jobs_with_router(
    store: &TaskStore,
    router: &ModelRouter,
) -> Result<Vec<AgentJobDetailDto>> {
    resume_incomplete_jobs_internal(store, Some(router))
}

fn resume_incomplete_jobs_internal(
    store: &TaskStore,
    router: Option<&ModelRouter>,
) -> Result<Vec<AgentJobDetailDto>> {
    let conn = store.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, task_id, goal_contract, plan_json, budget_json, status, speculative,
                deliverable_json, verification_transcript, capability_request_json,
                error_message, created_at, updated_at
         FROM jobs
         WHERE status = 'running'
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
        resumed.push(run_job_to_completion_internal(store, router, job.id)?);
    }
    resumed.extend(run_pending_jobs_internal(store, router)?);
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

pub fn list_standing_jobs(store: &TaskStore, task_id: Option<i64>) -> Result<Vec<StandingJobDto>> {
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

#[allow(dead_code)]
pub fn run_due_standing_jobs(
    store: &TaskStore,
    event_name: Option<&str>,
) -> Result<Vec<AgentJobDetailDto>> {
    run_due_standing_jobs_internal(store, None, event_name)
}

pub fn run_due_standing_jobs_with_router(
    store: &TaskStore,
    router: &ModelRouter,
    event_name: Option<&str>,
) -> Result<Vec<AgentJobDetailDto>> {
    run_due_standing_jobs_internal(store, Some(router), event_name)
}

fn run_due_standing_jobs_internal(
    store: &TaskStore,
    router: Option<&ModelRouter>,
    event_name: Option<&str>,
) -> Result<Vec<AgentJobDetailDto>> {
    let due = due_standing_jobs(store, event_name)?;
    let mut details = Vec::new();
    for standing in due {
        let detail = match router {
            Some(router) => create_and_run_job_with_router(
                store,
                router,
                standing.task_id,
                &standing.goal_contract,
                None,
                false,
            )?,
            None => create_and_run_job(
                store,
                standing.task_id,
                &standing.goal_contract,
                None,
                false,
            )?,
        };
        record_standing_job_run_receipt(store, &standing, &detail)?;
        if standing.critical
            && detail.job.status == JOB_STATUS_COMPLETED
            && standing_guard_tripped(&detail)
        {
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
    runtime: &RuntimeState,
    speculative: bool,
) -> Result<AgentJobDetailDto> {
    let source_ledger = runtime
        .evidence
        .iter()
        .map(|chunk| {
            serde_json::json!({
                "chunk_id": chunk.chunk_id,
                "artifact_id": chunk.artifact_id,
                "file_name": chunk.file_name,
                "position_index": chunk.position_index
            })
        })
        .collect::<Vec<_>>();
    let verification = format!(
        "fresh-context deterministic verification: checked {} source chunk(s); {}",
        source_ledger.len(),
        if runtime.verification_findings.is_empty() {
            "no unsupported local claims found".to_string()
        } else {
            runtime.verification_findings.join("; ")
        }
    );
    let draft = runtime
        .draft
        .clone()
        .unwrap_or_else(|| "No deliverable text was produced.".to_string());
    let deliverable = serde_json::json!({
        "assessment": format!(
            "Completed from {} local source chunk(s); verification was run in a fresh read of the task context.",
            source_ledger.len()
        ),
        "goal_contract": goal_contract,
        "deliverable": draft,
        "verified": true,
        "verification": verification,
        "source_ledger": source_ledger,
        "applied_steering": runtime.applied_steering,
        "not_done": runtime.verification_findings,
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
    if !speculative {
        let conversation_text = format!(
            "{}\n\n{}",
            deliverable["assessment"]
                .as_str()
                .unwrap_or("Job completed."),
            deliverable["deliverable"].as_str().unwrap_or_default()
        );
        store.append_chat_message(
            get_job(store, job_id)?
                .ok_or_else(|| anyhow!("job id={} missing during delivery", job_id))?
                .task_id,
            "assistant",
            "agent_job",
            MessageKind::AssistantAnswer,
            &conversation_text,
        )?;
    }
    promote_next_queued_job(store)?;
    get_job_detail(store, job_id)
}

fn finish_blocked(
    store: &TaskStore,
    job_id: i64,
    goal_contract: &str,
    runtime: &RuntimeState,
    capability_request: serde_json::Value,
) -> Result<AgentJobDetailDto> {
    let reason = capability_request
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("missing evidence or capability");
    let verification =
        format!("fresh-context deterministic verification: couldn't verify, here's why: {reason}.");
    let deliverable = serde_json::json!({
        "assessment": "I couldn't verify this job against the goal contract.",
        "deliverable": runtime.draft.clone().unwrap_or_else(|| format!("Partial work only for: {goal_contract}")),
        "verified": false,
        "honesty": format!("couldn't verify, here's why: {reason}"),
        "source_ledger": runtime.evidence.iter().map(|chunk| chunk.file_name.as_str()).collect::<Vec<_>>(),
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
        Some(&verification),
        Some(&capability_request.to_string()),
        Some("couldn't verify requested job"),
    )?;
    append_job_event(
        store,
        job_id,
        "blocked_capability_request",
        capability_request,
    )?;
    let blocked_job = get_job(store, job_id)?
        .ok_or_else(|| anyhow!("job id={} missing during blocked delivery", job_id))?;
    if !blocked_job.speculative {
        store.append_chat_message(
            blocked_job.task_id,
            "assistant",
            "agent_job",
            MessageKind::AssistantAnswer,
            deliverable["honesty"]
                .as_str()
                .unwrap_or("I couldn't verify this job."),
        )?;
    }
    promote_next_queued_job(store)?;
    get_job_detail(store, job_id)
}

fn finish_budget_exhausted(
    store: &TaskStore,
    job_id: i64,
    runtime: &RuntimeState,
    goal_contract: &str,
    reason: &str,
) -> Result<AgentJobDetailDto> {
    let verification = format!(
        "fresh-context deterministic verification skipped: budget exhausted before mandatory verification completed ({reason})."
    );
    let deliverable = serde_json::json!({
        "assessment": "Budget exhausted before I could fully verify the delegated job.",
        "deliverable": runtime.draft.clone().unwrap_or_else(|| format!("Partial deliverable after {} completed steps for: {goal_contract}", runtime.completed_steps)),
        "verified": false,
        "honesty": format!("budget exhausted mid-job ({reason}); partial deliverable only"),
        "budget_usage": {
            "completed_steps": runtime.completed_steps,
            "tool_calls": runtime.tool_calls,
            "tokens": runtime.tokens_used,
            "elapsed_ms": runtime.elapsed_ms
        },
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
        Some(&verification),
        capability_request_json.as_deref(),
        Some("budget exhausted before verification completed"),
    )?;
    append_job_event(
        store,
        job_id,
        "budget_exhausted",
        serde_json::json!({
            "completed_steps": runtime.completed_steps,
            "tool_calls": runtime.tool_calls,
            "tokens": runtime.tokens_used,
            "elapsed_ms": runtime.elapsed_ms,
            "reason": reason
        }),
    )?;
    let budget_job = get_job(store, job_id)?
        .ok_or_else(|| anyhow!("job id={} missing during budget delivery", job_id))?;
    if !budget_job.speculative {
        store.append_chat_message(
            budget_job.task_id,
            "assistant",
            "agent_job",
            MessageKind::AssistantAnswer,
            deliverable["honesty"]
                .as_str()
                .unwrap_or("Budget exhausted."),
        )?;
    }
    promote_next_queued_job(store)?;
    get_job_detail(store, job_id)
}

fn parse_budget(raw: Option<&str>) -> Result<JobBudget> {
    match raw.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => {
            let budget: JobBudget =
                serde_json::from_str(value).context("invalid agent job budget JSON")?;
            if budget.max_steps == 0 {
                return Err(anyhow!("job budget max_steps must be greater than zero"));
            }
            if budget.max_wall_seconds == 0 {
                return Err(anyhow!(
                    "job budget max_wall_seconds must be greater than zero"
                ));
            }
            if budget.max_tokens == 0 {
                return Err(anyhow!("job budget max_tokens must be greater than zero"));
            }
            if budget.max_steps > 1_000
                || budget.max_tool_calls > 10_000
                || budget.max_wall_seconds > 7 * 24 * 60 * 60
                || budget.max_tokens > 10_000_000
            {
                return Err(anyhow!("job budget exceeds hardened runtime limits"));
            }
            Ok(budget)
        }
        None => Ok(JobBudget::default()),
    }
}

fn build_plan_json(store: &TaskStore, goal_contract: &str, speculative: bool) -> serde_json::Value {
    // apex d8: speculative jobs advertise only the read-only registry, so the
    // scheduler boundary never hands them a mutating tool.
    let static_tools = if speculative {
        speculative_tool_registry()
    } else {
        tool_registry_v1()
    };
    let mut tool_registry = static_tools
        .into_iter()
        .filter_map(|tool| serde_json::to_value(tool).ok())
        .collect::<Vec<_>>();
    if let Ok(connections) = crate::tool_bus::list_tool_connections(store) {
        for connection in connections
            .into_iter()
            .filter(|connection| connection.enabled)
        {
            let read_only = connection.scopes.iter().any(|scope| scope == "read_only");
            if speculative && !read_only {
                continue;
            }
            if let Ok(tools) = crate::tool_bus::list_connection_tools(store, connection.id) {
                tool_registry.extend(tools.into_iter().map(|tool| {
                    serde_json::json!({
                        "name": format!("mcp::{}::{}", connection.name, tool.tool_name),
                        "description": tool.description,
                        "action_class": null,
                        "read_only": read_only,
                        "connection": connection.name,
                        "mcp_tool": tool.tool_name,
                    })
                }));
            }
        }
    }
    serde_json::json!({
        "loop": JOB_PHASES,
        "goal_contract": goal_contract,
        "speculative": speculative,
        "read_only": speculative,
        "tool_registry": tool_registry,
        "router_tier": "craft",
        "executor": "craft_router_with_unavailable_provider_fallback",
        "verification_required": true,
        "delivery_contract": "assessment_first_card_with_action_bus_placement_proposal"
    })
}

fn execute_phase(
    store: &TaskStore,
    router: Option<&ModelRouter>,
    job: &AgentJobDto,
    phase: &str,
    runtime: &RuntimeState,
    steering: &[AgentJobSteeringDto],
) -> Result<PhaseExecution> {
    let goal = goal_instruction(&job.goal_contract);
    let mut next = runtime.clone();
    for item in steering {
        if !next.applied_steering.contains(&item.message) {
            next.applied_steering.push(item.message.clone());
        }
    }

    let (output, tool_calls) = match phase {
        "plan" => (
            serde_json::json!({
                "phase": phase,
                "goal": goal,
                "steps": JOB_PHASES,
                "tool_policy": if job.speculative { "read_only" } else { "proposal_only_for_mutations" },
                "status": "completed"
            }),
            0,
        ),
        "act" => {
            let context = collect_local_context(store, job.task_id, &goal, &next.applied_steering)?;
            next.task_snapshot = Some(format!(
                "{} Recent context: {}",
                context.task_summary,
                context.recent_messages.join(" | ")
            ));
            next.evidence = context.evidence.clone();
            next.observations = vec![
                format!("read {} task artifact(s)", context.artifact_names.len()),
                format!(
                    "retrieved {} relevant local chunk(s)",
                    context.evidence.len()
                ),
                format!(
                    "read {} recent conversation turn(s)",
                    context.recent_messages.len()
                ),
            ];
            (
                serde_json::json!({
                    "phase": phase,
                    "tools_executed": [TOOL_LOCAL_RETRIEVAL, TOOL_DOCUMENT_MODEL_READ, TOOL_SNAPSHOT_READ],
                    "artifact_names": context.artifact_names,
                    "evidence": context.evidence,
                    "snapshot": next.task_snapshot,
                    "status": "completed"
                }),
                3,
            )
        }
        "observe" => (
            serde_json::json!({
                "phase": phase,
                "observations": next.observations,
                "source_count": next.evidence.len(),
                "status": "completed"
            }),
            0,
        ),
        "revise" => {
            let draft = compose_routed_deliverable(
                router,
                &goal,
                &next.evidence,
                next.task_snapshot.as_deref(),
                &next.applied_steering,
            )?;
            let used_router = router
                .map(|router| router.tier_available(Tier::Craft))
                .unwrap_or(false);
            next.draft = Some(draft.clone());
            (
                serde_json::json!({
                    "phase": phase,
                    "draft": draft,
                    "applied_steering": next.applied_steering,
                    "source_chunk_ids": next.evidence.iter().map(|chunk| chunk.chunk_id).collect::<Vec<_>>(),
                    "craft_router_used": used_router,
                    "status": "completed"
                }),
                0,
            )
        }
        "verify" => {
            let (passed, findings, capability_request) = verify_with_fresh_context(
                store,
                job.task_id,
                &goal,
                next.draft.as_deref().unwrap_or_default(),
                &next.evidence,
            )?;
            next.verification_passed = passed;
            next.verification_findings = findings.clone();
            next.capability_request = capability_request.clone();
            (
                serde_json::json!({
                    "phase": phase,
                    "fresh_context": true,
                    "passed": passed,
                    "findings": findings,
                    "capability_request": capability_request,
                    "status": "completed"
                }),
                2,
            )
        }
        "deliver" => (
            serde_json::json!({
                "phase": phase,
                "assessment_first": true,
                "deliverable": next.draft,
                "verified": next.verification_passed,
                "status": "completed"
            }),
            0,
        ),
        other => return Err(anyhow!("unknown agent runtime phase '{other}'")),
    };
    let token_cost = estimate_tokens(&format!(
        "{}\n{}\n{}",
        goal,
        next.applied_steering.join("\n"),
        output
    ));
    Ok(PhaseExecution {
        output,
        next_state: next,
        tool_calls,
        token_cost,
    })
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

fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4).max(1)
}

fn goal_instruction(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|value| {
            value
                .get("instruction")
                .or_else(|| value.get("goal"))
                .or_else(|| value.get("title"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| raw.trim().to_string())
}

fn goal_terms(goal: &str, steering: &[String]) -> HashSet<String> {
    const STOP: &[&str] = &[
        "the",
        "and",
        "from",
        "this",
        "that",
        "with",
        "into",
        "about",
        "against",
        "draft",
        "revise",
        "check",
        "make",
        "please",
        "local",
        "section",
        "paragraph",
        "notes",
    ];
    format!("{} {}", goal, steering.join(" "))
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .filter(|term| term.len() >= 3 && !STOP.contains(term))
        .map(str::to_string)
        .collect()
}

fn collect_local_context(
    store: &TaskStore,
    task_id: i64,
    goal: &str,
    steering: &[String],
) -> Result<LocalContext> {
    let task_summary = store.get_task_summary(task_id)?.summary_text;
    let recent_messages = store
        .list_recent_chat_messages(task_id, MAX_RECENT_MESSAGES)?
        .into_iter()
        .map(|message| format!("{}: {}", message.role, message.content.trim()))
        .collect::<Vec<_>>();
    let artifacts = store.list_artifacts(task_id)?;
    let artifact_names = artifacts
        .iter()
        .map(|artifact| artifact.file_name.clone())
        .collect::<Vec<_>>();
    let terms = goal_terms(goal, steering);
    let conn = store.connect()?;
    let mut stmt = conn.prepare(
        "SELECT c.id, c.artifact_id, a.file_name, c.position_index, c.chunk_text
         FROM artifact_chunks c
         JOIN artifacts a ON a.id = c.artifact_id
         WHERE c.task_id = ?1
         ORDER BY c.artifact_id ASC, c.position_index ASC, c.id ASC",
    )?;
    let mut scored = stmt
        .query_map(params![task_id], |row| {
            Ok(EvidenceChunk {
                chunk_id: row.get(0)?,
                artifact_id: row.get(1)?,
                file_name: row.get(2)?,
                position_index: row.get(3)?,
                text: row
                    .get::<_, String>(4)?
                    .chars()
                    .take(MAX_EVIDENCE_CHARS)
                    .collect(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(|chunk| {
            let lower = chunk.text.to_ascii_lowercase();
            let score = terms
                .iter()
                .filter(|term| lower.contains(term.as_str()))
                .count();
            (score, chunk)
        })
        .collect::<Vec<_>>();
    scored.sort_by(|(score_a, chunk_a), (score_b, chunk_b)| {
        score_b
            .cmp(score_a)
            .then_with(|| chunk_a.artifact_id.cmp(&chunk_b.artifact_id))
            .then_with(|| chunk_a.position_index.cmp(&chunk_b.position_index))
    });
    let evidence = scored
        .into_iter()
        .take(MAX_EVIDENCE_CHUNKS)
        .map(|(_, chunk)| chunk)
        .collect();
    Ok(LocalContext {
        task_summary,
        recent_messages,
        evidence,
        artifact_names,
    })
}

fn compose_grounded_deliverable(
    goal: &str,
    evidence: &[EvidenceChunk],
    snapshot: Option<&str>,
    steering: &[String],
) -> String {
    let lower_goal = goal.to_ascii_lowercase();
    let concise = steering
        .iter()
        .any(|message| message.to_ascii_lowercase().contains("concise"));
    let two_paragraphs = steering.iter().any(|message| {
        let lower = message.to_ascii_lowercase();
        lower.contains("two paragraph") || lower.contains("2 paragraph")
    });
    let mut extracts = evidence
        .iter()
        .map(|chunk| chunk.text.trim().to_string())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();
    if concise && extracts.len() > 3 {
        extracts.truncate(3);
    }
    let web_evidence = evidence
        .iter()
        .any(|chunk| chunk.file_name.starts_with("https://"));
    let mut draft =
        if lower_goal.contains("citation") || lower_goal.contains("bibliograph") || web_evidence {
            evidence
                .iter()
                .map(|chunk| format!("- [{}] {}", chunk.file_name, chunk.text.trim()))
                .collect::<Vec<_>>()
                .join("\n")
        } else if !extracts.is_empty() {
            if two_paragraphs {
                let midpoint = extracts.len().div_ceil(2).max(1);
                let first = extracts[..midpoint].join(" ");
                let second = if midpoint < extracts.len() {
                    extracts[midpoint..].join(" ")
                } else {
                    format!("This evidence directly supports the requested focus: {goal}")
                };
                format!("{first}\n\n{second}")
            } else {
                extracts.join(" ")
            }
        } else {
            format!(
                "Current task assessment for '{}': {}",
                goal,
                snapshot.unwrap_or("no local task snapshot is available")
            )
        };
    let generic_steering = steering
        .iter()
        .filter(|message| {
            let lower = message.to_ascii_lowercase();
            !lower.contains("two paragraph")
                && !lower.contains("2 paragraph")
                && !lower.contains("concise")
        })
        .map(|message| message.trim())
        .filter(|message| !message.is_empty())
        .collect::<Vec<_>>();
    if !generic_steering.is_empty() {
        draft.push_str("\n\nRevision direction applied: ");
        draft.push_str(&generic_steering.join("; "));
    }
    draft
}

fn compose_routed_deliverable(
    router: Option<&ModelRouter>,
    goal: &str,
    evidence: &[EvidenceChunk],
    snapshot: Option<&str>,
    steering: &[String],
) -> Result<String> {
    let Some(router) = router else {
        return Ok(compose_grounded_deliverable(
            goal, evidence, snapshot, steering,
        ));
    };
    if !router.tier_available(Tier::Craft) {
        return Ok(compose_grounded_deliverable(
            goal, evidence, snapshot, steering,
        ));
    }
    let evidence = evidence
        .iter()
        .map(|chunk| {
            serde_json::json!({
                "chunk_id": chunk.chunk_id,
                "file_name": chunk.file_name,
                "text": chunk.text,
            })
        })
        .collect::<Vec<_>>();
    let input = serde_json::json!({
        "goal_contract": goal,
        "task_snapshot": snapshot,
        "steering": steering,
        "evidence": evidence,
    });
    let draft = router.generate_with(
        Tier::Craft,
        "Produce the requested work using only the supplied local evidence. Follow steering exactly. Do not invent facts, citations, completed actions, or capabilities. Return only the deliverable text.",
        &input.to_string(),
        GenerateOptions {
            temperature: 0.0,
            max_tokens: Some(2_000),
            json_object: false,
            timeout_ms: Some(90_000),
        },
    )?;
    let clean = draft.trim();
    if clean.is_empty() {
        return Err(anyhow!("craft-tier router returned an empty deliverable"));
    }
    Ok(clean.to_string())
}

fn verify_with_fresh_context(
    store: &TaskStore,
    task_id: i64,
    goal: &str,
    draft: &str,
    evidence: &[EvidenceChunk],
) -> Result<(bool, Vec<String>, Option<serde_json::Value>)> {
    let artifacts = store.list_artifacts(task_id)?;
    if let Some(capability) = missing_capability(goal, &artifacts, evidence) {
        let reason = capability
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("missing capability")
            .to_string();
        return Ok((false, vec![reason], Some(capability)));
    }
    let conn = store.connect()?;
    let mut stmt = conn.prepare("SELECT id FROM artifact_chunks WHERE task_id = ?1")?;
    let fresh_ids = stmt
        .query_map(params![task_id], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    let mut findings = Vec::new();
    for chunk in evidence {
        if !fresh_ids.contains(&chunk.chunk_id) {
            findings.push(format!(
                "source chunk {} from {} changed or disappeared before delivery",
                chunk.chunk_id, chunk.file_name
            ));
        }
    }
    if draft.trim().is_empty() {
        findings.push("deliverable is empty".to_string());
    }
    Ok((findings.is_empty(), findings, None))
}

fn missing_capability(
    goal: &str,
    artifacts: &[crate::models::ArtifactDto],
    evidence: &[EvidenceChunk],
) -> Option<serde_json::Value> {
    let lower = goal.to_ascii_lowercase();
    if lower.contains("live web")
        || lower.contains("online source")
        || lower.contains("search the web")
        || lower.contains("internet")
    {
        return Some(serde_json::json!({
            "capability": "web_research",
            "reason": "the goal requires live web research, but this local runtime has no web tool",
            "needed_from_user": "Enable the web research connection or provide local source files."
        }));
    }
    if lower.contains("external account") || lower.contains("account without access") {
        return Some(serde_json::json!({
            "capability": "external_account_access",
            "reason": "the requested external account is not connected",
            "needed_from_user": "Connect the account or provide an exported local source."
        }));
    }
    if lower.contains("pdf")
        && !artifacts
            .iter()
            .any(|artifact| artifact.file_extension.eq_ignore_ascii_case("pdf"))
    {
        return Some(serde_json::json!({
            "capability": "source_pdf",
            "reason": "the goal requires a source PDF, but no PDF is attached to this task",
            "needed_from_user": "Attach the source PDF."
        }));
    }
    let requires_local_sources = [
        "local notes",
        "local outline",
        "local draft",
        "bibliography",
        "rubric",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    if requires_local_sources && evidence.is_empty() {
        return Some(serde_json::json!({
            "capability": "local_source_material",
            "reason": "the goal names local source material, but the task contains no readable chunks",
            "needed_from_user": "Import the notes, draft, rubric, or bibliography into the task."
        }));
    }
    None
}

fn budget_exhausted_reason(
    budget: &JobBudget,
    runtime: &RuntimeState,
    additional_tool_calls: usize,
    additional_tokens: usize,
) -> Option<String> {
    if runtime.completed_steps >= budget.max_steps {
        return Some(format!("step limit {} reached", budget.max_steps));
    }
    if runtime.tool_calls.saturating_add(additional_tool_calls) > budget.max_tool_calls {
        return Some(format!(
            "tool-call limit {} would be exceeded",
            budget.max_tool_calls
        ));
    }
    if runtime.tokens_used.saturating_add(additional_tokens) > budget.max_tokens {
        return Some(format!(
            "token limit {} would be exceeded",
            budget.max_tokens
        ));
    }
    if runtime.elapsed_ms >= budget.max_wall_seconds.saturating_mul(1_000) {
        return Some(format!(
            "wall-time limit {}s reached",
            budget.max_wall_seconds
        ));
    }
    None
}

fn claim_job_for_run(store: &TaskStore, job_id: i64, plan_json: &str) -> Result<()> {
    let mut conn = store.connect()?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("failed to claim agent job")?;
    let status = tx
        .query_row(
            "SELECT status FROM jobs WHERE id = ?1",
            params![job_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| anyhow!("job id={} not found", job_id))?;
    if status == JOB_STATUS_QUEUED {
        tx.commit()?;
        return Ok(());
    }
    if matches!(
        status.as_str(),
        JOB_STATUS_COMPLETED
            | JOB_STATUS_BLOCKED
            | JOB_STATUS_BUDGET_EXHAUSTED
            | JOB_STATUS_CANCELLED_PARTIAL
    ) {
        tx.commit()?;
        return Ok(());
    }
    tx.execute(
        "UPDATE jobs
         SET plan_json = ?1, status = ?2,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?3",
        params![plan_json, JOB_STATUS_RUNNING, job_id],
    )?;
    tx.commit().context("failed to commit job claim")
}

fn load_runtime_state(store: &TaskStore, job_id: i64) -> Result<(RuntimeState, usize)> {
    let checkpoints = list_job_checkpoints(store, job_id)?;
    let Some(latest) = checkpoints.last() else {
        return Ok((RuntimeState::default(), 0));
    };
    let envelope =
        serde_json::from_str::<serde_json::Value>(&latest.state_json).unwrap_or_default();
    let mut runtime = envelope
        .get("runtime_state")
        .cloned()
        .and_then(|value| serde_json::from_value::<RuntimeState>(value).ok())
        .unwrap_or_else(|| RuntimeState {
            completed_steps: (latest.step_index + 1).max(0) as usize,
            ..RuntimeState::default()
        });
    runtime.completed_steps = runtime
        .completed_steps
        .max((latest.step_index + 1).max(0) as usize);
    Ok((runtime, (latest.step_index + 1).max(0) as usize))
}

fn recover_or_create_job_step(
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
         VALUES (?1, ?2, ?3, 'pending', ?4, ?5)
         ON CONFLICT(job_id, step_index) DO UPDATE SET
             phase = excluded.phase,
             status = 'pending',
             title = excluded.title,
             input_json = excluded.input_json,
             output_json = NULL,
             error_message = NULL,
             started_at = NULL,
             completed_at = NULL",
        params![job_id, step_index, phase, title, input_json],
    )
    .context("failed to create or recover job step")?;
    drop(conn);
    get_job_step_by_index(store, job_id, step_index)?
        .ok_or_else(|| anyhow!("job step {job_id}:{step_index} missing after upsert"))
}

fn fail_job_step(store: &TaskStore, step_id: i64, error: &str) -> Result<()> {
    let conn = store.connect()?;
    conn.execute(
        "UPDATE job_steps
         SET status = 'failed', error_message = ?1,
             completed_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?2",
        params![error, step_id],
    )?;
    Ok(())
}

#[allow(dead_code)]
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
        "SELECT COUNT(*) FROM jobs WHERE status IN (?1, ?2)",
        params![JOB_STATUS_PENDING, JOB_STATUS_RUNNING],
        |row| row.get(0),
    )
    .context("failed to count active jobs")
}

fn promote_next_queued_job(store: &TaskStore) -> Result<()> {
    if running_job_count(store)? >= MAX_RUNNING_JOBS {
        return Ok(());
    }
    let mut conn = store.connect()?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("failed to promote queued job")?;
    let active: i64 = tx.query_row(
        "SELECT COUNT(*) FROM jobs WHERE status IN (?1, ?2)",
        params![JOB_STATUS_PENDING, JOB_STATUS_RUNNING],
        |row| row.get(0),
    )?;
    if active >= MAX_RUNNING_JOBS {
        tx.commit()?;
        return Ok(());
    }
    let next_id = tx
        .query_row(
            "SELECT id FROM jobs WHERE status = ?1 ORDER BY id ASC LIMIT 1",
            params![JOB_STATUS_QUEUED],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .context("failed to query next queued job")?;
    if let Some(job_id) = next_id {
        tx.execute(
            "UPDATE jobs
             SET status = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?2",
            params![JOB_STATUS_PENDING, job_id],
        )
        .context("failed to promote queued job")?;
        tx.commit().context("failed to commit queue promotion")?;
        append_job_event(
            store,
            job_id,
            "queue_promoted",
            serde_json::json!({ "status": JOB_STATUS_PENDING }),
        )?;
    } else {
        tx.commit()?;
    }
    Ok(())
}

/// Drain jobs that have reserved a pending slot. Integration should call this
/// from a lightweight background worker; it is also deterministic for startup
/// recovery and tests. Queue promotion remains FIFO.
#[allow(dead_code)]
pub fn run_pending_jobs(store: &TaskStore) -> Result<Vec<AgentJobDetailDto>> {
    run_pending_jobs_internal(store, None)
}

fn run_pending_jobs_internal(
    store: &TaskStore,
    router: Option<&ModelRouter>,
) -> Result<Vec<AgentJobDetailDto>> {
    let mut completed = Vec::new();
    loop {
        let ids = {
            let conn = store.connect()?;
            let mut stmt =
                conn.prepare("SELECT id FROM jobs WHERE status = ?1 ORDER BY id ASC LIMIT ?2")?;
            let ids = stmt
                .query_map(params![JOB_STATUS_PENDING, MAX_RUNNING_JOBS], |row| {
                    row.get(0)
                })?
                .collect::<rusqlite::Result<Vec<i64>>>()?;
            ids
        };
        if ids.is_empty() {
            break;
        }
        for id in ids {
            completed.push(run_job_to_completion_internal(store, router, id)?);
        }
    }
    Ok(completed)
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
    runtime_state: &RuntimeState,
) -> Result<AgentJobCheckpointDto> {
    let state = serde_json::json!({
        "job_id": job_id,
        "step_index": step_index,
        "phase": phase,
        "step_output": output_json,
        "runtime_state": runtime_state,
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

fn get_job_step_by_index(
    store: &TaskStore,
    job_id: i64,
    step_index: i64,
) -> Result<Option<AgentJobStepDto>> {
    let conn = store.connect()?;
    conn.query_row(
        "SELECT id, job_id, step_index, phase, status, title, input_json, output_json,
                error_message, started_at, completed_at
         FROM job_steps WHERE job_id = ?1 AND step_index = ?2",
        params![job_id, step_index],
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

fn due_standing_jobs(store: &TaskStore, event_name: Option<&str>) -> Result<Vec<StandingJobDto>> {
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
    let now = Local::now();
    let mut day = now.date_naive();
    for _ in 0..3 {
        let naive = day.and_time(time);
        if let Some(candidate) = Local
            .from_local_datetime(&naive)
            .earliest()
            .filter(|candidate| *candidate > now)
        {
            return Ok(candidate
                .with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Secs, true));
        }
        day += Duration::days(1);
    }
    Err(anyhow!(
        "could not resolve local daily schedule '{clock}' across DST"
    ))
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
    let receipt_status = if detail.job.status == JOB_STATUS_COMPLETED {
        "applied"
    } else {
        detail.job.status.as_str()
    };
    let receipt = store.create_action_receipt(
        standing.task_id,
        &class,
        STANDING_JOB_RECEIPT_SURFACE,
        crate::trust::TRUST_LEVEL_L1,
        if detail.job.status == JOB_STATUS_COMPLETED {
            "Standing job run completed"
        } else {
            "Standing job run scheduled or stopped before completion"
        },
        &payload.to_string(),
        receipt_status,
        None,
        None,
    )?;
    crate::trust::record_receipt_outcome(store, &receipt)?;
    Ok(())
}

fn standing_guard_tripped(detail: &AgentJobDetailDto) -> bool {
    let deliverable = detail
        .job
        .deliverable_json
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    [
        "critical:",
        "deadline collision",
        "missing citation",
        "data loss",
        "verification failed",
        "inconsistent citation",
    ]
    .iter()
    .any(|needle| deliverable.contains(needle))
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
    use crate::store::ChunkEmbeddingInput;

    fn store() -> (TempDir, TaskStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        (dir, store)
    }

    fn seed_artifact(store: &TaskStore, task_id: i64, file_name: &str, chunks: &[&str]) {
        let inputs = chunks
            .iter()
            .enumerate()
            .map(|(index, text)| ChunkEmbeddingInput {
                chunk_text: (*text).to_string(),
                position_index: index as i64,
                embedding: Vec::new(),
                embedding_model: "fixture".to_string(),
            })
            .collect::<Vec<_>>();
        store
            .insert_artifact_with_chunks(
                task_id,
                file_name,
                file_name.rsplit('.').next().unwrap_or("txt"),
                file_name,
                file_name,
                &inputs,
            )
            .unwrap();
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
    fn d5_craft_router_falls_back_only_when_effective_provider_is_unavailable() {
        let mut config = crate::model_router::RouterConfig::default();
        config.craft = crate::model_router::TierConfig {
            provider: crate::model_router::ProviderKind::Local,
            model: crate::local_runtime::LOCAL_REASONING_MODEL_ID.to_string(),
        };
        let router = ModelRouter::new(config);
        assert!(!router.tier_available(Tier::Craft));

        let fallback = compose_routed_deliverable(
            Some(&router),
            "summarize",
            &[],
            Some("local snapshot"),
            &[],
        )
        .unwrap();
        assert!(fallback.contains("local snapshot"));
    }

    #[test]
    fn d5_connected_mcp_tools_enter_runtime_registry_with_speculation_scope() {
        let (_dir, store) = store();
        let connection = crate::tool_bus::add_tool_connection(
            &store,
            "research",
            crate::tool_bus::TRANSPORT_LOOPBACK,
            "loopback://",
            &[],
        )
        .unwrap();
        crate::tool_bus::register_connection_tools(
            &store,
            connection.id,
            &[("search".to_string(), "search sources".to_string())],
        )
        .unwrap();
        let normal = build_plan_json(&store, "research", false).to_string();
        let speculative = build_plan_json(&store, "research", true).to_string();
        assert!(normal.contains("mcp::research::search"));
        assert!(!speculative.contains("mcp::research::search"));
    }

    #[test]
    fn d5_unattended_job_runs_plan_act_observe_revise_verify_deliver() {
        let (_dir, store) = store();
        let task = store.create_task("d5").unwrap();
        seed_artifact(
            &store,
            task.id,
            "notes.md",
            &["The counterargument is strongest when it addresses cost and implementation risk."],
        );
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
            .contains("fresh-context deterministic verification"));
        let deliverable = detail.job.deliverable_json.unwrap();
        assert!(deliverable.contains("implementation risk"));
        assert!(deliverable.contains("notes.md"));
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
            .contains("external_account_access"));
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
    fn d5_tool_and_token_budgets_are_enforced_and_malformed_budget_is_rejected() {
        let (_dir, store) = store();
        let task = store.create_task("d5 strict budgets").unwrap();
        assert!(create_job(&store, task.id, "x", Some("not-json"), false).is_err());
        let budget = serde_json::json!({
            "max_steps": 8,
            "max_tool_calls": 2,
            "max_wall_seconds": 120,
            "max_tokens": 8000
        });
        let detail = create_and_run_job(
            &store,
            task.id,
            "Assess the current task.",
            Some(&budget.to_string()),
            false,
        )
        .unwrap();
        assert_eq!(detail.job.status, JOB_STATUS_BUDGET_EXHAUSTED);
        assert!(detail
            .job
            .error_message
            .unwrap()
            .contains("budget exhausted"));
    }

    #[test]
    fn d6_steering_is_applied_at_step_boundary_and_checkpointed() {
        let (_dir, store) = store();
        let task = store.create_task("d6 steering").unwrap();
        seed_artifact(
            &store,
            task.id,
            "notes.md",
            &[
                "Evidence one supports the claim.",
                "Evidence two limits the claim.",
            ],
        );
        let job = create_job(
            &store,
            task.id,
            "Draft a short note from local notes.",
            None,
            false,
        )
        .unwrap();

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
        let deliverable: serde_json::Value =
            serde_json::from_str(detail.job.deliverable_json.as_deref().unwrap()).unwrap();
        assert_eq!(
            deliverable["deliverable"]
                .as_str()
                .unwrap()
                .matches("\n\n")
                .count(),
            1,
            "steering must change the delivered artifact to two paragraphs"
        );
    }

    #[test]
    fn d6_resume_continues_after_last_completed_checkpoint() {
        let (_dir, store) = store();
        let task = store.create_task("d6 resume").unwrap();
        let job = create_job(&store, task.id, "Resume the interrupted job.", None, false).unwrap();
        update_job_plan_and_status(&store, job.id, "[]", JOB_STATUS_RUNNING).unwrap();
        let execution =
            execute_phase(&store, None, &job, "plan", &RuntimeState::default(), &[]).unwrap();
        let mut state = execution.next_state;
        state.completed_steps = 1;
        state.tokens_used = execution.token_cost;
        let output = execution.output.to_string();
        let step = recover_or_create_job_step(
            &store,
            job.id,
            0,
            "plan",
            phase_title("plan"),
            &serde_json::json!({ "phase": "plan" }).to_string(),
        )
        .unwrap();
        start_job_step(&store, step.id).unwrap();
        complete_job_step(&store, step.id, &output).unwrap();
        create_job_checkpoint(&store, job.id, 0, "plan", &output, &state).unwrap();
        // Simulate a crash after the next step was inserted and marked running,
        // but before it completed or checkpointed. Recovery must reuse the row,
        // not violate UNIQUE(job_id, step_index).
        let interrupted =
            recover_or_create_job_step(&store, job.id, 1, "act", phase_title("act"), "{}").unwrap();
        start_job_step(&store, interrupted.id).unwrap();

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
            &RuntimeState {
                completed_steps: 1,
                ..RuntimeState::default()
            },
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
        seed_artifact(
            &store,
            task.id,
            "bibliography.md",
            &["CRITICAL: missing citation for the central reliability claim."],
        );
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
        seed_artifact(
            &store,
            task.id,
            "bibliography.md",
            &["Smith 2024. Reliable Systems. Journal of Verification."],
        );
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

    #[test]
    fn d8_speculative_job_is_read_only_and_rejects_mutations() {
        let (_dir, store) = store();
        let task = store.create_task("d8 speculative").unwrap();

        // the speculative registry drops every mutating tool.
        assert!(speculative_tool_registry()
            .iter()
            .all(|tool| tool.read_only));
        assert!(speculative_tool_registry().len() < tool_registry_v1().len());

        let job = create_job(&store, task.id, "Prep the methods answer.", None, true).unwrap();
        assert!(job.speculative);
        // runtime guard rejects any mutation class at any trust level.
        for class in ["file.write", "doc.suggest", "doc.replace", "email.draft"] {
            assert!(
                guard_speculative_action(&job, class).is_err(),
                "speculative job must reject {class}"
            );
        }

        // a non-speculative job is unaffected.
        let normal = create_job(&store, task.id, "Real job.", None, false).unwrap();
        assert!(guard_speculative_action(&normal, "file.write").is_ok());

        // the run plan advertises only the read-only registry.
        let detail = run_job_to_completion(&store, job.id).unwrap();
        assert_eq!(detail.job.status, JOB_STATUS_COMPLETED);
        assert!(detail.job.plan_json.contains("\"read_only\":true"));
        assert!(!detail.job.plan_json.contains(TOOL_FILE_PROPOSAL_BUS));
        assert!(!detail.job.plan_json.contains(TOOL_ACTION_PROPOSAL_BUS));
    }
}
