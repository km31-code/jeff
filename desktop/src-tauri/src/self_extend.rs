// apex d9: self-extension. When Jeff hits a capability wall (an action-bus
// no_adapter / unsupported_surface rejection, or a blocked job), it records a
// capability gap. For a recurring gap it drafts a ToolSpec, generates the tool
// code (Craft-tier; deterministic template fallback), stages it, dry-run tests
// it, and offers an approval card. On approval the tool installs and registers
// as tool.custom.<name>.
//
// Invariants (Part IV), all enforced and tested:
// - Self-built tools are permanently L1 (propose-only): tool.custom.* is
//   hard-capped in trust.rs; Jeff can grow its own hands, never its own
//   autonomy.
// - Every tool run produces an action receipt.
// - A killed tool is removed from the active registry immediately; subsequent
//   attempts degrade to guided fallback.
// - applescript tools may only target apps in their declared allowlist.
// - text_script tools run in a deny-by-default operating-system sandbox with
//   no network and workspace-only mutable filesystem access. The static guard
//   remains defense in depth, never the security boundary.

#![cfg_attr(test, allow(dead_code))]

use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
};

#[cfg(target_os = "macos")]
use std::os::unix::process::CommandExt;

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::{
    action_bus::ActionClass,
    models::{CapabilityGapDto, CustomToolDto, CustomToolRunResultDto},
    store::TaskStore,
};

pub const MIN_GAP_OCCURRENCES: i64 = 2;

pub const KIND_APPLESCRIPT: &str = "applescript";
pub const KIND_SITE_ADAPTER: &str = "site_adapter";
pub const KIND_TEXT_SCRIPT: &str = "text_script";

pub const STATUS_STAGED: &str = "staged";
pub const STATUS_INSTALLED: &str = "installed";
pub const STATUS_KILLED: &str = "killed";

pub const RUN_STATUS_APPLIED: &str = "applied";
pub const RUN_STATUS_GUIDED: &str = "guided";
pub const RUN_STATUS_FAILED: &str = "failed";
pub const RUN_STATUS_PENDING_APPROVAL: &str = "pending_approval";
pub const RUN_STATUS_REJECTED: &str = "rejected";

pub const GAP_REASON_NO_ADAPTER: &str = "no_adapter";
pub const GAP_REASON_UNSUPPORTED_SURFACE: &str = "unsupported_surface";

const TEXT_SCRIPT_TIMEOUT_SECS: u64 = 5;
const MAX_TOOL_INPUT_BYTES: usize = 64 * 1024;
const MAX_TOOL_OUTPUT_BYTES: u64 = 1024 * 1024;

