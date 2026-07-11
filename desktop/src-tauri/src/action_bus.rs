// apex d1: action bus. every mutation gets a unified receipt and, when
// possible, a local undo snapshot before touching the outside world.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::{models::ActionReceiptDto, store::TaskStore};

pub const ACTION_STATUS_APPLIED: &str = "applied";
pub const ACTION_STATUS_REJECTED: &str = "rejected";
pub const ACTION_STATUS_REVERTED: &str = "reverted";
pub const ACTION_STATUS_FAILED: &str = "failed";
pub const UNDO_RETENTION_DAYS: u64 = 30;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionClass {
    DocInsert,
    DocReplace,
    DocSuggest,
    FileWrite,
    FileDelete,
    EmailDraft,
    EmailSend,
    CalendarPropose,
    SystemOpen,
    ToolCustom(String),
}

impl ActionClass {
    pub fn as_str(&self) -> String {
        match self {
            Self::DocInsert => "doc.insert".to_string(),
            Self::DocReplace => "doc.replace".to_string(),
            Self::DocSuggest => "doc.suggest".to_string(),
            Self::FileWrite => "file.write".to_string(),
            Self::FileDelete => "file.delete".to_string(),
            Self::EmailDraft => "email.draft".to_string(),
            Self::EmailSend => "email.send".to_string(),
            Self::CalendarPropose => "calendar.propose".to_string(),
            Self::SystemOpen => "system.open".to_string(),
            Self::ToolCustom(name) => format!("tool.custom.{}", sanitize_tool_name(name)),
        }
    }

