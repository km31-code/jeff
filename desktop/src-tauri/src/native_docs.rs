// apex d3: native document write-back. Pages and Word use their application
// scripting dictionaries; unsupported, unobservable, or drifted documents fall
// back to guided apply rather than an unsafe buffer swap.
//
// Requesting an edit never runs AppleScript. It persists a mode-0600 operation
// bundle under Jeff's undo root and creates a pending Action Bus receipt. Only
// `approve_native_doc_write` may execute the apply script; the same bundle owns
// the exact anchored revert operation.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    action_bus::{
        ActionClass, ACTION_STATUS_APPLIED, ACTION_STATUS_FAILED, ACTION_STATUS_REJECTED,
        ACTION_STATUS_REVERTED,
    },
    models::{ActionReceiptDto, NativeDocsStatusDto},
    store::TaskStore,
};

pub const AX_BUFFER_WRITEBACK_ENABLED_KEY: &str = "native_docs_ax_buffer_writeback_enabled";
pub const FALLBACK_UNSUPPORTED_SURFACE: &str = "unsupported_surface";
pub const FALLBACK_ANCHOR_MISS: &str = "anchor_miss";
pub const FALLBACK_OBSERVED_TEXT_REQUIRED: &str = "observed_text_required";
#[allow(dead_code)]
pub const FALLBACK_AUTOMATION_PERMISSION: &str = "automation_permission_denied";
#[allow(dead_code)]
pub const FALLBACK_AUTOMATION_UNAVAILABLE: &str = "automation_unavailable";

const BUNDLE_VERSION: u32 = 1;
const MAX_DOCUMENT_TITLE_CHARS: usize = 512;
const MAX_OPERATION_TEXT_CHARS: usize = 200_000;
const MAX_OBSERVED_TEXT_CHARS: usize = 2_000_000;
const MAX_ANCHOR_CHARS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativeDocApp {
    Pages,
    Word,
    Unsupported,
}

