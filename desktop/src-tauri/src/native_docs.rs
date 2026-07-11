// apex d3: native document write-back. Pages and Word use their application
// scripting dictionaries; unsupported or drifted documents fall back to guided
// apply receipts rather than an unsafe buffer swap.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::{
    action_bus::ActionClass,
    models::{ActionReceiptDto, NativeDocsStatusDto},
    store::TaskStore,
};

pub const AX_BUFFER_WRITEBACK_ENABLED_KEY: &str = "native_docs_ax_buffer_writeback_enabled";
pub const FALLBACK_UNSUPPORTED_SURFACE: &str = "unsupported_surface";
pub const FALLBACK_ANCHOR_MISS: &str = "anchor_miss";

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
    pub apply_script: String,
    pub revert_script: String,
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
    "Native Pages and Word edits use macOS Apple Events automation. macOS may ask you to allow Jeff to control Pages or Microsoft Word; Jeff does not use Accessibility buffer writeback unless the explicit native-docs AX fallback flag is enabled."
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

    if let Some(observed_text) = request.observed_text.as_deref() {
        if !anchors_match(
            observed_text,
            &request.before_text,
            &request.anchor_before,
            &request.anchor_after,
        ) {
            return create_guided_receipt(store, &request, app, FALLBACK_ANCHOR_MISS);
        }
    }

    if ax_buffer_writeback_enabled(store)? {
        return Err(anyhow!(
            "AX buffer writeback is intentionally not used for native docs; scripting adapters are required"
        ));
    }

    let scripts = build_native_doc_scripts(app, &request)?;
    let class = if request.before_text.trim().is_empty() {
        ActionClass::DocInsert
    } else {
        ActionClass::DocReplace
    };
    let payload_excerpt = serde_json::json!({
        "app": app.label(),
        "document_title": request.document_title,
        "mode": class.as_str(),
        "before_text": excerpt(&request.before_text, 120),
        "after_text": excerpt(&request.after_text, 120),
        "anchor_before": request.anchor_before,
        "anchor_after": request.anchor_after,
        "apply_script_kind": "apple_events_scripting_dictionary",
        "revert_script_kind": "apple_events_scripting_dictionary",
        "apply_script": excerpt(&scripts.apply_script, 220),
    })
    .to_string();

    store.create_action_receipt(
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
        Some(&format!(
            "native-docs-revert:{}:{}",
            app.surface(),
            stable_script_hash(&scripts.revert_script)
        )),
    )
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
    match app {
        NativeDocApp::Pages => Ok(NativeDocScripts {
            apply_script: build_pages_script(
                &request.document_title,
                &request.before_text,
                &request.after_text,
                &request.anchor_before,
            ),
            revert_script: build_pages_script(
                &request.document_title,
                &request.after_text,
                &request.before_text,
                &request.anchor_before,
            ),
        }),
        NativeDocApp::Word => Ok(NativeDocScripts {
            apply_script: build_word_script(
                &request.document_title,
                &request.before_text,
                &request.after_text,
                &request.anchor_before,
            ),
            revert_script: build_word_script(
                &request.document_title,
                &request.after_text,
                &request.before_text,
                &request.anchor_before,
            ),
        }),
        NativeDocApp::Unsupported => Err(anyhow!("unsupported native document app")),
    }
}

