// apex d1: action bus. every mutation gets a unified receipt and, when
// possible, a local undo snapshot before touching the outside world.

use std::{
    fs,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
    EmailLabel,
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
            Self::EmailLabel => "email.label".to_string(),
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
            "email.label" => Some(Self::EmailLabel),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRequest {
    pub task_id: i64,
    pub class: ActionClass,
    pub surface: String,
    pub description: String,
    pub payload: serde_json::Value,
    pub reversibility: Reversibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionAuthorization {
    Proposal,
    ExplicitApproval,
    EarnedAutonomy,
}

pub trait ActionAdapter: Sync {
    fn supports(&self, request: &ActionRequest) -> bool;
    fn supports_autonomous_execution(&self) -> bool;
    fn execute(
        &self,
        store: &TaskStore,
        request: &ActionRequest,
        authorization: ActionAuthorization,
    ) -> Result<ActionReceiptDto>;
    #[allow(dead_code)]
    fn revert(&self, store: &TaskStore, receipt_id: i64) -> Result<ActionReceiptDto>;
}

#[derive(Debug, Clone)]
pub struct FileWritePayload {
    pub allowed_root: PathBuf,
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
pub struct ProposalAdapter;

static FILE_WRITE_ADAPTER: FileWriteAdapter = FileWriteAdapter;
static GOOGLE_DOCS_ADAPTER: GoogleDocsAdapter = GoogleDocsAdapter;
static PROPOSAL_ADAPTER: ProposalAdapter = ProposalAdapter;
static ACTION_ADAPTERS: [&'static dyn ActionAdapter; 3] =
    [&FILE_WRITE_ADAPTER, &GOOGLE_DOCS_ADAPTER, &PROPOSAL_ADAPTER];

/// The single production dispatch spine for every externally-visible action.
/// Callers must state whether a user explicitly approved this exact run or the
/// action is relying on earned autonomy. Adapters cannot choose their own trust
/// level or bypass the reversibility requirement.
pub struct ActionBus;

impl ActionBus {
    #[allow(dead_code)]
    pub fn registered_adapter_count() -> usize {
        ACTION_ADAPTERS.len()
    }

    pub fn dispatch_proposal(
        store: &TaskStore,
        request: &ActionRequest,
    ) -> Result<ActionReceiptDto> {
        Self::dispatch(store, request, ActionAuthorization::Proposal)
    }

    pub fn dispatch_explicit(
        store: &TaskStore,
        request: &ActionRequest,
    ) -> Result<ActionReceiptDto> {
        Self::dispatch(store, request, ActionAuthorization::ExplicitApproval)
    }

    #[allow(dead_code)]
    pub fn dispatch_trusted(
        store: &TaskStore,
        request: &ActionRequest,
    ) -> Result<ActionReceiptDto> {
        Self::dispatch(store, request, ActionAuthorization::EarnedAutonomy)
    }

    fn dispatch(
        store: &TaskStore,
        request: &ActionRequest,
        authorization: ActionAuthorization,
    ) -> Result<ActionReceiptDto> {
        let adapter = ACTION_ADAPTERS
            .iter()
            .copied()
            .find(|adapter| adapter.supports(request))
            .ok_or_else(|| {
                anyhow!(
                    "no registered action adapter for class={} surface={}",
                    request.class.as_str(),
                    request.surface
                )
            })?;

        if authorization == ActionAuthorization::EarnedAutonomy {
            if request.reversibility != Reversibility::Reversible
                || !adapter.supports_autonomous_execution()
            {
                return Err(anyhow!(
                    "{} cannot run autonomously without a verified automatic revert",
                    request.class.as_str()
                ));
            }
            if !crate::trust::execute_allowed_without_approval(store, &request.class.as_str())? {
                return Err(anyhow!(
                    "{} requires approval at L1",
                    request.class.as_str()
                ));
            }
        }

        adapter.execute(store, request, authorization)
    }
}

impl FileWriteAdapter {
    #[allow(dead_code)]
    pub fn execute_file_write_trusted(
        store: &TaskStore,
        task_id: i64,
        surface: &str,
        description: &str,
        payload: FileWritePayload,
    ) -> Result<ActionReceiptDto> {
        let request = file_write_request(task_id, surface, description, &payload);
        ActionBus::dispatch_trusted(store, &request)
    }

    pub fn execute_file_write_approved(
        store: &TaskStore,
        task_id: i64,
        surface: &str,
        description: &str,
        payload: FileWritePayload,
    ) -> Result<ActionReceiptDto> {
        let request = file_write_request(task_id, surface, description, &payload);
        ActionBus::dispatch_explicit(store, &request)
    }

    fn execute_authorized(
        store: &TaskStore,
        task_id: i64,
        surface: &str,
        description: &str,
        authorization: ActionAuthorization,
        payload: FileWritePayload,
    ) -> Result<ActionReceiptDto> {
        let level = match authorization {
            ActionAuthorization::Proposal => {
                return Err(anyhow!(
                    "file.write proposals use the subtask proposal store"
                ));
            }
            ActionAuthorization::ExplicitApproval => crate::trust::TRUST_LEVEL_L1.to_string(),
            ActionAuthorization::EarnedAutonomy => {
                let level =
                    crate::trust::level_for_action(store, &ActionClass::FileWrite.as_str())?;
                if level == crate::trust::TRUST_LEVEL_L1 {
                    return Err(anyhow!("file.write requires approval at L1"));
                }
                level
            }
        };
        let receipt = store.create_action_receipt(
            task_id,
            &ActionClass::FileWrite.as_str(),
            surface,
            &level,
            description,
            &payload.payload_excerpt,
            "running",
            None,
            None,
        )?;

        let validated =
            match validate_file_write_destination(&payload.allowed_root, &payload.destination_path)
            {
                Ok(paths) => paths,
                Err(err) => {
                    let _ = store.update_action_receipt_status(
                        receipt.id,
                        ACTION_STATUS_FAILED,
                        Some(&err.to_string()),
                        None,
                    );
                    return Err(err);
                }
            };
        let expected_post_hash = sha256_bytes(payload.content.as_bytes());
        let undo_ref = match snapshot_for_file_write(
            store,
            receipt.id,
            &validated.destination,
            &validated.allowed_root,
            &expected_post_hash,
        ) {
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
        store.update_action_receipt_status(receipt.id, "applying", None, Some(&undo_ref))?;

        if let Err(err) = atomic_write_file(&validated.destination, payload.content.as_bytes()) {
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

    fn supports_autonomous_execution(&self) -> bool {
        true
    }

    fn execute(
        &self,
        store: &TaskStore,
        request: &ActionRequest,
        authorization: ActionAuthorization,
    ) -> Result<ActionReceiptDto> {
        if !self.supports(request) {
            return Err(anyhow!("file write adapter does not support this action"));
        }
        let allowed_root = request
            .payload
            .get("allowed_root")
            .and_then(|value| value.as_str())
            .ok_or_else(|| anyhow!("file.write payload missing allowed_root"))?;
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
        let payload_excerpt = request
            .payload
            .get("payload_excerpt")
            .and_then(|value| value.as_str())
            .unwrap_or("file.write");
        Self::execute_authorized(
            store,
            request.task_id,
            &request.surface,
            &request.description,
            authorization,
            FileWritePayload {
                allowed_root: PathBuf::from(allowed_root),
                destination_path: PathBuf::from(destination_path),
                content: content.to_string(),
                payload_excerpt: payload_excerpt.chars().take(500).collect(),
            },
        )
    }

    fn revert(&self, store: &TaskStore, receipt_id: i64) -> Result<ActionReceiptDto> {
        revert_action_receipt(store, receipt_id)
    }
}

fn file_write_request(
    task_id: i64,
    surface: &str,
    description: &str,
    payload: &FileWritePayload,
) -> ActionRequest {
    ActionRequest {
        task_id,
        class: ActionClass::FileWrite,
        surface: surface.to_string(),
        description: description.to_string(),
        payload: serde_json::json!({
            "allowed_root": payload.allowed_root.display().to_string(),
            "destination_path": payload.destination_path.display().to_string(),
            "content": payload.content.clone(),
            "payload_excerpt": payload.payload_excerpt.clone(),
        }),
        reversibility: Reversibility::Reversible,
    }
}

struct ValidatedFileWritePaths {
    allowed_root: PathBuf,
    destination: PathBuf,
}

// resolve symlinks through the destination's nearest existing ancestor so
// containment checks are not defeated by an ancestor symlink (e.g. macOS
// /var -> /private/var). The non-existent tail is kept literal.
fn resolve_existing_ancestor(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(canonical) = fs::canonicalize(&existing) {
            let mut resolved = canonical;
            for part in tail.iter().rev() {
                resolved.push(part);
            }
            return resolved;
        }
        match existing.file_name() {
            Some(name) => tail.push(name.to_os_string()),
            None => return path.to_path_buf(),
        }
        if !existing.pop() {
            return path.to_path_buf();
        }
    }
}

fn validate_file_write_destination(
    allowed_root: &Path,
    destination: &Path,
) -> Result<ValidatedFileWritePaths> {
    let allowed_root = fs::canonicalize(allowed_root).with_context(|| {
        format!(
            "file.write allowed root does not exist: {}",
            allowed_root.display()
        )
    })?;
    if !allowed_root.is_dir() {
        return Err(anyhow!(
            "file.write allowed root is not a directory: {}",
            allowed_root.display()
        ));
    }
    // resolve ancestor symlinks before the containment check so a symlinked
    // temp/workspace root is not a false "escape".
    let canonical_destination = resolve_existing_ancestor(destination);
    if !canonical_destination.is_absolute() || !canonical_destination.starts_with(&allowed_root) {
        return Err(anyhow!(
            "file.write destination {} escapes allowed root {}",
            destination.display(),
            allowed_root.display()
        ));
    }
    let relative = canonical_destination
        .strip_prefix(&allowed_root)
        .context("failed to resolve file.write destination relative to allowed root")?
        .to_path_buf();
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(anyhow!(
            "file.write destination must be a normal relative file path"
        ));
    }

    let mut current = allowed_root.clone();
    let parent_relative = relative
        .parent()
        .ok_or_else(|| anyhow!("file.write destination has no parent"))?;
    for component in parent_relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(anyhow!("file.write parent contains an invalid component"));
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(anyhow!(
                    "file.write refuses symlinked parent {}",
                    current.display()
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(anyhow!(
                    "file.write parent component is not a directory: {}",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).with_context(|| {
                    format!("failed to create file.write parent {}", current.display())
                })?;
            }
            Err(err) => return Err(err.into()),
        }
    }
    let canonical_parent = fs::canonicalize(
        destination
            .parent()
            .ok_or_else(|| anyhow!("file.write destination has no parent"))?,
    )?;
    if !canonical_parent.starts_with(&allowed_root) {
        return Err(anyhow!("file.write canonical parent escapes allowed root"));
    }
    if let Ok(metadata) = fs::symlink_metadata(destination) {
        if metadata.file_type().is_symlink() {
            return Err(anyhow!(
                "file.write refuses final symlink {}",
                destination.display()
            ));
        }
        if !metadata.is_file() {
            return Err(anyhow!(
                "file.write destination is not a regular file: {}",
                destination.display()
            ));
        }
        let canonical_destination = fs::canonicalize(destination)?;
        if !canonical_destination.starts_with(&allowed_root) {
            return Err(anyhow!("file.write destination escapes allowed root"));
        }
    }
    Ok(ValidatedFileWritePaths {
        allowed_root,
        destination: destination.to_path_buf(),
    })
}

fn atomic_write_file(destination: &Path, content: &[u8]) -> Result<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow!("atomic write destination has no parent"))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("jeff-write");
    let temporary = parent.join(format!(
        ".{file_name}.jeff-{}-{nonce}.tmp",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .with_context(|| format!("failed to create atomic temp file {}", temporary.display()))?;
    let write_result = (|| -> Result<()> {
        file.write_all(content)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, destination).with_context(|| {
            format!(
                "failed to atomically replace {} with {}",
                destination.display(),
                temporary.display()
            )
        })?;
        if let Ok(parent_dir) = OpenOptions::new().read(true).open(parent) {
            let _ = parent_dir.sync_all();
        }
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
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
        ActionBus::dispatch_proposal(
            store,
            &ActionRequest {
                task_id,
                class,
                surface: "google_docs".to_string(),
                description: description.to_string(),
                payload: serde_json::from_str(&payload_excerpt)?,
                reversibility: Reversibility::Guided,
            },
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

    fn supports_autonomous_execution(&self) -> bool {
        false
    }

    fn execute(
        &self,
        store: &TaskStore,
        request: &ActionRequest,
        authorization: ActionAuthorization,
    ) -> Result<ActionReceiptDto> {
        if !self.supports(request) {
            return Err(anyhow!("google docs adapter does not support this action"));
        }
        if authorization != ActionAuthorization::Proposal {
            return Err(anyhow!(
                "google docs changes must be proposed before extension approval"
            ));
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

impl ActionAdapter for ProposalAdapter {
    fn supports(&self, request: &ActionRequest) -> bool {
        match &request.class {
            ActionClass::DocInsert | ActionClass::DocReplace | ActionClass::DocSuggest => {
                request.surface.starts_with("native_docs.")
            }
            ActionClass::EmailDraft | ActionClass::EmailLabel => request.surface == "gmail",
            ActionClass::CalendarPropose => request.surface == "calendar",
            ActionClass::ToolCustom(_) => request.surface == "self_extend",
            _ => false,
        }
    }

    fn supports_autonomous_execution(&self) -> bool {
        false
    }

    fn execute(
        &self,
        store: &TaskStore,
        request: &ActionRequest,
        authorization: ActionAuthorization,
    ) -> Result<ActionReceiptDto> {
        if !self.supports(request) {
            return Err(anyhow!("proposal adapter does not support this action"));
        }
        if authorization != ActionAuthorization::Proposal {
            return Err(anyhow!(
                "{} on {} is proposal-only",
                request.class.as_str(),
                request.surface
            ));
        }
        store.create_action_receipt(
            request.task_id,
            &request.class.as_str(),
            &request.surface,
            crate::trust::TRUST_LEVEL_L1,
            &request.description,
            &excerpt_payload(&request.payload),
            "pending_approval",
            None,
            None,
        )
    }

    fn revert(&self, _store: &TaskStore, receipt_id: i64) -> Result<ActionReceiptDto> {
        Err(anyhow!(
            "proposal receipt id={} requires its surface-specific revert adapter",
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
    if receipt.status != ACTION_STATUS_APPLIED {
        return Err(anyhow!(
            "receipt id={} cannot be reverted from status {}",
            receipt_id,
            receipt.status
        ));
    }
    let undo_ref = receipt
        .undo_ref
        .as_deref()
        .ok_or_else(|| anyhow!("receipt id={} has no undo snapshot", receipt_id))?;
    let (undo_path, metadata_hash) = undo_ref.rsplit_once('#').ok_or_else(|| {
        anyhow!(
            "receipt id={} has an unbound legacy undo snapshot; guided revert required",
            receipt_id
        )
    })?;
    let expected_undo_path = store.action_undo_root().join(receipt_id.to_string());
    if Path::new(undo_path) != expected_undo_path {
        return Err(anyhow!(
            "receipt undo path does not match its owned receipt directory"
        ));
    }
    restore_file_write_snapshot(&expected_undo_path, metadata_hash)?;
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
    allowed_root: &Path,
    expected_post_hash: &str,
) -> Result<String> {
    let undo_root = store.action_undo_root();
    fs::create_dir_all(&undo_root)
        .with_context(|| format!("failed to create undo root {}", undo_root.display()))?;
    let undo_dir = undo_root.join(receipt_id.to_string());
    fs::create_dir(&undo_dir)
        .with_context(|| format!("failed to create undo dir {}", undo_dir.display()))?;
    let before_path = undo_dir.join("before.bin");
    let existed = destination.exists();
    let before_bytes = if existed {
        if fs::symlink_metadata(destination)?.file_type().is_symlink() {
            return Err(anyhow!("refusing to snapshot a symlink destination"));
        }
        let bytes = fs::read(destination)
            .with_context(|| format!("failed to read pre-write file {}", destination.display()))?;
        let mut before_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&before_path)?;
        before_file.write_all(&bytes)?;
        before_file.sync_all()?;
        Some(bytes)
    } else {
        None
    };
    let created_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let metadata = serde_json::json!({
        "receipt_id": receipt_id,
        "destination_path": destination.display().to_string(),
        "allowed_root": allowed_root.display().to_string(),
        "existed": existed,
        "before_hash": before_bytes.as_deref().map(sha256_bytes),
        "expected_post_hash": expected_post_hash,
        "created_unix": created_unix,
        "retention_days": UNDO_RETENTION_DAYS,
    });
    let metadata_bytes = serde_json::to_vec_pretty(&metadata)?;
    let metadata_path = undo_dir.join("metadata.json");
    let mut metadata_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&metadata_path)?;
    metadata_file.write_all(&metadata_bytes)?;
    metadata_file.sync_all()?;
    let metadata_hash = sha256_bytes(&metadata_bytes);
    Ok(format!("{}#{metadata_hash}", undo_dir.display()))
}

fn restore_file_write_snapshot(undo_dir: &Path, expected_metadata_hash: &str) -> Result<()> {
    let metadata_path = undo_dir.join("metadata.json");
    let raw = fs::read(&metadata_path)
        .with_context(|| format!("failed to read undo metadata {}", metadata_path.display()))?;
    if sha256_bytes(&raw) != expected_metadata_hash {
        return Err(anyhow!("undo metadata integrity check failed"));
    }
    let metadata: serde_json::Value =
        serde_json::from_slice(&raw).context("failed to parse undo metadata")?;
    let receipt_id = metadata
        .get("receipt_id")
        .and_then(|value| value.as_i64())
        .ok_or_else(|| anyhow!("undo metadata missing receipt_id"))?;
    if undo_dir.file_name().and_then(|name| name.to_str()) != Some(&receipt_id.to_string()) {
        return Err(anyhow!("undo metadata receipt ownership mismatch"));
    }
    let destination = metadata
        .get("destination_path")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow!("undo metadata missing destination_path"))?;
    let allowed_root = metadata
        .get("allowed_root")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow!("undo metadata missing allowed_root"))?;
    let existed = metadata
        .get("existed")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let expected_post_hash = metadata
        .get("expected_post_hash")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow!("undo metadata missing expected_post_hash"))?;
    let created_unix = metadata
        .get("created_unix")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| anyhow!("undo metadata missing created_unix"))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if now.saturating_sub(created_unix) > UNDO_RETENTION_DAYS * 24 * 60 * 60 {
        return Err(anyhow!("undo snapshot has expired"));
    }
    let destination = PathBuf::from(destination);
    let allowed_root = fs::canonicalize(allowed_root)?;
    if !resolve_existing_ancestor(&destination).starts_with(&allowed_root) {
        return Err(anyhow!(
            "undo destination escapes its recorded allowed root"
        ));
    }
    let destination_metadata = fs::symlink_metadata(&destination)
        .context("cannot safely revert because the written destination is missing")?;
    if destination_metadata.file_type().is_symlink() || !destination_metadata.is_file() {
        return Err(anyhow!("cannot safely revert a non-regular destination"));
    }
    let current = fs::read(&destination)?;
    if sha256_bytes(&current) != expected_post_hash {
        return Err(anyhow!(
            "destination changed after Jeff's write; refusing to overwrite later user edits"
        ));
    }
    if existed {
        let before = fs::read(undo_dir.join("before.bin"))?;
        let expected_before_hash = metadata
            .get("before_hash")
            .and_then(|value| value.as_str())
            .ok_or_else(|| anyhow!("undo metadata missing before_hash"))?;
        if sha256_bytes(&before) != expected_before_hash {
            return Err(anyhow!("undo before-image integrity check failed"));
        }
        atomic_write_file(&destination, &before)?;
    } else {
        fs::remove_file(&destination)
            .with_context(|| format!("failed to remove newly created {}", destination.display()))?;
    }
    Ok(())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
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
    fn d1_all_proposal_surfaces_route_through_registered_adapters() {
        let (_dir, store) = store();
        let task = store.create_task("d1 proposals").unwrap();
        let cases = [
            (ActionClass::DocSuggest, "google_docs"),
            (ActionClass::DocReplace, "native_docs.pages"),
            (ActionClass::EmailDraft, "gmail"),
            (ActionClass::EmailLabel, "gmail"),
            (ActionClass::CalendarPropose, "calendar"),
            (ActionClass::ToolCustom("upper".to_string()), "self_extend"),
        ];

        assert_eq!(ActionBus::registered_adapter_count(), 3);
        for (class, surface) in cases {
            let request = ActionRequest {
                task_id: task.id,
                class: class.clone(),
                surface: surface.to_string(),
                description: format!("propose {}", class.as_str()),
                payload: serde_json::json!({"bounded": true}),
                reversibility: Reversibility::Guided,
            };
            let receipt = ActionBus::dispatch_proposal(&store, &request).unwrap();
            assert_eq!(receipt.class, class.as_str());
            assert_eq!(receipt.surface, surface);
            assert_eq!(receipt.status, "pending_approval");
            assert_eq!(receipt.level, crate::trust::TRUST_LEVEL_L1);
        }
    }

    #[test]
    fn d1_proposal_only_surfaces_reject_execution_without_surface_approval() {
        let (_dir, store) = store();
        let task = store.create_task("d1 proposal authorization").unwrap();
        let request = ActionRequest {
            task_id: task.id,
            class: ActionClass::EmailDraft,
            surface: "gmail".to_string(),
            description: "draft".to_string(),
            payload: serde_json::json!({}),
            reversibility: Reversibility::Guided,
        };
        assert!(ActionBus::dispatch_explicit(&store, &request).is_err());
        assert!(ActionBus::dispatch_trusted(&store, &request).is_err());
    }

    #[test]
    fn d1_file_write_receipt_reverts_byte_identical() {
        let (dir, store) = store();
        let task = store.create_task("d1").unwrap();
        let dest = dir.path().join("target.txt");
        fs::write(&dest, b"before bytes").unwrap();
        let receipt = FileWriteAdapter::execute_file_write_approved(
            &store,
            task.id,
            "file",
            "test file write",
            FileWritePayload {
                allowed_root: dir.path().to_path_buf(),
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
        let receipt = FileWriteAdapter::execute_file_write_approved(
            &store,
            task.id,
            "file",
            "create file",
            FileWritePayload {
                allowed_root: dir.path().to_path_buf(),
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