// static pre-execution denylist for text_script sandboxing. This is only a
// fast, legible rejection layer; the operating-system sandbox is authoritative.
const BLOCKED_SCRIPT_TOKENS: &[&str] = &[
    "http://",
    "https://",
    "curl",
    "wget",
    "netcat",
    "nc ",
    "socket",
    "ftp",
    "ssh",
    "scp",
    "telnet",
    "/etc/",
    "/usr/local",
    "/var/",
    "~/",
    ".ssh",
    "subprocess",
    "os.system",
    "import socket",
    "requests.",
    "urllib",
    "fetch(",
    "xmlhttprequest",
    "..",
    "sudo",
    "rm -rf",
    "dd if",
    "mkfs",
    ":(){",
    "> /dev",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SelfExtendToolSpec {
    pub name: String,
    pub kind: String,
    pub purpose: String,
    pub target_allowlist: Vec<String>,
    pub inputs: String,
    pub outputs: String,
    pub test_plan: String,
}

// ---- gap detection -------------------------------------------------------

pub fn record_capability_gap(store: &TaskStore, surface: &str, description: &str) -> Result<i64> {
    let surface = surface.trim();
    let description = description.trim();
    if surface.is_empty() || description.is_empty() {
        return Err(anyhow!("capability gap requires a surface and description"));
    }
    let conn = store.connect()?;
    conn.execute(
        "INSERT INTO capability_gaps (surface, description, occurrence_count)
         VALUES (?1, ?2, 1)
         ON CONFLICT(surface, description) DO UPDATE SET
             occurrence_count = occurrence_count + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
        params![surface, description],
    )
    .context("failed to record capability gap")?;
    conn.query_row(
        "SELECT id FROM capability_gaps WHERE surface = ?1 AND description = ?2",
        params![surface, description],
        |row| row.get(0),
    )
    .context("failed to read capability gap id")
}

pub fn list_capability_gaps(store: &TaskStore) -> Result<Vec<CapabilityGapDto>> {
    let conn = store.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, surface, description, occurrence_count, created_at, updated_at
         FROM capability_gaps ORDER BY occurrence_count DESC, id DESC",
    )?;
    let rows = stmt
        .query_map([], capability_gap_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[allow(dead_code)]
pub fn recurring_gaps(store: &TaskStore) -> Result<Vec<CapabilityGapDto>> {
    Ok(list_capability_gaps(store)?
        .into_iter()
        .filter(|gap| gap.occurrence_count >= MIN_GAP_OCCURRENCES)
        .collect())
}

fn get_gap(store: &TaskStore, gap_id: i64) -> Result<Option<CapabilityGapDto>> {
    let conn = store.connect()?;
    conn.query_row(
        "SELECT id, surface, description, occurrence_count, created_at, updated_at
         FROM capability_gaps WHERE id = ?1",
        params![gap_id],
        capability_gap_from_row,
    )
    .optional()
    .map_err(Into::into)
}

// ---- proposal / lifecycle ------------------------------------------------

// draft a ToolSpec for a recurring gap, generate + stage + dry-run the tool.
pub fn propose_tool_for_gap(store: &TaskStore, gap_id: i64) -> Result<CustomToolDto> {
    let gap = get_gap(store, gap_id)?.ok_or_else(|| anyhow!("gap id={gap_id} not found"))?;
    if gap.occurrence_count < MIN_GAP_OCCURRENCES {
        return Err(anyhow!(
            "gap id={gap_id} has {} occurrence(s); need >= {MIN_GAP_OCCURRENCES}",
            gap.occurrence_count
        ));
    }
    let spec = draft_tool_spec(&gap);
    let code = generate_tool_code(&spec);
    stage_tool_files(store, &spec, &code)?;
    let transcript = dry_run_test(store, &spec, &code)?;
    insert_custom_tool(
        store,
        &spec,
        &code,
        &transcript,
        STATUS_STAGED,
        Some(gap_id),
    )
}

// deterministic ToolSpec draft. the Craft-tier drafter is the env-gated upgrade
// that plugs in here; the template keeps the lifecycle testable.
pub fn draft_tool_spec(gap: &CapabilityGapDto) -> SelfExtendToolSpec {
    let kind = infer_kind(&gap.surface);
    let name = sanitize_name(&format!("{}_{}", gap.surface, gap.id));
    let allowlist = if kind == KIND_TEXT_SCRIPT {
        Vec::new()
    } else {
        vec![gap.surface.clone()]
    };
    SelfExtendToolSpec {
        name,
        kind: kind.to_string(),
        purpose: format!("Handle: {}", gap.description),
        target_allowlist: allowlist,
        inputs: "text".to_string(),
        outputs: "text".to_string(),
        test_plan: "hello world -> HELLO WORLD".to_string(),
    }
}

pub fn infer_kind(surface: &str) -> &'static str {
    let lower = surface.to_ascii_lowercase();
    if lower.contains("http")
        || lower.contains(".com")
        || lower.contains("site")
        || lower.contains("docs.google")
    {
        KIND_SITE_ADAPTER
    } else if lower.contains("pages")
        || lower.contains("word")
        || lower.contains("app")
        || lower.contains("keynote")
    {
        KIND_APPLESCRIPT
    } else {
        KIND_TEXT_SCRIPT
    }
}

// deterministic code generation per kind. safe by construction.
pub fn generate_tool_code(spec: &SelfExtendToolSpec) -> String {
    match spec.kind.as_str() {
        KIND_TEXT_SCRIPT => "tr '[:lower:]' '[:upper:]'".to_string(),
        KIND_APPLESCRIPT => format!(
            "tell application \"{}\"\n  -- generated placeholder automation\nend tell",
            spec.target_allowlist.first().cloned().unwrap_or_default()
        ),
        _ => "// site adapter placeholder: operates on the allowlisted origin DOM".to_string(),
    }
}

fn stage_tool_files(store: &TaskStore, spec: &SelfExtendToolSpec, code: &str) -> Result<()> {
    let dir = store.custom_tools_root().join("staging").join(&spec.name);
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create staging dir {}", dir.display()))?;
    fs::write(dir.join("spec.json"), serde_json::to_vec_pretty(spec)?)
        .context("failed to write tool spec")?;
    fs::write(dir.join("tool.code"), code).context("failed to write tool code")?;
    Ok(())
}

// dry-run the generated tool against its test plan and capture a transcript.
fn dry_run_test(store: &TaskStore, spec: &SelfExtendToolSpec, code: &str) -> Result<String> {
    match spec.kind.as_str() {
        KIND_TEXT_SCRIPT => match run_text_script_code(store, code, "hello world") {
            Ok(output) => Ok(format!("dry-run text_script: input='hello world' output='{output}'")),
            Err(err) => Ok(format!("dry-run text_script failed: {err}")),
        },
        KIND_APPLESCRIPT => Ok(format!(
            "dry-run applescript: allowlist={:?}; script staged (live execution is automation-permission gated)",
            spec.target_allowlist
        )),
        _ => Ok("dry-run site_adapter: staged against saved DOM snapshot (not executed here)".to_string()),
    }
}

pub fn approve_custom_tool(store: &TaskStore, tool_id: i64) -> Result<CustomToolDto> {
    let tool = get_custom_tool(store, tool_id)?
        .ok_or_else(|| anyhow!("custom tool id={tool_id} not found"))?;
    if tool.status == STATUS_KILLED {
        return Err(anyhow!("cannot approve a killed tool"));
    }
    // move staging -> installed.
    let staging = store.custom_tools_root().join("staging").join(&tool.name);
    let installed = store.custom_tools_root().join("installed").join(&tool.name);
    if staging.exists() {
        fs::create_dir_all(installed.parent().unwrap_or(&installed))
            .context("failed to create installed root")?;
        let _ = fs::remove_dir_all(&installed);
        fs::rename(&staging, &installed)
            .or_else(|_| copy_dir(&staging, &installed))
            .context("failed to install tool files")?;
    }
    set_tool_status(store, tool_id, STATUS_INSTALLED)?;
    get_custom_tool(store, tool_id)?.ok_or_else(|| anyhow!("tool missing after install"))
}

pub fn kill_custom_tool(store: &TaskStore, tool_id: i64) -> Result<CustomToolDto> {
    let tool = get_custom_tool(store, tool_id)?
        .ok_or_else(|| anyhow!("custom tool id={tool_id} not found"))?;
    set_tool_status(store, tool_id, STATUS_KILLED)?;
    let installed = store.custom_tools_root().join("installed").join(&tool.name);
    let _ = fs::remove_dir_all(&installed);
    get_custom_tool(store, tool_id)?.ok_or_else(|| anyhow!("tool missing after kill"))
}

#[allow(dead_code)]
pub fn is_tool_active(store: &TaskStore, name: &str) -> bool {
    get_custom_tool_by_name(store, name)
        .ok()
        .flatten()
        .map(|tool| tool.status == STATUS_INSTALLED)
        .unwrap_or(false)
}

// ---- running -------------------------------------------------------------

// Propose an installed custom-tool run. Killed/absent tools degrade to guided
// fallback and re-record the gap. Installed tools never execute here: every
// exact input requires a separate approval command and receipt transition.
pub fn run_custom_tool(
    store: &TaskStore,
    task_id: i64,
    name: &str,
    input: &str,
) -> Result<CustomToolRunResultDto> {
    if input.len() > MAX_TOOL_INPUT_BYTES {
        return Err(anyhow!(
            "custom tool input exceeds the {} byte limit",
            MAX_TOOL_INPUT_BYTES
        ));
    }
    let class = ActionClass::ToolCustom(name.to_string());
    // hard cap: tool.custom.* can never run above L1.
    let level = crate::trust::level_for_action(store, &class.as_str())?;
    crate::trust::assert_runtime_level_allowed(&class.as_str(), &level)?;

    let Some(tool) = get_custom_tool_by_name(store, name)?.filter(|t| t.status == STATUS_INSTALLED)
    else {
        // no active adapter -> guided fallback + gap.
        let _ = record_capability_gap(store, name, GAP_REASON_NO_ADAPTER);
        let receipt = store.create_action_receipt(
            task_id,
            &class.as_str(),
            "self_extend",
            &level,
            &format!("custom tool '{name}' is not installed"),
            input.chars().take(200).collect::<String>().as_str(),
            RUN_STATUS_GUIDED,
            Some(GAP_REASON_NO_ADAPTER),
            None,
        )?;
        return Ok(CustomToolRunResultDto {
            status: RUN_STATUS_GUIDED.to_string(),
            output: None,
            message: format!("'{name}' is not available; guided fallback"),
            receipt_id: Some(receipt.id),
        });
    };

    match tool.kind.as_str() {
        KIND_TEXT_SCRIPT => {
            let receipt = crate::action_bus::ActionBus::dispatch_proposal(
                store,
                &crate::action_bus::ActionRequest {
                    task_id,
                    class,
                    surface: "self_extend".to_string(),
                    description: format!("run custom text tool '{name}'"),
                    payload: serde_json::json!({
                        "tool_id": tool.id,
                        "input_excerpt": input.chars().take(200).collect::<String>(),
                    }),
                    reversibility: crate::action_bus::Reversibility::Irreversible,
                },
            )?;
            let conn = store.connect()?;
            if let Err(err) = conn.execute(
                "INSERT INTO custom_tool_runs (receipt_id, task_id, tool_id, input)
                 VALUES (?1, ?2, ?3, ?4)",
                params![receipt.id, task_id, tool.id, input],
            ) {
                let _ = store.update_action_receipt_status(
                    receipt.id,
                    RUN_STATUS_FAILED,
                    Some("failed to persist exact custom-tool run"),
                    None,
                );
                return Err(err).context("failed to persist custom-tool run");
            }
            Ok(CustomToolRunResultDto {
                status: RUN_STATUS_PENDING_APPROVAL.to_string(),
                output: None,
                message: format!("'{name}' requires approval for this exact run"),
                receipt_id: Some(receipt.id),
            })
        }
        // applescript / site_adapter live execution is automation/extension
        // gated; produce a guided receipt.
        _ => {
            let receipt = store.create_action_receipt(
                task_id,
                &class.as_str(),
                "self_extend",
                &level,
                &format!(
                    "custom tool '{name}' ({}) staged; live execution is permission gated",
                    tool.kind
                ),
                input.chars().take(200).collect::<String>().as_str(),
                RUN_STATUS_GUIDED,
                None,
                None,
            )?;
            Ok(CustomToolRunResultDto {
                status: RUN_STATUS_GUIDED.to_string(),
                output: None,
                message: format!("'{name}' guided (permission-gated kind {})", tool.kind),
                receipt_id: Some(receipt.id),
            })
        }
    }
}

#[derive(Debug)]
struct PendingCustomToolRun {
    receipt_id: i64,
    name: String,
    kind: String,
    code: String,
    input: String,
}

pub fn approve_custom_tool_run(
    store: &TaskStore,
    receipt_id: i64,
) -> Result<CustomToolRunResultDto> {
    let pending = claim_custom_tool_run(store, receipt_id)?;
    if pending.kind != KIND_TEXT_SCRIPT {
        return Err(anyhow!(
            "custom tool kind '{}' is not executable",
            pending.kind
        ));
    }

    match run_text_script_code(store, &pending.code, &pending.input) {
        Ok(output) => {
            finish_custom_tool_run(store, receipt_id, RUN_STATUS_APPLIED, Some(&output), None)?;
            Ok(CustomToolRunResultDto {
                status: RUN_STATUS_APPLIED.to_string(),
                output: Some(output),
                message: format!("ran '{}' after explicit approval", pending.name),
                receipt_id: Some(pending.receipt_id),
            })
        }
        Err(err) => {
            let message = err.to_string();
            finish_custom_tool_run(store, receipt_id, RUN_STATUS_FAILED, None, Some(&message))?;
            Ok(CustomToolRunResultDto {
                status: RUN_STATUS_FAILED.to_string(),
                output: None,
                message,
                receipt_id: Some(pending.receipt_id),
            })
        }
    }
}

pub fn reject_custom_tool_run(
    store: &TaskStore,
    receipt_id: i64,
) -> Result<CustomToolRunResultDto> {
    let mut conn = store.connect()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let changed = tx.execute(
        "UPDATE custom_tool_runs
         SET status = 'rejected', resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE receipt_id = ?1 AND status = 'pending_approval'",
        params![receipt_id],
    )?;
    if changed != 1 {
        return Err(anyhow!("custom tool run is not pending approval"));
    }
    let receipt_changed = tx.execute(
        "UPDATE action_receipts
         SET status = 'rejected', resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1 AND status = 'pending_approval' AND class LIKE 'tool.custom.%'",
        params![receipt_id],
    )?;
    if receipt_changed != 1 {
        return Err(anyhow!("custom tool receipt is not pending approval"));
    }
    tx.commit()?;
    Ok(CustomToolRunResultDto {
        status: RUN_STATUS_REJECTED.to_string(),
        output: None,
        message: "custom tool run rejected".to_string(),
        receipt_id: Some(receipt_id),
    })
}

fn claim_custom_tool_run(store: &TaskStore, receipt_id: i64) -> Result<PendingCustomToolRun> {
    let mut conn = store.connect()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let pending = tx
        .query_row(
            "SELECT r.receipt_id, t.name, t.kind, t.code, r.input
             FROM custom_tool_runs r
             JOIN custom_tools t ON t.id = r.tool_id
             JOIN action_receipts a ON a.id = r.receipt_id
             WHERE r.receipt_id = ?1
               AND r.status = 'pending_approval'
               AND a.status = 'pending_approval'
               AND a.class LIKE 'tool.custom.%'
               AND t.status = 'installed'",
            params![receipt_id],
            |row| {
                Ok(PendingCustomToolRun {
                    receipt_id: row.get(0)?,
                    name: row.get(1)?,
                    kind: row.get(2)?,
                    code: row.get(3)?,
                    input: row.get(4)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| {
            anyhow!("custom tool run is not pending approval or its tool is disabled")
        })?;
    let run_changed = tx.execute(
        "UPDATE custom_tool_runs SET status = 'running'
         WHERE receipt_id = ?1 AND status = 'pending_approval'",
        params![receipt_id],
    )?;
    let receipt_changed = tx.execute(
        "UPDATE action_receipts SET status = 'approved'
         WHERE id = ?1 AND status = 'pending_approval'",
        params![receipt_id],
    )?;
    if run_changed != 1 || receipt_changed != 1 {
        return Err(anyhow!("custom tool run approval lost a concurrent race"));
    }
    tx.commit()?;
    Ok(pending)
}

fn finish_custom_tool_run(
    store: &TaskStore,
    receipt_id: i64,
    status: &str,
    output: Option<&str>,
    error_message: Option<&str>,
) -> Result<()> {
    if !matches!(status, RUN_STATUS_APPLIED | RUN_STATUS_FAILED) {
        return Err(anyhow!("invalid terminal custom tool run status"));
    }
    let mut conn = store.connect()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let run_changed = tx.execute(
        "UPDATE custom_tool_runs
         SET status = ?1, output = ?2, error_message = ?3,
             resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE receipt_id = ?4 AND status = 'running'",
        params![status, output, error_message, receipt_id],
    )?;
    let receipt_changed = tx.execute(
        "UPDATE action_receipts
         SET status = ?1, failure_reason = ?2,
             resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?3 AND status = 'approved'",
        params![status, error_message, receipt_id],
    )?;
    if run_changed != 1 || receipt_changed != 1 {
        return Err(anyhow!(
            "custom tool run could not be finalized consistently"
        ));
    }
    tx.commit()?;
    Ok(())
}

// applescript allowlist rail: a tool may only target declared apps.
#[allow(dead_code)]
pub fn tool_may_target(tool: &CustomToolDto, app: &str) -> bool {
    tool.target_allowlist
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(app.trim()))
}

// static pre-execution guard for text_script sandboxing.
pub fn script_is_sandbox_safe(code: &str) -> Result<()> {
    let lower = code.to_ascii_lowercase();
    for token in BLOCKED_SCRIPT_TOKENS {
        if lower.contains(token) {
            return Err(anyhow!(
                "text_script blocked by sandbox guard: contains '{token}'"
            ));
        }
    }
    Ok(())
}

// execute a text_script in a deny-by-default operating-system sandbox. macOS
// is the supported desktop runtime; other platforms fail closed.
pub fn run_text_script_code(store: &TaskStore, code: &str, input: &str) -> Result<String> {
    script_is_sandbox_safe(code)?;
    let workspace = store.workspace_root_path();
    let _ = fs::create_dir_all(&workspace);
    let cwd: PathBuf = if workspace.exists() {
        workspace
    } else {
        std::env::temp_dir()
    };

    #[cfg(not(target_os = "macos"))]
    return Err(anyhow!(
        "text_script execution is disabled: no supported operating-system sandbox"
    ));

    #[cfg(target_os = "macos")]
    let sandbox_profile = macos_text_script_sandbox_profile(&cwd)?;

    #[cfg(target_os = "macos")]
    let mut child = {
        let mut command = Command::new("/usr/bin/sandbox-exec");
        command
            .arg("-p")
            .arg(sandbox_profile)
            .arg("/bin/sh")
            .arg("-c")
            .arg(code)
            .current_dir(&cwd)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        command
            .spawn()
            .context("failed to spawn text_script subprocess")?
    };

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("text_script stdout pipe missing"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("text_script stderr pipe missing"))?;
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(MAX_TOOL_OUTPUT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr
            .take(MAX_TOOL_OUTPUT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
    }

    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(TEXT_SCRIPT_TIMEOUT_SECS);
    loop {
        match child.try_wait().context("failed to poll text_script")? {
            Some(_) => break,
            None => {
                if std::time::Instant::now() > deadline {
                    #[cfg(target_os = "macos")]
                    unsafe {
                        libc::kill(-(child.id() as i32), libc::SIGKILL);
                    }
                    let _ = child.wait();
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Err(anyhow!("text_script timed out"));
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }

    let status = child
        .wait()
        .context("failed to collect text_script status")?;
    let stdout = stdout_reader
        .join()
        .map_err(|_| anyhow!("text_script stdout reader panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow!("text_script stderr reader panicked"))??;
    if stdout.len() as u64 > MAX_TOOL_OUTPUT_BYTES || stderr.len() as u64 > MAX_TOOL_OUTPUT_BYTES {
        return Err(anyhow!("text_script output exceeded the 1 MiB limit"));
    }
    if !status.success() {
        return Err(anyhow!(
            "text_script exited with status {:?}: {}",
            status.code(),
            String::from_utf8_lossy(&stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&stdout).trim_end().to_string())
}

#[cfg(target_os = "macos")]
fn macos_text_script_sandbox_profile(workspace: &std::path::Path) -> Result<String> {
    let workspace = workspace
        .canonicalize()
        .context("failed to canonicalize custom-tool workspace")?;
    let escaped = workspace
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    Ok(format!(
        "(version 1)\n\
         (deny default)\n\
         (import \"system.sb\")\n\
         (allow process-exec process-fork)\n\
         (allow signal (target self))\n\
         (allow file-read* (subpath \"/usr/bin\") (subpath \"/bin\") \
          (literal \"/private/var/select/sh\") \
          (subpath \"{escaped}\"))\n\
         (allow file-write* (subpath \"{escaped}\"))\n\
         (deny network*)\n\
         (deny file-read* (literal \"/private/etc/passwd\") \
          (literal \"/private/etc/master.passwd\"))"
    ))
}

// ---- store helpers -------------------------------------------------------

fn insert_custom_tool(
    store: &TaskStore,
    spec: &SelfExtendToolSpec,
    code: &str,
    transcript: &str,
    status: &str,
    gap_id: Option<i64>,
) -> Result<CustomToolDto> {
    let conn = store.connect()?;
    conn.execute(
        "INSERT INTO custom_tools
         (name, kind, purpose, target_allowlist_json, spec_json, code, test_transcript, status, gap_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            spec.name,
            spec.kind,
            spec.purpose,
            serde_json::to_string(&spec.target_allowlist)?,
            serde_json::to_string(spec)?,
            code,
            transcript,
            status,
            gap_id,
        ],
    )
    .context("failed to insert custom tool")?;
    let id = conn.last_insert_rowid();
    drop(conn);
    get_custom_tool(store, id)?.ok_or_else(|| anyhow!("custom tool missing after insert"))
}

fn set_tool_status(store: &TaskStore, tool_id: i64, status: &str) -> Result<()> {
    let conn = store.connect()?;
    conn.execute(
        "UPDATE custom_tools SET status = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?2",
        params![status, tool_id],
    )
    .context("failed to update custom tool status")?;
    Ok(())
}

pub fn list_custom_tools(store: &TaskStore) -> Result<Vec<CustomToolDto>> {
    let conn = store.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, name, kind, purpose, target_allowlist_json, code, test_transcript, status, created_at
         FROM custom_tools ORDER BY id DESC",
    )?;
    let rows = stmt
        .query_map([], custom_tool_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn get_custom_tool(store: &TaskStore, tool_id: i64) -> Result<Option<CustomToolDto>> {
    let conn = store.connect()?;
    conn.query_row(
        "SELECT id, name, kind, purpose, target_allowlist_json, code, test_transcript, status, created_at
         FROM custom_tools WHERE id = ?1",
        params![tool_id],
        custom_tool_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn get_custom_tool_by_name(store: &TaskStore, name: &str) -> Result<Option<CustomToolDto>> {
    let conn = store.connect()?;
    conn.query_row(
        "SELECT id, name, kind, purpose, target_allowlist_json, code, test_transcript, status, created_at
         FROM custom_tools WHERE name = ?1",
        params![name],
        custom_tool_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn capability_gap_from_row(row: &Row<'_>) -> rusqlite::Result<CapabilityGapDto> {
    Ok(CapabilityGapDto {
        id: row.get(0)?,
        surface: row.get(1)?,
        description: row.get(2)?,
        occurrence_count: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn custom_tool_from_row(row: &Row<'_>) -> rusqlite::Result<CustomToolDto> {
    let allowlist_json: String = row.get(4)?;
    Ok(CustomToolDto {
        id: row.get(0)?,
        name: row.get(1)?,
        kind: row.get(2)?,
        purpose: row.get(3)?,
        target_allowlist: serde_json::from_str(&allowlist_json).unwrap_or_default(),
        code: row.get(5)?,
        test_transcript: row.get(6)?,
        status: row.get(7)?,
        created_at: row.get(8)?,
    })
}

fn sanitize_name(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_store() -> (TempDir, TaskStore, i64) {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        let task = store.create_task("self extend").unwrap();
        (dir, store, task.id)
    }

    #[test]
    fn d9_recurring_gap_proposes_stages_and_dry_runs() {
        let (_dir, store, _task) = test_store();
        record_capability_gap(&store, "textutil", GAP_REASON_UNSUPPORTED_SURFACE).unwrap();
        let gap_id =
            record_capability_gap(&store, "textutil", GAP_REASON_UNSUPPORTED_SURFACE).unwrap();
        assert_eq!(recurring_gaps(&store).unwrap().len(), 1);

        let tool = propose_tool_for_gap(&store, gap_id).unwrap();
        assert_eq!(tool.kind, KIND_TEXT_SCRIPT);
        assert_eq!(tool.status, STATUS_STAGED);
        let transcript = tool.test_transcript.unwrap();
        assert!(transcript.contains("HELLO WORLD"), "{transcript}");
    }

    #[test]
    fn d9_single_occurrence_gap_cannot_propose() {
        let (_dir, store, _task) = test_store();
        let gap_id = record_capability_gap(&store, "textutil", "unsupported").unwrap();
        assert!(propose_tool_for_gap(&store, gap_id).is_err());
    }

    #[test]
    fn d9_each_run_requires_approval_before_execution() {
        let (_dir, store, task_id) = test_store();
        record_capability_gap(&store, "textutil", "unsupported").unwrap();
        let gap_id = record_capability_gap(&store, "textutil", "unsupported").unwrap();
        let tool = propose_tool_for_gap(&store, gap_id).unwrap();

        let installed = approve_custom_tool(&store, tool.id).unwrap();
        assert_eq!(installed.status, STATUS_INSTALLED);
        assert!(is_tool_active(&store, &installed.name));

        let result = run_custom_tool(&store, task_id, &installed.name, "hello world").unwrap();
        assert_eq!(result.status, RUN_STATUS_PENDING_APPROVAL);
        assert!(result.output.is_none());
        let receipt_id = result.receipt_id.unwrap();
        let pending = store.get_action_receipt(receipt_id).unwrap().unwrap();
        assert_eq!(pending.status, RUN_STATUS_PENDING_APPROVAL);

        let result = approve_custom_tool_run(&store, receipt_id).unwrap();
        assert_eq!(result.status, RUN_STATUS_APPLIED, "{}", result.message);
        assert_eq!(result.output.as_deref(), Some("HELLO WORLD"));
        // a receipt exists for the run, classed tool.custom.<name>.
        let receipts = store.list_action_receipts(Some(task_id), 10).unwrap();
        assert!(receipts
            .iter()
            .any(|r| r.class.starts_with("tool.custom.") && r.status == RUN_STATUS_APPLIED));
        assert!(approve_custom_tool_run(&store, receipt_id).is_err());
    }

    #[test]
    fn d9_rejected_run_never_executes() {
        let (_dir, store, task_id) = test_store();
        record_capability_gap(&store, "textutil", "unsupported").unwrap();
        let gap_id = record_capability_gap(&store, "textutil", "unsupported").unwrap();
        let tool = propose_tool_for_gap(&store, gap_id).unwrap();
        let installed = approve_custom_tool(&store, tool.id).unwrap();
        let proposed = run_custom_tool(&store, task_id, &installed.name, "secret").unwrap();
        let receipt_id = proposed.receipt_id.unwrap();

        let rejected = reject_custom_tool_run(&store, receipt_id).unwrap();
        assert_eq!(rejected.status, RUN_STATUS_REJECTED);
        assert!(approve_custom_tool_run(&store, receipt_id).is_err());
        assert_eq!(
            store
                .get_action_receipt(receipt_id)
                .unwrap()
                .unwrap()
                .status,
            RUN_STATUS_REJECTED
        );
    }

    #[test]
    fn d9_kill_switch_degrades_to_guided_fallback() {
        let (_dir, store, task_id) = test_store();
        record_capability_gap(&store, "textutil", "unsupported").unwrap();
        let gap_id = record_capability_gap(&store, "textutil", "unsupported").unwrap();
        let tool = propose_tool_for_gap(&store, gap_id).unwrap();
        let installed = approve_custom_tool(&store, tool.id).unwrap();

        kill_custom_tool(&store, installed.id).unwrap();
        assert!(!is_tool_active(&store, &installed.name));
        let result = run_custom_tool(&store, task_id, &installed.name, "hello world").unwrap();
        assert_eq!(result.status, RUN_STATUS_GUIDED);
        assert!(result.output.is_none());
    }

    #[test]
    fn d9_tool_custom_hard_capped_at_l1() {
        let (_dir, store, _task) = test_store();
        // trust hard cap (D4): tool.custom.* cannot be raised, even by tamper.
        assert!(crate::trust::set_trust_level(&store, "tool.custom.textutil", "L2", true).is_err());
        let conn = store.connect().unwrap();
        conn.execute(
            "INSERT INTO trust_levels (class, level, approval_streak, sticky_l1) VALUES ('tool.custom.textutil','L3',99,0)",
            [],
        )
        .unwrap();
        drop(conn);
        assert_eq!(
            crate::trust::effective_level_for_action(&store, "tool.custom.textutil").unwrap(),
            crate::trust::TrustLevel::L1
        );
    }

    #[test]
    fn d9_sandbox_guard_refuses_network_and_escape() {
        let (_dir, store, _task) = test_store();
        assert!(script_is_sandbox_safe("tr '[:lower:]' '[:upper:]'").is_ok());
        assert!(run_text_script_code(&store, "curl https://example.com", "x").is_err());
        assert!(script_is_sandbox_safe("cat ../../etc/passwd").is_err());
        assert!(script_is_sandbox_safe("python -c 'import socket'").is_err());
        assert!(run_text_script_code(&store, "p=/e${x}tc/passwd; cat \"$p\"", "").is_err());
        assert!(run_text_script_code(
            &store,
            "u=u; /usr/bin/c${u}rl -s http${x}://127.0.0.1:9",
            ""
        )
        .is_err());
    }

    #[test]
    fn d9_applescript_allowlist_is_enforced() {
        let tool = CustomToolDto {
            id: 1,
            name: "pages_helper".to_string(),
            kind: KIND_APPLESCRIPT.to_string(),
            purpose: "x".to_string(),
            target_allowlist: vec!["Pages".to_string()],
            code: String::new(),
            test_transcript: None,
            status: STATUS_INSTALLED.to_string(),
            created_at: String::new(),
        };
        assert!(tool_may_target(&tool, "Pages"));
        assert!(!tool_may_target(&tool, "Terminal"));
    }
}