    #[allow(dead_code)]
    pub fn parse(raw: &str) -> Option<Self> {
        let clean = raw.trim();
        match clean {
            "doc.insert" => Some(Self::DocInsert),
            "doc.replace" => Some(Self::DocReplace),
            "doc.suggest" => Some(Self::DocSuggest),
            "file.write" => Some(Self::FileWrite),
            "file.delete" => Some(Self::FileDelete),
            "email.draft" => Some(Self::EmailDraft),
            "email.send" => Some(Self::EmailSend),
            "calendar.propose" => Some(Self::CalendarPropose),
            "system.open" => Some(Self::SystemOpen),
            _ => clean
                .strip_prefix("tool.custom.")
                .filter(|name| !name.trim().is_empty())
                .map(|name| Self::ToolCustom(name.to_string())),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reversibility {
    Reversible,
    Guided,
    Irreversible,
}

impl Reversibility {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reversible => "reversible",
            Self::Guided => "guided",
            Self::Irreversible => "irreversible",
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRequest {
    pub task_id: i64,
    pub class: ActionClass,
    pub surface: String,
    pub description: String,
    pub payload: serde_json::Value,
    pub reversibility: Reversibility,
}

#[allow(dead_code)]
pub trait ActionAdapter {
    fn supports(&self, request: &ActionRequest) -> bool;
    fn execute(&self, store: &TaskStore, request: &ActionRequest) -> Result<ActionReceiptDto>;
    fn revert(&self, store: &TaskStore, receipt_id: i64) -> Result<ActionReceiptDto>;
}

#[derive(Debug, Clone)]
pub struct FileWritePayload {
    pub destination_path: PathBuf,
    pub content: String,
    pub payload_excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoogleDocsWritePayload {
    pub document_title: String,
    pub before_text: String,
    pub after_text: String,
    pub anchor_before: String,
    pub anchor_after: String,
    pub prefer_suggesting: bool,
}

pub struct FileWriteAdapter;
pub struct GoogleDocsAdapter;

impl FileWriteAdapter {
    #[allow(dead_code)]
    pub fn execute_file_write_trusted(
        store: &TaskStore,
        task_id: i64,
        surface: &str,
        description: &str,
        payload: FileWritePayload,
    ) -> Result<ActionReceiptDto> {
        if !crate::trust::execute_allowed_without_approval(store, &ActionClass::FileWrite.as_str())?
        {
            return Err(anyhow!("file.write requires approval at L1"));
        }
        let level = crate::trust::level_for_action(store, &ActionClass::FileWrite.as_str())?;
        Self::execute_file_write(store, task_id, surface, description, &level, payload)
    }

    pub fn execute_file_write(
        store: &TaskStore,
        task_id: i64,
        surface: &str,
        description: &str,
        level: &str,
        payload: FileWritePayload,
    ) -> Result<ActionReceiptDto> {
        crate::trust::assert_runtime_level_allowed(&ActionClass::FileWrite.as_str(), level)?;
        let receipt = store.create_action_receipt(
            task_id,
            &ActionClass::FileWrite.as_str(),
            surface,
            level,
            description,
            &payload.payload_excerpt,
            "running",
            None,
            None,
        )?;

        let undo_ref = match snapshot_for_file_write(store, receipt.id, &payload.destination_path) {
            Ok(path) => path,
            Err(err) => {
                let receipt = store.update_action_receipt_status(
                    receipt.id,
                    ACTION_STATUS_FAILED,
                    Some(&err.to_string()),
                    None,
                )?;
                return Err(anyhow!(
                    "failed to snapshot file before write (receipt id={}): {}",
                    receipt.id,
                    err
                ));
            }
        };

        if let Err(err) = fs::write(&payload.destination_path, &payload.content) {
            let receipt = store.update_action_receipt_status(
                receipt.id,
                ACTION_STATUS_FAILED,
                Some(&err.to_string()),
                Some(&undo_ref),
            )?;
            return Err(anyhow!(
                "failed to write file for action receipt id={}: {}",
                receipt.id,
                err
            ));
        }

        let receipt = store.update_action_receipt_status(
            receipt.id,
            ACTION_STATUS_APPLIED,
            None,
            Some(&undo_ref),
        )?;
        crate::trust::record_receipt_outcome(store, &receipt)?;
        Ok(receipt)
    }
}

impl ActionAdapter for FileWriteAdapter {
    fn supports(&self, request: &ActionRequest) -> bool {
        request.class == ActionClass::FileWrite
    }

    fn execute(&self, store: &TaskStore, request: &ActionRequest) -> Result<ActionReceiptDto> {
        if !self.supports(request) {
            return Err(anyhow!("file write adapter does not support this action"));
        }
        let destination_path = request
            .payload
            .get("destination_path")
            .and_then(|value| value.as_str())
            .ok_or_else(|| anyhow!("file.write payload missing destination_path"))?;
        let content = request
            .payload
            .get("content")
            .and_then(|value| value.as_str())
            .ok_or_else(|| anyhow!("file.write payload missing content"))?;
        Self::execute_file_write(
            store,
            request.task_id,
            &request.surface,
            &request.description,
            "L1",
            FileWritePayload {
                destination_path: PathBuf::from(destination_path),
                content: content.to_string(),
                payload_excerpt: excerpt_payload(&request.payload),
            },
        )
    }

    fn revert(&self, store: &TaskStore, receipt_id: i64) -> Result<ActionReceiptDto> {
        revert_action_receipt(store, receipt_id)
    }
}

impl GoogleDocsAdapter {
    pub fn request_tracked_change(
        store: &TaskStore,
        task_id: i64,
        description: &str,
        payload: GoogleDocsWritePayload,
    ) -> Result<ActionReceiptDto> {
        let class = if payload.prefer_suggesting {
            ActionClass::DocSuggest
        } else {
            ActionClass::DocReplace
        };
        let payload_excerpt = serde_json::json!({
            "document_title": payload.document_title,
            "before_text": excerpt_chars(&payload.before_text, 120),
            "after_text": excerpt_chars(&payload.after_text, 120),
            "anchor_before": payload.anchor_before,
            "anchor_after": payload.anchor_after,
            "prefer_suggesting": payload.prefer_suggesting
        })
        .to_string();
        store.create_action_receipt(
            task_id,
            &class.as_str(),
            "google_docs",
            "L1",
            description,
            &payload_excerpt,
            "pending_approval",
            None,
            None,
        )
    }
}

impl ActionAdapter for GoogleDocsAdapter {
    fn supports(&self, request: &ActionRequest) -> bool {
        matches!(
            request.class,
            ActionClass::DocInsert | ActionClass::DocReplace | ActionClass::DocSuggest
        ) && request.surface == "google_docs"
    }

    fn execute(&self, store: &TaskStore, request: &ActionRequest) -> Result<ActionReceiptDto> {
        if !self.supports(request) {
            return Err(anyhow!("google docs adapter does not support this action"));
        }
        store.create_action_receipt(
            request.task_id,
            &request.class.as_str(),
            &request.surface,
            "L1",
            &request.description,
            &excerpt_payload(&request.payload),
            "pending_approval",
            None,
            None,
        )
    }

    fn revert(&self, _store: &TaskStore, receipt_id: i64) -> Result<ActionReceiptDto> {
        Err(anyhow!(
            "google docs receipt id={} reverts through native tracked-change discard or guided fallback",
            receipt_id
        ))
    }
}

pub fn record_rejected_action(
    store: &TaskStore,
    task_id: i64,
    class: ActionClass,
    surface: &str,
    level: &str,
    description: &str,
    payload_excerpt: &str,
) -> Result<ActionReceiptDto> {
    let receipt = store.create_action_receipt(
        task_id,
        &class.as_str(),
        surface,
        level,
        description,
        payload_excerpt,
        ACTION_STATUS_REJECTED,
        None,
        None,
    )?;
    crate::trust::record_receipt_outcome(store, &receipt)?;
    Ok(receipt)
}

pub fn revert_action_receipt(store: &TaskStore, receipt_id: i64) -> Result<ActionReceiptDto> {
    let receipt = store
        .get_action_receipt(receipt_id)?
        .ok_or_else(|| anyhow!("action receipt id={} not found", receipt_id))?;
    if receipt.class != ActionClass::FileWrite.as_str() {
        return Err(anyhow!(
            "receipt id={} is class {}; only file.write can be automatically reverted",
            receipt_id,
            receipt.class
        ));
    }
    if receipt.status == ACTION_STATUS_REVERTED {
        return Ok(receipt);
    }
    let undo_ref = receipt
        .undo_ref
        .as_deref()
        .ok_or_else(|| anyhow!("receipt id={} has no undo snapshot", receipt_id))?;
    restore_file_write_snapshot(Path::new(undo_ref))?;
    let receipt = store.update_action_receipt_status(
        receipt_id,
        ACTION_STATUS_REVERTED,
        None,
        Some(undo_ref),
    )?;
    crate::trust::record_receipt_outcome(store, &receipt)?;
    Ok(receipt)
}

fn snapshot_for_file_write(
    store: &TaskStore,
    receipt_id: i64,
    destination: &Path,
) -> Result<String> {
    let undo_dir = store.action_undo_root().join(receipt_id.to_string());
    fs::create_dir_all(&undo_dir)
        .with_context(|| format!("failed to create undo dir {}", undo_dir.display()))?;
    let before_path = undo_dir.join("before.bin");
    let existed = destination.exists();
    if existed {
        fs::copy(destination, &before_path).with_context(|| {
            format!(
                "failed to snapshot '{}' to '{}'",
                destination.display(),
                before_path.display()
            )
        })?;
    }
    let metadata = serde_json::json!({
        "destination_path": destination,
        "existed": existed,
        "retention_days": UNDO_RETENTION_DAYS
    });
    fs::write(
        undo_dir.join("metadata.json"),
        serde_json::to_vec_pretty(&metadata)?,
    )
    .with_context(|| format!("failed to write undo metadata in {}", undo_dir.display()))?;
    Ok(undo_dir.display().to_string())
}

fn restore_file_write_snapshot(undo_dir: &Path) -> Result<()> {
    let metadata_path = undo_dir.join("metadata.json");
    let raw = fs::read(&metadata_path)
        .with_context(|| format!("failed to read undo metadata {}", metadata_path.display()))?;
    let metadata: serde_json::Value =
        serde_json::from_slice(&raw).context("failed to parse undo metadata")?;
    let destination = metadata
        .get("destination_path")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow!("undo metadata missing destination_path"))?;
    let existed = metadata
        .get("existed")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let destination = PathBuf::from(destination);
    if existed {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to recreate {}", parent.display()))?;
        }
        fs::copy(undo_dir.join("before.bin"), &destination).with_context(|| {
            format!(
                "failed to restore file snapshot to {}",
                destination.display()
            )
        })?;
    } else if destination.exists() {
        fs::remove_file(&destination)
            .with_context(|| format!("failed to remove newly created {}", destination.display()))?;
    }
    Ok(())
}

#[allow(dead_code)]
pub fn excerpt_payload(payload: &serde_json::Value) -> String {
    let raw = payload.to_string();
    raw.chars().take(500).collect()
}

#[allow(dead_code)]
pub fn anchor_context_50(text: &str, start: usize, end: usize) -> (String, String) {
    let before = text[..start.min(text.len())]
        .chars()
        .rev()
        .take(50)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    let after = text[end.min(text.len())..]
        .chars()
        .take(50)
        .collect::<String>();
    (before, after)
}

fn excerpt_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn sanitize_tool_name(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, TaskStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn d1_action_class_taxonomy_round_trips() {
        assert_eq!(
            ActionClass::parse("doc.insert"),
            Some(ActionClass::DocInsert)
        );
        assert_eq!(
            ActionClass::parse("tool.custom.pages_helper"),
            Some(ActionClass::ToolCustom("pages_helper".to_string()))
        );
        assert_eq!(
            ActionClass::ToolCustom("bad name!".to_string()).as_str(),
            "tool.custom.badname"
        );
    }

    #[test]
    fn d1_file_write_receipt_reverts_byte_identical() {
        let (dir, store) = store();
        let task = store.create_task("d1").unwrap();
        let dest = dir.path().join("target.txt");
        fs::write(&dest, b"before bytes").unwrap();
        let receipt = FileWriteAdapter::execute_file_write(
            &store,
            task.id,
            "file",
            "test file write",
            "L1",
            FileWritePayload {
                destination_path: dest.clone(),
                content: "after bytes".to_string(),
                payload_excerpt: "target.txt".to_string(),
            },
        )
        .unwrap();
        assert_eq!(receipt.status, ACTION_STATUS_APPLIED);
        assert_eq!(fs::read(&dest).unwrap(), b"after bytes");

        let reverted = revert_action_receipt(&store, receipt.id).unwrap();
        assert_eq!(reverted.status, ACTION_STATUS_REVERTED);
        assert_eq!(fs::read(&dest).unwrap(), b"before bytes");
    }

    #[test]
    fn d1_revert_removes_file_that_did_not_exist_before() {
        let (dir, store) = store();
        let task = store.create_task("d1 create").unwrap();
        let dest = dir.path().join("created.txt");
        let receipt = FileWriteAdapter::execute_file_write(
            &store,
            task.id,
            "file",
            "create file",
            "L1",
            FileWritePayload {
                destination_path: dest.clone(),
                content: "new".to_string(),
                payload_excerpt: "created.txt".to_string(),
            },
        )
        .unwrap();
        assert!(dest.exists());
        revert_action_receipt(&store, receipt.id).unwrap();
        assert!(!dest.exists());
    }

    #[test]
    fn d2_anchor_context_is_capped_to_fifty_chars() {
        let text = "a".repeat(80) + "TARGET" + &"b".repeat(80);
        let start = 80;
        let end = 86;
        let (before, after) = anchor_context_50(&text, start, end);
        assert_eq!(before.len(), 50);
        assert_eq!(after.len(), 50);
        assert!(before.chars().all(|ch| ch == 'a'));
        assert!(after.chars().all(|ch| ch == 'b'));
    }
}