pub fn build_pages_script(
    document_title: &str,
    before_text: &str,
    after_text: &str,
    anchor_before: &str,
) -> String {
    let title = apple_script_literal(document_title);
    let before = apple_script_literal(before_text);
    let after = apple_script_literal(after_text);
    let anchor = apple_script_literal(anchor_before);
    format!(
        r#"tell application "Pages"
  activate
  if not (exists front document) then error "No Pages document is open"
  set expectedTitle to {title}
  if expectedTitle is not "" and name of front document does not contain expectedTitle then error "Pages document title mismatch"
  set bodyText to body text of front document
  set targetText to {before}
  set replacementText to {after}
  set anchorText to {anchor}
  if targetText is "" then
    if anchorText is "" then
      set body text of front document to bodyText & replacementText
    else
      set anchorOffset to offset of anchorText in bodyText
      if anchorOffset is 0 then error "Anchor text not found"
      set insertionPoint to anchorOffset + (length of anchorText)
      set character insertionPoint of body text of front document to replacementText & character insertionPoint of body text of front document
    end if
  else
    set targetOffset to offset of targetText in bodyText
    if targetOffset is 0 then error "Target text not found"
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
) -> String {
    let title = apple_script_literal(document_title);
    let before = apple_script_literal(before_text);
    let after = apple_script_literal(after_text);
    let anchor = apple_script_literal(anchor_before);
    format!(
        r#"tell application "Microsoft Word"
  activate
  if not (exists active document) then error "No Word document is open"
  set expectedTitle to {title}
  if expectedTitle is not "" and name of active document does not contain expectedTitle then error "Word document title mismatch"
  set targetText to {before}
  set replacementText to {after}
  set anchorText to {anchor}
  set docText to content of text object of active document
  if targetText is "" then
    if anchorText is "" then
      insert text replacementText at after (end of content of text object of active document)
    else
      set anchorOffset to offset of anchorText in docText
      if anchorOffset is 0 then error "Anchor text not found"
      set insertionStart to anchorOffset + (length of anchorText)
      set insertionRange to create range active document start insertionStart end insertionStart
      insert text replacementText at insertionRange
    end if
  else
    set findObject to find object of text object of active document
    clear formatting findObject
    set content of findObject to targetText
    set content of replacement of findObject to replacementText
    execute find findObject replace replace one
  end if
end tell"#
    )
}

pub fn anchors_match(
    observed_text: &str,
    before_text: &str,
    anchor_before: &str,
    anchor_after: &str,
) -> bool {
    let observed = normalize_for_anchor(observed_text);
    let before = normalize_for_anchor(before_text);
    let anchor_before = normalize_for_anchor(anchor_before);
    let anchor_after = normalize_for_anchor(anchor_after);
    if before.is_empty() {
        return anchor_before.is_empty() || observed.contains(&anchor_before);
    }

    let mut search_from = 0;
    while let Some(offset) = observed[search_from..].find(&before) {
        let start = search_from + offset;
        let end = start + before.len();
        let before_ok =
            anchor_before.is_empty() || observed[..start].trim_end().ends_with(&anchor_before);
        let after_ok =
            anchor_after.is_empty() || observed[end..].trim_start().starts_with(&anchor_after);
        if before_ok && after_ok {
            return true;
        }
        search_from = end;
    }
    false
}

fn create_guided_receipt(
    store: &TaskStore,
    request: &NativeDocsWriteRequest,
    app: NativeDocApp,
    reason: &str,
) -> Result<ActionReceiptDto> {
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
        &ActionClass::DocReplace.as_str(),
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
    Ok(receipt)
}

fn normalize_for_anchor(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn apple_script_literal(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn excerpt(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn stable_script_hash(value: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
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
    fn d3_pages_script_uses_scripting_dictionary_not_ax_buffer() {
        let script = build_pages_script("Doc", "old", "new", "Intro ");
        assert!(script.contains("tell application \"Pages\""));
        assert!(script.contains("body text of front document"));
        assert!(script.contains("characters targetOffset thru targetEnd"));
        assert!(!script.contains("AXValue"));
        assert!(!script.contains("kAX"));
    }

    #[test]
    fn d3_word_script_uses_word_find_dictionary_not_ax_buffer() {
        let script = build_word_script("Doc", "old", "new", "Intro ");
        assert!(script.contains("tell application \"Microsoft Word\""));
        assert!(script.contains("find object of text object of active document"));
        assert!(script.contains("replace replace one"));
        assert!(!script.contains("AXValue"));
        assert!(!script.contains("kAX"));
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
        assert_eq!(
            receipt.failure_reason.as_deref(),
            Some(FALLBACK_UNSUPPORTED_SURFACE)
        );
        let cards = store.get_unresolved_live_edits().unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].status, "fallback");
    }

    #[test]
    fn d3_anchor_drift_routes_guided_without_script_receipt() {
        let (_dir, store) = store();
        let task = store.create_task("d3 drift").unwrap();
        let mut req = request("Pages");
        req.task_id = task.id;
        req.observed_text = Some("Intro changed sentence outro".to_string());
        let receipt = request_native_doc_write(&store, req).unwrap();
        assert_eq!(receipt.status, "guided");
        assert_eq!(
            receipt.failure_reason.as_deref(),
            Some(FALLBACK_ANCHOR_MISS)
        );
        assert!(!receipt.payload_excerpt.contains("apply_script_kind"));
    }

    #[test]
    fn d3_pages_supported_request_creates_pending_revertable_receipt() {
        let (_dir, store) = store();
        let task = store.create_task("d3 pages").unwrap();
        let mut req = request("Pages");
        req.task_id = task.id;
        let receipt = request_native_doc_write(&store, req).unwrap();
        assert_eq!(receipt.status, "pending_approval");
        assert_eq!(receipt.class, "doc.replace");
        assert_eq!(receipt.surface, "native_docs.pages");
        assert!(receipt
            .undo_ref
            .as_deref()
            .unwrap_or("")
            .contains("native-docs-revert"));
        assert!(receipt
            .payload_excerpt
            .contains("apple_events_scripting_dictionary"));
    }
}
