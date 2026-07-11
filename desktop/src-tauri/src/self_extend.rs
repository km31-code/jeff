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
// - text_script tools run in a confined subprocess (workspace-only cwd,
//   stripped env) behind a static pre-execution guard that refuses network and
//   filesystem-escape operations. This is a static + confinement guard, NOT a
//   kernel sandbox; a real OS sandbox is the hardening upgrade.

#![cfg_attr(test, allow(dead_code))]

use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

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

pub const GAP_REASON_NO_ADAPTER: &str = "no_adapter";
pub const GAP_REASON_UNSUPPORTED_SURFACE: &str = "unsupported_surface";

const TEXT_SCRIPT_TIMEOUT_SECS: u64 = 5;

// static pre-execution denylist for text_script sandboxing. imperfect by design
// (see module note) -- the confinement boundary, not a kernel sandbox.
const BLOCKED_SCRIPT_TOKENS: &[&str] = &[
    "http://", "https://", "curl", "wget", "netcat", "nc ", "socket", "ftp",
    "ssh", "scp", "telnet", "/etc/", "/usr/local", "/var/", "~/", ".ssh",
    "subprocess", "os.system", "import socket", "requests.", "urllib", "fetch(",
    "xmlhttprequest", "..", "sudo", "rm -rf", "dd if", "mkfs", ":(){", "> /dev",
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
    insert_custom_tool(store, &spec, &code, &transcript, STATUS_STAGED, Some(gap_id))
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
    if lower.contains("http") || lower.contains(".com") || lower.contains("site") || lower.contains("docs.google")
    {
        KIND_SITE_ADAPTER
    } else if lower.contains("pages") || lower.contains("word") || lower.contains("app") || lower.contains("keynote")
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

// run an installed custom tool. Killed/absent tools degrade to guided fallback
// and re-record the gap. Every path produces an action receipt (L1).
pub fn run_custom_tool(
    store: &TaskStore,
    task_id: i64,
    name: &str,
    input: &str,
) -> Result<CustomToolRunResultDto> {
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
        KIND_TEXT_SCRIPT => match run_text_script_code(store, &tool.code, input) {
            Ok(output) => {
                let receipt = store.create_action_receipt(
                    task_id,
                    &class.as_str(),
                    "self_extend",
                    &level,
                    &format!("ran custom text tool '{name}'"),
                    output.chars().take(200).collect::<String>().as_str(),
                    RUN_STATUS_APPLIED,
                    None,
                    None,
                )?;
                Ok(CustomToolRunResultDto {
                    status: RUN_STATUS_APPLIED.to_string(),
                    output: Some(output),
                    message: format!("ran '{name}'"),
                    receipt_id: Some(receipt.id),
                })
            }
            Err(err) => {
                let receipt = store.create_action_receipt(
                    task_id,
                    &class.as_str(),
                    "self_extend",
                    &level,
                    &format!("custom text tool '{name}' refused/failed"),
                    err.to_string().chars().take(200).collect::<String>().as_str(),
                    RUN_STATUS_FAILED,
                    Some(&err.to_string()),
                    None,
                )?;
                Ok(CustomToolRunResultDto {
                    status: RUN_STATUS_FAILED.to_string(),
                    output: None,
                    message: err.to_string(),
                    receipt_id: Some(receipt.id),
                })
            }
        },
        // applescript / site_adapter live execution is automation/extension
        // gated; produce a guided receipt.
        _ => {
            let receipt = store.create_action_receipt(
                task_id,
                &class.as_str(),
                "self_extend",
                &level,
                &format!("custom tool '{name}' ({}) staged; live execution is permission gated", tool.kind),
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

// execute a text_script in a confined subprocess: workspace-only cwd, stripped
// env, minimal PATH, behind the static guard. NOT a kernel sandbox.
pub fn run_text_script_code(store: &TaskStore, code: &str, input: &str) -> Result<String> {
    script_is_sandbox_safe(code)?;
    let workspace = store.workspace_root_path();
    let _ = fs::create_dir_all(&workspace);
    let cwd: PathBuf = if workspace.exists() {
        workspace
    } else {
        std::env::temp_dir()
    };

    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(code)
        .current_dir(&cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn text_script subprocess")?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(TEXT_SCRIPT_TIMEOUT_SECS);
    loop {
        match child.try_wait().context("failed to poll text_script")? {
            Some(_) => break,
            None => {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    return Err(anyhow!("text_script timed out"));
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }

    let output = child
        .wait_with_output()
        .context("failed to collect text_script output")?;
    if !output.status.success() {
        return Err(anyhow!(
            "text_script exited with status {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string())
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
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
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
        let gap_id = record_capability_gap(&store, "textutil", GAP_REASON_UNSUPPORTED_SURFACE).unwrap();
        assert_eq!(recurring_gaps(&store).unwrap().len(), 1);

        let tool = propose_tool_for_gap(&store, gap_id).unwrap();
        assert_eq!(tool.kind, KIND_TEXT_SCRIPT);
        assert_eq!(tool.status, STATUS_STAGED);
        assert!(tool.test_transcript.unwrap().contains("HELLO WORLD"));
    }

    #[test]
    fn d9_single_occurrence_gap_cannot_propose() {
        let (_dir, store, _task) = test_store();
        let gap_id = record_capability_gap(&store, "textutil", "unsupported").unwrap();
        assert!(propose_tool_for_gap(&store, gap_id).is_err());
    }

    #[test]
    fn d9_approve_then_run_produces_applied_receipt() {
        let (_dir, store, task_id) = test_store();
        record_capability_gap(&store, "textutil", "unsupported").unwrap();
        let gap_id = record_capability_gap(&store, "textutil", "unsupported").unwrap();
        let tool = propose_tool_for_gap(&store, gap_id).unwrap();

        let installed = approve_custom_tool(&store, tool.id).unwrap();
        assert_eq!(installed.status, STATUS_INSTALLED);
        assert!(is_tool_active(&store, &installed.name));

        let result = run_custom_tool(&store, task_id, &installed.name, "hello world").unwrap();
        assert_eq!(result.status, RUN_STATUS_APPLIED);
        assert_eq!(result.output.as_deref(), Some("HELLO WORLD"));
        // a receipt exists for the run, classed tool.custom.<name>.
        let receipts = store.list_action_receipts(Some(task_id), 10).unwrap();
        assert!(receipts.iter().any(|r| r.class.starts_with("tool.custom.") && r.status == RUN_STATUS_APPLIED));
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