impl NativeDocApp {
    pub fn surface(self) -> &'static str {
        match self {
            Self::Pages => "native_docs.pages",
            Self::Word => "native_docs.word",
            Self::Unsupported => "native_docs.unsupported",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pages => "Pages",
            Self::Word => "Microsoft Word",
            Self::Unsupported => "Unsupported app",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeDocsWriteRequest {
    pub task_id: i64,
    pub app_name: String,
    pub document_title: String,
    pub before_text: String,
    pub after_text: String,
    pub anchor_before: String,
    pub anchor_after: String,
    pub observed_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeDocScripts {
    pub permission_probe_script: String,
    pub apply_script: String,
    pub revert_script: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct NativeDocOperationBundle {
    version: u32,
    receipt_id: i64,
    task_id: i64,
    app: NativeDocApp,
    request: NativeDocsWriteRequest,
    scripts: NativeDocScripts,
}

pub fn native_docs_status(store: &TaskStore) -> Result<NativeDocsStatusDto> {
    Ok(NativeDocsStatusDto {
        pages_supported: cfg!(target_os = "macos"),
        word_supported: cfg!(target_os = "macos"),
        automation_permission_status: automation_permission_status().to_string(),
        automation_permission_explainer: automation_permission_explainer().to_string(),
        ax_buffer_writeback_enabled: ax_buffer_writeback_enabled(store)?,
    })
}

pub fn automation_permission_status() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos_automation_permission_required"
    } else {
        "unavailable_on_this_platform"
    }
}

pub fn automation_permission_explainer() -> &'static str {
    "Native Pages and Word edits use macOS Apple Events automation. macOS may ask you to allow Jeff to control Pages or Microsoft Word. If permission is denied, Jeff leaves the document untouched and provides guided placement; Accessibility buffer writeback stays off by default."
}

pub fn ax_buffer_writeback_enabled(store: &TaskStore) -> Result<bool> {
    Ok(store
        .get_app_setting_bool(AX_BUFFER_WRITEBACK_ENABLED_KEY)?
        .unwrap_or(false))
}

pub fn request_native_doc_write(
    store: &TaskStore,
    request: NativeDocsWriteRequest,
) -> Result<ActionReceiptDto> {
    let app = parse_native_doc_app(&request.app_name);
    if app == NativeDocApp::Unsupported {
        return create_guided_receipt(store, &request, app, FALLBACK_UNSUPPORTED_SURFACE);
    }
    validate_request_shape(&request)?;

    let Some(observed_text) = request.observed_text.as_deref() else {
        return create_guided_receipt(
            store,
            &request,
            app,
            FALLBACK_OBSERVED_TEXT_REQUIRED,
        );
    };
    if observed_text.chars().count() > MAX_OBSERVED_TEXT_CHARS {
        return Err(anyhow!("observed native document text exceeds safe length"));
    }
    if !anchors_match(
        observed_text,
        &request.before_text,
        &request.anchor_before,
        &request.anchor_after,
    ) {
        return create_guided_receipt(store, &request, app, FALLBACK_ANCHOR_MISS);
    }

    // The AX flag never diverts this adapter into a buffer swap. It only
    // advertises whether a separate, future last-resort adapter may be offered.
    let _ax_fallback_allowed = ax_buffer_writeback_enabled(store)?;
    let scripts = build_native_doc_scripts(app, &request)?;
    let class = action_class_for_request(&request);
    let payload_excerpt = serde_json::json!({
        "app": app.label(),
        "document_title": request.document_title,
        "mode": class.as_str(),
        "before_text": excerpt(&request.before_text, 120),
        "after_text": excerpt(&request.after_text, 120),
        "anchor_before": request.anchor_before,
        "anchor_after": request.anchor_after,
        "apply_script_kind": "apple_events_scripting_dictionary",
        "revert_script_kind": "persisted_apple_events_scripting_dictionary",
        "approval_required": true,
    })
    .to_string();

    let receipt = store.create_action_receipt(
        request.task_id,
        &class.as_str(),
        app.surface(),
        "L1",
        &format!(
            "Native {} edit for {}",
            app.label(),
            request.document_title.trim()
        ),
        &payload_excerpt,
        "pending_approval",
        None,
        None,
    )?;

    let bundle = NativeDocOperationBundle {
        version: BUNDLE_VERSION,
        receipt_id: receipt.id,
        task_id: request.task_id,
        app,
        request: NativeDocsWriteRequest {
            observed_text: None,
            ..request
        },
        scripts,
    };
    match persist_operation_bundle(store, &bundle) {
        Ok(path) => store.update_action_receipt_status(
            receipt.id,
            "pending_approval",
            None,
            Some(&path.to_string_lossy()),
        ),
        Err(error) => {
            let reason = excerpt(&format!("failed to persist native-doc undo bundle: {error}"), 240);
            let failed = store.update_action_receipt_status(
                receipt.id,
                ACTION_STATUS_FAILED,
                Some(&reason),
                None,
            )?;
            let _ = crate::trust::record_receipt_outcome(store, &failed);
            Err(error)
        }
    }
}

// Execute only after the caller has received explicit user approval. The
// permission probe is non-mutating and runs while the receipt is still pending,
// so a denied Automation grant can transition cleanly to guided apply.
#[allow(dead_code)]
pub fn approve_native_doc_write(
    store: &TaskStore,
    receipt_id: i64,
) -> Result<ActionReceiptDto> {
    if !cfg!(target_os = "macos") {
        return guide_pending_native_receipt(
            store,
            receipt_id,
            FALLBACK_AUTOMATION_UNAVAILABLE,
        );
    }
    approve_native_doc_write_with_executor(store, receipt_id, execute_osascript)
}

#[allow(dead_code)]
pub fn reject_native_doc_write(store: &TaskStore, receipt_id: i64) -> Result<ActionReceiptDto> {
    let receipt = require_native_receipt(store, receipt_id)?;
    if receipt.status == ACTION_STATUS_REJECTED {
        return Ok(receipt);
    }
    if !matches!(receipt.status.as_str(), "pending_approval" | "approved") {
        return Err(anyhow!(
            "native document receipt id={receipt_id} cannot be rejected from status {}",
            receipt.status
        ));
    }
    let rejected = store.update_action_receipt_status(
        receipt_id,
        ACTION_STATUS_REJECTED,
        None,
        receipt.undo_ref.as_deref(),
    )?;
    if let Some(path) = receipt.undo_ref.as_deref() {
        let _ = remove_validated_bundle(store, path);
    }
    let _ = crate::trust::record_receipt_outcome(store, &rejected);
    Ok(rejected)
}

#[allow(dead_code)]
pub fn revert_native_doc_write(
    store: &TaskStore,
    receipt_id: i64,
) -> Result<ActionReceiptDto> {
    if !cfg!(target_os = "macos") {
        return Err(anyhow!("native document revert is unavailable on this platform"));
    }
    revert_native_doc_write_with_executor(store, receipt_id, execute_osascript)
}

#[allow(dead_code)]
fn approve_native_doc_write_with_executor<F>(
    store: &TaskStore,
    receipt_id: i64,
    executor: F,
) -> Result<ActionReceiptDto>
where
    F: Fn(&str) -> Result<()>,
{
    let receipt = require_native_receipt(store, receipt_id)?;
    if receipt.status == ACTION_STATUS_APPLIED {
        return Ok(receipt);
    }
    if !matches!(receipt.status.as_str(), "pending_approval" | "approved") {
        return Err(anyhow!(
            "native document receipt id={receipt_id} cannot be applied from status {}",
            receipt.status
        ));
    }
    let bundle = load_operation_bundle(store, &receipt)?;

    if let Err(error) = executor(&bundle.scripts.permission_probe_script) {
        let reason = error.to_string();
        if is_automation_permission_error(&reason) && receipt.status == "pending_approval" {
            return guide_pending_native_receipt(
                store,
                receipt_id,
                FALLBACK_AUTOMATION_PERMISSION,
            );
        }
        let failed = store.update_action_receipt_status(
            receipt_id,
            ACTION_STATUS_FAILED,
            Some(&excerpt(&reason, 240)),
            receipt.undo_ref.as_deref(),
        )?;
        let _ = crate::trust::record_receipt_outcome(store, &failed);
        return Ok(failed);
    }

    let applying = store.update_action_receipt_status(
        receipt_id,
        "applying",
        None,
        receipt.undo_ref.as_deref(),
    )?;
    match executor(&bundle.scripts.apply_script) {
        Ok(()) => {
            let applied = store.update_action_receipt_status(
                receipt_id,
                ACTION_STATUS_APPLIED,
                None,
                applying.undo_ref.as_deref(),
            )?;
            crate::trust::record_receipt_outcome(store, &applied)?;
            Ok(applied)
        }
        Err(error) => {
            let reason = excerpt(&error.to_string(), 240);
            let failed = store.update_action_receipt_status(
                receipt_id,
                ACTION_STATUS_FAILED,
                Some(&reason),
                applying.undo_ref.as_deref(),
            )?;
            let _ = create_guided_live_edit_for_bundle(store, &bundle, &reason);
            let _ = crate::trust::record_receipt_outcome(store, &failed);
            Ok(failed)
        }
    }
}

#[allow(dead_code)]
fn revert_native_doc_write_with_executor<F>(
    store: &TaskStore,
    receipt_id: i64,
    executor: F,
) -> Result<ActionReceiptDto>
where
    F: Fn(&str) -> Result<()>,
{
    let receipt = require_native_receipt(store, receipt_id)?;
    if receipt.status == ACTION_STATUS_REVERTED {
        return Ok(receipt);
    }
    if receipt.status != ACTION_STATUS_APPLIED {
        return Err(anyhow!(
            "native document receipt id={receipt_id} can only revert from applied status"
        ));
    }
    let bundle = load_operation_bundle(store, &receipt)?;
    executor(&bundle.scripts.permission_probe_script)
        .context("native document revert permission check failed")?;
    // The revert script contains the same strict title + unique anchor checks,
    // now surrounding after_text. Drift therefore fails without a wrong write.
    executor(&bundle.scripts.revert_script).context("native document revert failed")?;
    let reverted = store.update_action_receipt_status(
        receipt_id,
        ACTION_STATUS_REVERTED,
        None,
        receipt.undo_ref.as_deref(),
    )?;
    crate::trust::record_receipt_outcome(store, &reverted)?;
    Ok(reverted)
}

pub fn parse_native_doc_app(app_name: &str) -> NativeDocApp {
    let normalized = app_name.trim().to_ascii_lowercase();
    if normalized == "pages" || normalized.contains("apple pages") {
        NativeDocApp::Pages
    } else if normalized == "word"
        || normalized == "microsoft word"
        || normalized.contains("word.app")
    {
        NativeDocApp::Word
    } else {
        NativeDocApp::Unsupported
    }
}

pub fn build_native_doc_scripts(
    app: NativeDocApp,
    request: &NativeDocsWriteRequest,
) -> Result<NativeDocScripts> {
    let permission_probe_script = match app {
        NativeDocApp::Pages => {
            "tell application \"Pages\" to return version".to_string()
        }
        NativeDocApp::Word => {
            "tell application \"Microsoft Word\" to return version".to_string()
        }
        NativeDocApp::Unsupported => return Err(anyhow!("unsupported native document app")),
    };
    let build = |before: &str, after: &str| match app {
        NativeDocApp::Pages => build_pages_script(
            &request.document_title,
            before,
            after,
            &request.anchor_before,
            &request.anchor_after,
        ),
        NativeDocApp::Word => build_word_script(
            &request.document_title,
            before,
            after,
            &request.anchor_before,
            &request.anchor_after,
        ),
        NativeDocApp::Unsupported => unreachable!(),
    };
    Ok(NativeDocScripts {
        permission_probe_script,
        apply_script: build(&request.before_text, &request.after_text),
        revert_script: build(&request.after_text, &request.before_text),
    })
}

pub fn build_pages_script(
    document_title: &str,
    before_text: &str,
    after_text: &str,
    anchor_before: &str,
    anchor_after: &str,
) -> String {
    let title = apple_script_literal(document_title);
    let before = apple_script_literal(before_text);
    let after = apple_script_literal(after_text);
    let leading = apple_script_literal(anchor_before);
    let trailing = apple_script_literal(anchor_after);
    format!(
        r#"tell application "Pages"
  activate
  if not (exists front document) then error "No Pages document is open"
  set expectedTitle to {title}
  if expectedTitle is "" or name of front document is not expectedTitle then error "Pages document title mismatch"
  set bodyText to body text of front document as text
  set targetText to {before}
  set replacementText to {after}
  set anchorBeforeText to {leading}
  set anchorAfterText to {trailing}
  set needleText to anchorBeforeText & targetText & anchorAfterText
  if needleText is "" then error "Anchored target is empty"
  set needleOffset to offset of needleText in bodyText
  if needleOffset is 0 then error "Anchored target not found"
  set tailStart to needleOffset + 1
  if tailStart <= (length of bodyText) then
    set tailText to text tailStart thru -1 of bodyText
    if (offset of needleText in tailText) is not 0 then error "Anchored target is ambiguous"
  end if
  set targetOffset to needleOffset + (length of anchorBeforeText)
  if targetText is "" then
    if targetOffset > (length of bodyText) then
      set insertion point -1 of body text of front document to replacementText
    else
      set insertion point targetOffset of body text of front document to replacementText
    end if
  else
    set targetEnd to targetOffset + (length of targetText) - 1
    set characters targetOffset thru targetEnd of body text of front document to replacementText
  end if
end tell"#
    )
}

pub fn build_word_script(
    document_title: &str,
    before_text: &str,
    after_text: &str,
    anchor_before: &str,
    anchor_after: &str,
) -> String {
    let title = apple_script_literal(document_title);
    let before = apple_script_literal(before_text);
    let after = apple_script_literal(after_text);
    let leading = apple_script_literal(anchor_before);
    let trailing = apple_script_literal(anchor_after);
    format!(
        r#"tell application "Microsoft Word"
  activate
  if not (exists active document) then error "No Word document is open"
  set expectedTitle to {title}
  if expectedTitle is "" or name of active document is not expectedTitle then error "Word document title mismatch"
  set docText to content of text object of active document as text
  set targetText to {before}
  set replacementText to {after}
  set anchorBeforeText to {leading}
  set anchorAfterText to {trailing}
  set needleText to anchorBeforeText & targetText & anchorAfterText
  if needleText is "" then error "Anchored target is empty"
  set needleOffset to offset of needleText in docText
  if needleOffset is 0 then error "Anchored target not found"
  set tailStart to needleOffset + 1
  if tailStart <= (length of docText) then
    set tailText to text tailStart thru -1 of docText
    if (offset of needleText in tailText) is not 0 then error "Anchored target is ambiguous"
  end if
  set rangeStart to (needleOffset - 1) + (length of anchorBeforeText)
  set rangeEnd to rangeStart + (length of targetText)
  set targetRange to create range active document start rangeStart end rangeEnd
  set content of targetRange to replacementText
end tell"#
    )
}

// True only when exactly one occurrence matches the target and both supplied
// surrounding anchors. Missing anchors are never treated as permission to use
// a first-occurrence replacement.
pub fn anchors_match(
    observed_text: &str,
    before_text: &str,
    anchor_before: &str,
    anchor_after: &str,
) -> bool {
    let observed = normalize_for_anchor(observed_text);
    let before = normalize_for_anchor(before_text);
    let leading = normalize_for_anchor(anchor_before);
    let trailing = normalize_for_anchor(anchor_after);
    if leading.is_empty() && trailing.is_empty() {
        return false;
    }

    if before.is_empty() {
        let needle = normalize_for_anchor(&format!("{anchor_before}{anchor_after}"));
        return !needle.is_empty() && overlapping_occurrences(&observed, &needle) == 1;
    }

    let mut matching = 0usize;
    let mut search_from = 0usize;
    while search_from <= observed.len() {
        let Some(offset) = observed[search_from..].find(&before) else {
            break;
        };
        let start = search_from + offset;
        let end = start + before.len();
        let before_ok = leading.is_empty() || observed[..start].trim_end().ends_with(&leading);
        let after_ok = trailing.is_empty() || observed[end..].trim_start().starts_with(&trailing);
        if before_ok && after_ok {
            matching += 1;
            if matching > 1 {
                return false;
            }
        }
        search_from = start.saturating_add(1);
    }
    matching == 1
}

fn action_class_for_request(request: &NativeDocsWriteRequest) -> ActionClass {
    if request.before_text.is_empty() {
        ActionClass::DocInsert
    } else {
        ActionClass::DocReplace
    }
}

fn validate_request_shape(request: &NativeDocsWriteRequest) -> Result<()> {
    if request.task_id <= 0 {
        return Err(anyhow!("native document task id must be positive"));
    }
    let title = request.document_title.trim();
    if title.is_empty() || title.chars().count() > MAX_DOCUMENT_TITLE_CHARS {
        return Err(anyhow!("native document title is missing or too long"));
    }
    for (label, value, limit) in [
        ("document_title", request.document_title.as_str(), MAX_DOCUMENT_TITLE_CHARS),
        ("before_text", request.before_text.as_str(), MAX_OPERATION_TEXT_CHARS),
        ("after_text", request.after_text.as_str(), MAX_OPERATION_TEXT_CHARS),
        ("anchor_before", request.anchor_before.as_str(), MAX_ANCHOR_CHARS),
        ("anchor_after", request.anchor_after.as_str(), MAX_ANCHOR_CHARS),
    ] {
        if value.contains('\0') {
            return Err(anyhow!("native document {label} contains a NUL byte"));
        }
        if value.chars().count() > limit {
            return Err(anyhow!("native document {label} exceeds safe length"));
        }
    }
    if request.before_text == request.after_text {
        return Err(anyhow!("native document edit would be a no-op"));
    }
    if request.anchor_before.is_empty() && request.anchor_after.is_empty() {
        return Err(anyhow!("native document edit requires surrounding anchor context"));
    }
    Ok(())
}

fn persist_operation_bundle(store: &TaskStore, bundle: &NativeDocOperationBundle) -> Result<PathBuf> {
    let directory = native_docs_bundle_root(store)?;
    let bytes = serde_json::to_vec_pretty(bundle).context("failed to serialize native-doc bundle")?;
    let digest = sha256_hex(&bytes);
    let path = directory.join(format!("receipt-{}-{digest}.json", bundle.receipt_id));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .with_context(|| format!("failed to create native-doc bundle {}", path.display()))?;
    file.write_all(&bytes)
        .context("failed to write native-doc bundle")?;
    file.sync_all().context("failed to sync native-doc bundle")?;
    Ok(path)
}

fn native_docs_bundle_root(store: &TaskStore) -> Result<PathBuf> {
    let directory = store.action_undo_root().join("native_docs");
    fs::create_dir_all(&directory)
        .with_context(|| format!("failed to create native-doc undo root {}", directory.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .context("failed to secure native-doc undo root")?;
    }
    Ok(directory)
}

#[allow(dead_code)]
fn load_operation_bundle(
    store: &TaskStore,
    receipt: &ActionReceiptDto,
) -> Result<NativeDocOperationBundle> {
    let undo_ref = receipt
        .undo_ref
        .as_deref()
        .ok_or_else(|| anyhow!("native document receipt id={} has no operation bundle", receipt.id))?;
    let path = validated_bundle_path(store, undo_ref)?;
    let bytes = fs::read(&path)
        .with_context(|| format!("failed to read native-doc bundle {}", path.display()))?;
    let digest = sha256_hex(&bytes);
    let expected_suffix = format!("-{digest}.json");
    if !path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.ends_with(&expected_suffix))
        .unwrap_or(false)
    {
        return Err(anyhow!("native document operation bundle integrity check failed"));
    }
    let bundle: NativeDocOperationBundle =
        serde_json::from_slice(&bytes).context("failed to parse native-doc bundle")?;
    if bundle.version != BUNDLE_VERSION
        || bundle.receipt_id != receipt.id
        || bundle.task_id != receipt.task_id
        || bundle.app.surface() != receipt.surface
    {
        return Err(anyhow!("native document operation bundle does not match receipt"));
    }
    validate_request_shape(&bundle.request)?;
    let regenerated = build_native_doc_scripts(bundle.app, &bundle.request)?;
    if regenerated != bundle.scripts {
        return Err(anyhow!("native document operation scripts failed regeneration check"));
    }
    Ok(bundle)
}

#[allow(dead_code)]
fn validated_bundle_path(store: &TaskStore, undo_ref: &str) -> Result<PathBuf> {
    let root = native_docs_bundle_root(store)?
        .canonicalize()
        .context("failed to canonicalize native-doc undo root")?;
    let requested = Path::new(undo_ref);
    let metadata = fs::symlink_metadata(requested)
        .with_context(|| format!("native-doc bundle is unavailable: {}", requested.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(anyhow!("native-doc bundle must be a regular non-symlink file"));
    }
    let canonical = requested
        .canonicalize()
        .context("failed to canonicalize native-doc bundle")?;
    if !canonical.starts_with(&root) {
        return Err(anyhow!("native-doc bundle escaped the undo root"));
    }
    Ok(canonical)
}

#[allow(dead_code)]
fn remove_validated_bundle(store: &TaskStore, undo_ref: &str) -> Result<()> {
    let path = validated_bundle_path(store, undo_ref)?;
    fs::remove_file(path).context("failed to remove rejected native-doc bundle")
}

#[allow(dead_code)]
fn require_native_receipt(store: &TaskStore, receipt_id: i64) -> Result<ActionReceiptDto> {
    let receipt = store
        .get_action_receipt(receipt_id)?
        .ok_or_else(|| anyhow!("action receipt id={receipt_id} not found"))?;
    if !matches!(receipt.surface.as_str(), "native_docs.pages" | "native_docs.word") {
        return Err(anyhow!("receipt id={receipt_id} is not a native document action"));
    }
    if !matches!(receipt.class.as_str(), "doc.insert" | "doc.replace") {
        return Err(anyhow!("receipt id={receipt_id} has an invalid native document class"));
    }
    Ok(receipt)
}

#[allow(dead_code)]
fn guide_pending_native_receipt(
    store: &TaskStore,
    receipt_id: i64,
    reason: &str,
) -> Result<ActionReceiptDto> {
    let receipt = require_native_receipt(store, receipt_id)?;
    if receipt.status == "guided" {
        return Ok(receipt);
    }
    if receipt.status != "pending_approval" {
        return Err(anyhow!(
            "native document receipt id={receipt_id} cannot become guided from status {}",
            receipt.status
        ));
    }
    let bundle = load_operation_bundle(store, &receipt)?;
    let guided = store.update_action_receipt_status(
        receipt_id,
        "guided",
        Some(reason),
        receipt.undo_ref.as_deref(),
    )?;
    let _ = create_guided_live_edit_for_bundle(store, &bundle, reason);
    let _ = crate::trust::record_receipt_outcome(store, &guided);
    Ok(guided)
}

#[allow(dead_code)]
fn create_guided_live_edit_for_bundle(
    store: &TaskStore,
    bundle: &NativeDocOperationBundle,
    reason: &str,
) -> Result<()> {
    store.create_guided_live_edit_receipt(
        Some(bundle.task_id),
        bundle.receipt_id,
        bundle.app.surface(),
        &bundle.request.document_title,
        reason,
        &bundle.request.before_text,
        &bundle.request.after_text,
    )?;
    Ok(())
}

fn create_guided_receipt(
    store: &TaskStore,
    request: &NativeDocsWriteRequest,
    app: NativeDocApp,
    reason: &str,
) -> Result<ActionReceiptDto> {
    let class = action_class_for_request(request);
    let payload_excerpt = serde_json::json!({
        "app": request.app_name,
        "document_title": request.document_title,
        "before_text": excerpt(&request.before_text, 120),
        "after_text": excerpt(&request.after_text, 120),
        "anchor_before": request.anchor_before,
        "anchor_after": request.anchor_after,
        "fallback_reason": reason,
        "guided_apply": true,
    })
    .to_string();
    let receipt = store.create_action_receipt(
        request.task_id,
        &class.as_str(),
        app.surface(),
        "L1",
        &format!(
            "Guided native document edit for {}",
            request.document_title.trim()
        ),
        &payload_excerpt,
        "guided",
        Some(reason),
        None,
    )?;
    let _ = store.create_guided_live_edit_receipt(
        Some(request.task_id),
        receipt.id,
        app.surface(),
        &request.document_title,
        reason,
        &request.before_text,
        &request.after_text,
    );
    let _ = crate::trust::record_receipt_outcome(store, &receipt);
    Ok(receipt)
}

#[allow(dead_code)]
fn execute_osascript(script: &str) -> Result<()> {
    let output = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .output()
        .context("failed to start osascript")?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    Err(anyhow!("AppleScript failed: {}", excerpt(detail, 500)))
}

#[allow(dead_code)]
fn is_automation_permission_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("-1743")
        || lower.contains("not authorized to send apple events")
        || lower.contains("not authorised to send apple events")
        || lower.contains("automation permission")
}

fn normalize_for_anchor(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn overlapping_occurrences(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let mut count = 0usize;
    let mut from = 0usize;
    while from <= haystack.len().saturating_sub(needle.len()) {
        let Some(offset) = haystack[from..].find(needle) else {
            break;
        };
        count += 1;
        from = from + offset + 1;
    }
    count
}

fn apple_script_literal(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

fn excerpt(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn sha256_hex(value: &[u8]) -> String {
    let digest = Sha256::digest(value);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    fn store() -> (TempDir, TaskStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        (dir, store)
    }

    fn request(app_name: &str) -> NativeDocsWriteRequest {
        NativeDocsWriteRequest {
            task_id: 1,
            app_name: app_name.to_string(),
            document_title: "Quarterly plan".to_string(),
            before_text: "old sentence".to_string(),
            after_text: "new sentence".to_string(),
            anchor_before: "Intro ".to_string(),
            anchor_after: " outro".to_string(),
            observed_text: Some("Intro old sentence outro".to_string()),
        }
    }

    #[test]
    fn d3_pages_script_uses_exact_title_unique_anchor_and_dictionary_range() {
        let script = build_pages_script("Doc", "old", "new", "Intro ", " outro");
        assert!(script.contains("tell application \"Pages\""));
        assert!(script.contains("body text of front document"));
        assert!(script.contains("name of front document is not expectedTitle"));
        assert!(script.contains("Anchored target is ambiguous"));
        assert!(script.contains("characters targetOffset thru targetEnd"));
        assert!(!script.contains("does not contain expectedTitle"));
        assert!(!script.contains("AXValue"));
        assert!(!script.contains("kAX"));
    }

    #[test]
    fn d3_word_script_uses_exact_anchored_range_not_global_find() {
        let script = build_word_script("Doc", "old", "new", "Intro ", " outro");
        assert!(script.contains("tell application \"Microsoft Word\""));
        assert!(script.contains("name of active document is not expectedTitle"));
        assert!(script.contains("create range active document start rangeStart end rangeEnd"));
        assert!(script.contains("Anchored target is ambiguous"));
        assert!(!script.contains("find object"));
        assert!(!script.contains("replace replace one"));
        assert!(!script.contains("AXValue"));
    }

    #[test]
    fn d3_ax_buffer_writeback_defaults_off() {
        let (_dir, store) = store();
        assert!(!ax_buffer_writeback_enabled(&store).unwrap());
    }

    #[test]
    fn d3_unsupported_app_returns_guided_receipt_and_card() {
        let (_dir, store) = store();
        let task = store.create_task("d3").unwrap();
        let mut req = request("Notion");
        req.task_id = task.id;
        let receipt = request_native_doc_write(&store, req).unwrap();
        assert_eq!(receipt.status, "guided");
        assert_eq!(receipt.failure_reason.as_deref(), Some(FALLBACK_UNSUPPORTED_SURFACE));
        let cards = store.get_unresolved_live_edits().unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].status, "fallback");
    }

    #[test]
    fn d3_missing_observation_is_guided_without_pending_execution() {
        let (_dir, store) = store();
        let task = store.create_task("d3 observation").unwrap();
        let mut req = request("Pages");
        req.task_id = task.id;
        req.observed_text = None;
        let receipt = request_native_doc_write(&store, req).unwrap();
        assert_eq!(receipt.status, "guided");
        assert_eq!(
            receipt.failure_reason.as_deref(),
            Some(FALLBACK_OBSERVED_TEXT_REQUIRED)
        );
        assert!(receipt.undo_ref.is_none());
    }

    #[test]
    fn d3_anchor_drift_or_ambiguity_routes_guided() {
        let (_dir, store) = store();
        let task = store.create_task("d3 drift").unwrap();
        let mut drift = request("Pages");
        drift.task_id = task.id;
        drift.observed_text = Some("Intro changed sentence outro".to_string());
        let receipt = request_native_doc_write(&store, drift).unwrap();
        assert_eq!(receipt.status, "guided");

        let mut ambiguous = request("Word");
        ambiguous.task_id = task.id;
        ambiguous.observed_text = Some(
            "Intro old sentence outro / Intro old sentence outro".to_string(),
        );
        let receipt = request_native_doc_write(&store, ambiguous).unwrap();
        assert_eq!(receipt.status, "guided");
        assert_eq!(receipt.failure_reason.as_deref(), Some(FALLBACK_ANCHOR_MISS));
    }

    #[test]
    fn d3_request_persists_private_revertable_bundle_without_executing() {
        let (_dir, store) = store();
        let task = store.create_task("d3 pages").unwrap();
        let mut req = request("Pages");
        req.task_id = task.id;
        let receipt = request_native_doc_write(&store, req).unwrap();
        assert_eq!(receipt.status, "pending_approval");
        let path = PathBuf::from(receipt.undo_ref.as_deref().unwrap());
        assert!(path.starts_with(store.action_undo_root().join("native_docs")));
        assert!(path.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let bundle = load_operation_bundle(&store, &receipt).unwrap();
        assert!(bundle.scripts.revert_script.contains("new sentence"));
        assert!(bundle.scripts.revert_script.contains("old sentence"));
    }

    #[test]
    fn d3_approval_executes_probe_then_apply_and_revert_uses_persisted_script() {
        let (_dir, store) = store();
        let task = store.create_task("d3 lifecycle").unwrap();
        let mut req = request("Word");
        req.task_id = task.id;
        let pending = request_native_doc_write(&store, req).unwrap();
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let approval_calls = Arc::clone(&calls);
        let applied = approve_native_doc_write_with_executor(&store, pending.id, move |script| {
            approval_calls.lock().unwrap().push(script.to_string());
            Ok(())
        })
        .unwrap();
        assert_eq!(applied.status, ACTION_STATUS_APPLIED);
        let approval_scripts = calls.lock().unwrap();
        assert_eq!(approval_scripts.len(), 2);
        assert!(approval_scripts[0].contains("return version"));
        assert!(approval_scripts[1].contains("new sentence"));
        drop(approval_scripts);

        let revert_calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured = Arc::clone(&revert_calls);
        let reverted = revert_native_doc_write_with_executor(&store, pending.id, move |script| {
            captured.lock().unwrap().push(script.to_string());
            Ok(())
        })
        .unwrap();
        assert_eq!(reverted.status, ACTION_STATUS_REVERTED);
        let scripts = revert_calls.lock().unwrap();
        assert_eq!(scripts.len(), 2);
        assert!(scripts[1].contains("new sentence"));
        assert!(scripts[1].contains("old sentence"));
    }

    #[test]
    fn d3_permission_denial_never_runs_apply_and_becomes_guided() {
        let (_dir, store) = store();
        let task = store.create_task("d3 permission").unwrap();
        let mut req = request("Pages");
        req.task_id = task.id;
        let pending = request_native_doc_write(&store, req).unwrap();
        let calls = Arc::new(Mutex::new(0usize));
        let captured = Arc::clone(&calls);
        let guided = approve_native_doc_write_with_executor(&store, pending.id, move |_script| {
            *captured.lock().unwrap() += 1;
            Err(anyhow!("Not authorized to send Apple events (-1743)"))
        })
        .unwrap();
        assert_eq!(*calls.lock().unwrap(), 1);
        assert_eq!(guided.status, "guided");
        assert_eq!(
            guided.failure_reason.as_deref(),
            Some(FALLBACK_AUTOMATION_PERMISSION)
        );
        assert_eq!(store.get_unresolved_live_edits().unwrap().len(), 1);
    }

    #[test]
    fn d3_reject_deletes_unneeded_bundle() {
        let (_dir, store) = store();
        let task = store.create_task("d3 reject").unwrap();
        let mut req = request("Pages");
        req.task_id = task.id;
        let pending = request_native_doc_write(&store, req).unwrap();
        let path = PathBuf::from(pending.undo_ref.as_deref().unwrap());
        let rejected = reject_native_doc_write(&store, pending.id).unwrap();
        assert_eq!(rejected.status, ACTION_STATUS_REJECTED);
        assert!(!path.exists());
    }

    #[test]
    fn d3_bundle_path_cannot_escape_undo_root() {
        let (_dir, store) = store();
        let task = store.create_task("d3 tamper").unwrap();
        let mut req = request("Pages");
        req.task_id = task.id;
        let pending = request_native_doc_write(&store, req).unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        let tampered = store
            .update_action_receipt_status(
                pending.id,
                "pending_approval",
                None,
                Some(&outside.path().to_string_lossy()),
            )
            .unwrap();
        let error = load_operation_bundle(&store, &tampered).unwrap_err();
        assert!(error.to_string().contains("escaped the undo root"));
    }

    #[test]
    fn d3_apple_script_literals_escape_control_characters() {
        let script = build_pages_script(
            "A \"quoted\" title",
            "old\nline",
            "new\\value",
            "before ",
            " after",
        );
        assert!(script.contains("A \\\"quoted\\\" title"));
        assert!(script.contains("old\\nline"));
        assert!(script.contains("new\\\\value"));
    }
}
