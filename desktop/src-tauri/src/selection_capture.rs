// phase 22: explicit selected-text capture. captures are hotkey/extension
// triggered only, held in memory, and consumed by the next chat turn.

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::{
    context_observer::ActiveWindowContext,
    models::{
        BrowserSelectionCaptureRequestDto, SelectionBridgeStatusDto, SelectionCaptureIndicatorDto,
        SelectionCaptureStatus,
    },
    state::JeffState,
    voice_naturalness::word_count,
};

pub const SELECTION_CAPTURE_HOTKEY: &str = "CmdOrCtrl+Shift+V";
pub const SELECTION_BRIDGE_PORT: u16 = 47832;
pub const MAX_SELECTION_CHARS: usize = 12_000;
pub const EVENT_SELECTION_CAPTURED: &str = "selection://captured";
pub const EVENT_SELECTION_FAILED: &str = "selection://capture-failed";
pub const EVENT_SELECTION_CLEARED: &str = "selection://cleared";
// phase 23: live app action events
pub const EVENT_LIVE_ACTION_APPROVED: &str = "live_action://approved";
pub const EVENT_LIVE_ACTION_FALLBACK: &str = "live_action://fallback_triggered";
pub const EVENT_LIVE_ACTION_RESULT: &str = "live_action://result";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedSelection {
    pub text: String,
    pub app_name: String,
    pub document_title: Option<String>,
    pub source_type: String,
    pub source_url: Option<String>,
    pub captured_at: i64,
    pub truncated: bool,
}

impl CapturedSelection {
    pub fn new(
        text: &str,
        app_name: &str,
        document_title: Option<String>,
        source_type: &str,
        source_url: Option<String>,
        captured_at: i64,
    ) -> Result<Self, String> {
        let normalized = normalize_selected_text(text);
        if normalized.is_empty() {
            return Err("no selected text was provided".to_string());
        }

        let (text, truncated) = truncate_selection(&normalized);
        Ok(Self {
            text,
            app_name: app_name.trim().to_string(),
            document_title: document_title
                .map(|title| title.trim().to_string())
                .filter(|title| !title.is_empty()),
            source_type: source_type.trim().to_string(),
            source_url,
            captured_at,
            truncated,
        })
    }

    fn indicator(&self) -> SelectionCaptureIndicatorDto {
        let count = word_count(&self.text);
        let mut message = format!("Captured {count} words from {}", self.app_name);
        if self.truncated {
            message.push_str(" (trimmed to the safe capture limit)");
        }
        SelectionCaptureIndicatorDto {
            status: SelectionCaptureStatus::Captured,
            app_name: self.app_name.clone(),
            document_title: self.document_title.clone(),
            captured_at: self.captured_at,
            word_count: count,
            source_type: self.source_type.clone(),
            message,
        }
    }

    fn prompt_context(&self) -> String {
        let document = self
            .document_title
            .as_deref()
            .filter(|title| !title.is_empty())
            .unwrap_or("unknown document");
        let url = self
            .source_url
            .as_deref()
            .map(|value| format!("\nSource URL: {value}"))
            .unwrap_or_default();
        format!(
            "User selected text from {} ({document}) via {} at {}. Use this selected text as explicit user-provided context for the next answer only.{url}\n\nSelected text:\n{}",
            self.app_name, self.source_type, self.captured_at, self.text
        )
    }
}

struct SelectionCaptureStateInner {
    current: Option<CapturedSelection>,
    indicator: Option<SelectionCaptureIndicatorDto>,
    bridge_token: String,
}

pub struct SelectionCaptureState {
    inner: Mutex<SelectionCaptureStateInner>,
}

impl SelectionCaptureState {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(SelectionCaptureStateInner {
                current: None,
                indicator: None,
                bridge_token: generate_bridge_token(),
            }),
        }
    }

    pub fn bridge_status(&self) -> SelectionBridgeStatusDto {
        let guard = self.inner.lock().expect("selection state lock poisoned");
        SelectionBridgeStatusDto {
            enabled: true,
            port: SELECTION_BRIDGE_PORT,
            token: guard.bridge_token.clone(),
        }
    }

    pub fn current_indicator(&self) -> Option<SelectionCaptureIndicatorDto> {
        self.inner
            .lock()
            .ok()
            .and_then(|guard| guard.indicator.clone())
    }

    pub fn set_capture(&self, selection: CapturedSelection) -> SelectionCaptureIndicatorDto {
        let indicator = selection.indicator();
        if let Ok(mut guard) = self.inner.lock() {
            guard.current = Some(selection);
            guard.indicator = Some(indicator.clone());
        }
        indicator
    }

    pub fn set_failed(
        &self,
        app_name: String,
        document_title: Option<String>,
        source_type: &str,
        message: String,
    ) -> SelectionCaptureIndicatorDto {
        let indicator = SelectionCaptureIndicatorDto {
            status: SelectionCaptureStatus::Failed,
            app_name,
            document_title,
            captured_at: unix_now(),
            word_count: 0,
            source_type: source_type.to_string(),
            message,
        };
        if let Ok(mut guard) = self.inner.lock() {
            guard.current = None;
            guard.indicator = Some(indicator.clone());
        }
        indicator
    }

    pub fn dismiss(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.current = None;
            guard.indicator = None;
        }
    }

    pub fn take_prompt_context(&self) -> Option<String> {
        self.inner.lock().ok().and_then(|mut guard| {
            let selected = guard.current.take();
            guard.indicator = None;
            selected.map(|selection| selection.prompt_context())
        })
    }

    fn token_matches(&self, token: &str) -> bool {
        self.inner
            .lock()
            .map(|guard| guard.bridge_token == token.trim())
            .unwrap_or(false)
    }
}

pub fn capture_selection_from_hotkey<R: Runtime>(app: &AppHandle<R>) {
    let Some(selection_state) = app.try_state::<SelectionCaptureState>() else {
        return;
    };

    let privacy_enabled = app
        .try_state::<JeffState>()
        .and_then(|state| state.store.get_privacy_selection_capture_enabled().ok())
        .unwrap_or(true);

    if !privacy_enabled {
        let indicator = selection_state.set_failed(
            "Selection capture".to_string(),
            None,
            "native_accessibility",
            "Selection capture is off in Privacy Center.".to_string(),
        );
        let _ = app.emit(EVENT_SELECTION_FAILED, &indicator);
        let _ = crate::ambient::show_overlay(app);
        return;
    }

    if !crate::context_observer::is_accessibility_trusted() {
        let indicator = selection_state.set_failed(
            "Accessibility".to_string(),
            None,
            "native_accessibility",
            "Selection capture needs Accessibility permission before Jeff can read selected text."
                .to_string(),
        );
        let _ = app.emit(EVENT_SELECTION_FAILED, &indicator);
        let _ = crate::ambient::show_overlay(app);
        return;
    }

    let context_hint = app
        .try_state::<crate::state::ContextState>()
        .and_then(|state| state.current());

    match capture_native_selection(context_hint) {
        Ok(selection) => {
            let indicator = selection_state.set_capture(selection);
            let _ = app.emit(EVENT_SELECTION_CAPTURED, &indicator);
            let _ = crate::ambient::show_overlay(app);
        }
        Err(error) => {
            let app_name = error
                .app_name()
                .unwrap_or("the active app")
                .trim()
                .to_string();
            let message = error.user_message();
            let indicator =
                selection_state.set_failed(app_name, None, "native_accessibility", message);
            let _ = app.emit(EVENT_SELECTION_FAILED, &indicator);
            let _ = crate::ambient::show_overlay(app);
        }
    }
}

pub fn capture_browser_selection_request<R: Runtime>(
    app: &AppHandle<R>,
    request: BrowserSelectionCaptureRequestDto,
) -> Result<SelectionCaptureIndicatorDto, String> {
    let selection_state = app
        .try_state::<SelectionCaptureState>()
        .ok_or_else(|| "selection capture state is not available".to_string())?;

    if !selection_state.token_matches(&request.token) {
        return Err("invalid browser selection bridge token".to_string());
    }

    let privacy_enabled = app
        .try_state::<JeffState>()
        .and_then(|state| state.store.get_privacy_selection_capture_enabled().ok())
        .unwrap_or(true);
    if !privacy_enabled {
        let indicator = selection_state.set_failed(
            request.app_name.trim().to_string(),
            request.document_title.clone(),
            "browser_extension",
            "Selection capture is off in Privacy Center.".to_string(),
        );
        let _ = app.emit(EVENT_SELECTION_FAILED, &indicator);
        return Err(indicator.message);
    }

    let selection = CapturedSelection::new(
        &request.text,
        if request.app_name.trim().is_empty() {
            "Browser"
        } else {
            &request.app_name
        },
        request.document_title,
        "browser_extension",
        request.source_url,
        request.captured_at.unwrap_or_else(unix_now),
    )?;
    let indicator = selection_state.set_capture(selection);
    let _ = app.emit(EVENT_SELECTION_CAPTURED, &indicator);
    let _ = crate::ambient::show_overlay(app);
    Ok(indicator)
}

pub fn start_browser_bridge<R: Runtime + 'static>(app: AppHandle<R>) {
    let _ = std::thread::Builder::new()
        .name("jeff-selection-bridge".to_string())
        .spawn(move || {
            let listener = match TcpListener::bind(("127.0.0.1", SELECTION_BRIDGE_PORT)) {
                Ok(listener) => listener,
                Err(err) => {
                    let _ = app.emit(
                        "selection://bridge-error",
                        serde_json::json!({ "error": err.to_string(), "port": SELECTION_BRIDGE_PORT }),
                    );
                    return;
                }
            };

            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => handle_bridge_stream(&app, stream),
                    Err(err) => {
                        let _ = app.emit(
                            "selection://bridge-error",
                            serde_json::json!({ "error": err.to_string(), "port": SELECTION_BRIDGE_PORT }),
                        );
                    }
                }
            }
        });
}

fn handle_bridge_stream<R: Runtime>(app: &AppHandle<R>, mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let request = match read_http_request(&mut stream) {
        Ok(request) => request,
        Err(err) => {
            let _ = write_http_response(
                &mut stream,
                400,
                &serde_json::json!({ "ok": false, "error": err }),
            );
            return;
        }
    };

    if request.method == "OPTIONS" {
        let _ = write_http_response(&mut stream, 204, &serde_json::json!({}));
        return;
    }

    match (request.method.as_str(), request.path.as_str()) {
        ("POST", "/selection-capture") => {
            let parsed: BrowserSelectionCaptureRequestDto =
                match serde_json::from_slice(&request.body) {
                    Ok(value) => value,
                    Err(err) => {
                        let _ = write_http_response(
                            &mut stream,
                            400,
                            &serde_json::json!({ "ok": false, "error": err.to_string() }),
                        );
                        return;
                    }
                };
            match capture_browser_selection_request(app, parsed) {
                Ok(indicator) => {
                    let _ = write_http_response(
                        &mut stream,
                        200,
                        &serde_json::json!({ "ok": true, "indicator": indicator }),
                    );
                }
                Err(err) => {
                    let _ = write_http_response(
                        &mut stream,
                        403,
                        &serde_json::json!({ "ok": false, "error": err }),
                    );
                }
            }
        }
        // phase 23: apply-edit — receives a proposed live edit from the extension,
        // stores a pending receipt, and emits an approval-request event to the frontend.
        ("POST", "/apply-edit") => {
            handle_apply_edit_request(app, &mut stream, &request.body);
        }
        // phase 23: apply-fallback — called by the extension when anchor validation fails.
        ("POST", "/apply-fallback") => {
            handle_apply_fallback_request(app, &mut stream, &request.body);
        }
        ("POST", "/apply-result") => {
            handle_apply_result_request(app, &mut stream, &request.body);
        }
        // phase 23: long-poll endpoint — extension polls for approval of a receipt.
        ("GET", path) if path.starts_with("/pending-approval/") => {
            let path_parts = path.trim_start_matches("/pending-approval/");
            handle_pending_approval_poll(app, &mut stream, path_parts);
        }
        _ => {
            let _ = write_http_response(
                &mut stream,
                404,
                &serde_json::json!({ "ok": false, "error": "unsupported selection bridge route" }),
            );
        }
    }
}

#[derive(serde::Deserialize)]
struct ApplyEditRequest {
    token: String,
    editor_surface: String,
    selection_anchor_hash: String,
    before_text: String,
    after_text: String,
    document_title: String,
}

#[derive(serde::Deserialize)]
struct ApplyFallbackRequest {
    token: String,
    receipt_id: i64,
}

#[derive(serde::Deserialize)]
struct ApplyResultRequest {
    token: String,
    receipt_id: i64,
    status: String,
    error: Option<String>,
}

fn handle_apply_edit_request<R: Runtime>(app: &AppHandle<R>, stream: &mut TcpStream, body: &[u8]) {
    let parsed: ApplyEditRequest = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(err) => {
            let _ = write_http_response(
                stream,
                400,
                &serde_json::json!({ "ok": false, "error": err.to_string() }),
            );
            return;
        }
    };

    // validate token
    let state = match app.try_state::<SelectionCaptureState>() {
        Some(s) => s,
        None => {
            let _ = write_http_response(stream, 500, &serde_json::json!({ "ok": false }));
            return;
        }
    };
    if !state.token_matches(&parsed.token) {
        let _ = write_http_response(
            stream,
            403,
            &serde_json::json!({ "ok": false, "error": "invalid token" }),
        );
        return;
    }

    let before_hash = sha256_hex(&parsed.before_text);
    if parsed.selection_anchor_hash.trim().to_ascii_lowercase() != before_hash {
        let _ = write_http_response(
            stream,
            400,
            &serde_json::json!({ "ok": false, "error": "selection anchor hash does not match before_text" }),
        );
        return;
    }
    let after_hash = sha256_hex(&parsed.after_text);

    // create receipt
    let jeff_state = match app.try_state::<JeffState>() {
        Some(s) => s,
        None => {
            let _ = write_http_response(stream, 500, &serde_json::json!({ "ok": false }));
            return;
        }
    };
    let task_id = jeff_state
        .store
        .get_active_task()
        .ok()
        .flatten()
        .map(|task| task.id);
    let receipt_id = match jeff_state.store.create_live_edit_receipt(
        task_id,
        &parsed.editor_surface,
        &parsed.document_title,
        &before_hash,
        &after_hash,
        &parsed.before_text,
        &parsed.after_text,
    ) {
        Ok(id) => id,
        Err(err) => {
            let _ = write_http_response(
                stream,
                500,
                &serde_json::json!({ "ok": false, "error": err.to_string() }),
            );
            return;
        }
    };

    // emit approval request to frontend
    let _ = app.emit(
        "live_action://apply_requested",
        serde_json::json!({ "receipt_id": receipt_id }),
    );

    let _ = write_http_response(
        stream,
        200,
        &serde_json::json!({ "status": "pending_approval", "receipt_id": receipt_id }),
    );
}

fn valid_live_action_token<R: Runtime>(app: &AppHandle<R>, token: &str) -> bool {
    app.try_state::<SelectionCaptureState>()
        .map(|state| state.token_matches(token))
        .unwrap_or(false)
}

fn handle_apply_fallback_request<R: Runtime>(
    app: &AppHandle<R>,
    stream: &mut TcpStream,
    body: &[u8],
) {
    let parsed: ApplyFallbackRequest = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(err) => {
            let _ = write_http_response(
                stream,
                400,
                &serde_json::json!({ "ok": false, "error": err.to_string() }),
            );
            return;
        }
    };

    if !valid_live_action_token(app, &parsed.token) {
        let _ = write_http_response(
            stream,
            403,
            &serde_json::json!({ "ok": false, "error": "invalid token" }),
        );
        return;
    }

    let jeff_state = match app.try_state::<JeffState>() {
        Some(s) => s,
        None => {
            let _ = write_http_response(stream, 500, &serde_json::json!({ "ok": false }));
            return;
        }
    };

    let _ = jeff_state
        .store
        .update_live_edit_status(parsed.receipt_id, "fallback");
    let _ = app.emit(
        EVENT_LIVE_ACTION_FALLBACK,
        serde_json::json!({ "receipt_id": parsed.receipt_id }),
    );

    let _ = write_http_response(stream, 200, &serde_json::json!({ "ok": true }));
}

fn handle_apply_result_request<R: Runtime>(
    app: &AppHandle<R>,
    stream: &mut TcpStream,
    body: &[u8],
) {
    let parsed: ApplyResultRequest = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(err) => {
            let _ = write_http_response(
                stream,
                400,
                &serde_json::json!({ "ok": false, "error": err.to_string() }),
            );
            return;
        }
    };

    if !valid_live_action_token(app, &parsed.token) {
        let _ = write_http_response(
            stream,
            403,
            &serde_json::json!({ "ok": false, "error": "invalid token" }),
        );
        return;
    }

    let status = match parsed.status.as_str() {
        "applied" => "applied",
        "failed" => "failed",
        _ => {
            let _ = write_http_response(
                stream,
                400,
                &serde_json::json!({ "ok": false, "error": "status must be applied or failed" }),
            );
            return;
        }
    };

    let jeff_state = match app.try_state::<JeffState>() {
        Some(s) => s,
        None => {
            let _ = write_http_response(stream, 500, &serde_json::json!({ "ok": false }));
            return;
        }
    };

    let update = jeff_state
        .store
        .update_live_edit_status(parsed.receipt_id, status);
    match update {
        Ok(_) => {
            let _ = app.emit(
                EVENT_LIVE_ACTION_RESULT,
                serde_json::json!({
                    "receipt_id": parsed.receipt_id,
                    "status": status,
                    "error": parsed.error,
                }),
            );
            let _ = write_http_response(stream, 200, &serde_json::json!({ "ok": true }));
        }
        Err(err) => {
            let _ = write_http_response(
                stream,
                404,
                &serde_json::json!({ "ok": false, "error": err.to_string() }),
            );
        }
    }
}

fn handle_pending_approval_poll<R: Runtime>(
    app: &AppHandle<R>,
    stream: &mut TcpStream,
    path_parts: &str,
) {
    let mut parts = path_parts.splitn(2, '/');
    let token = parts.next().unwrap_or_default();
    let receipt_id_str = parts.next().unwrap_or_default();
    if !valid_live_action_token(app, token) {
        let _ = write_http_response(
            stream,
            403,
            &serde_json::json!({ "ok": false, "error": "invalid token" }),
        );
        return;
    }

    let receipt_id: i64 = match receipt_id_str.trim().parse() {
        Ok(id) => id,
        Err(_) => {
            let _ = write_http_response(
                stream,
                400,
                &serde_json::json!({ "ok": false, "error": "invalid receipt_id" }),
            );
            return;
        }
    };

    let jeff_state = match app.try_state::<JeffState>() {
        Some(s) => s,
        None => {
            let _ = write_http_response(stream, 500, &serde_json::json!({ "ok": false }));
            return;
        }
    };

    // poll up to 20 seconds (40 × 500ms)
    for _ in 0..40 {
        // check current status
        if let Ok(receipts) = jeff_state.store.list_live_edit_receipts(None) {
            if let Some(receipt) = receipts.iter().find(|r| r.id == receipt_id) {
                if receipt.status == "approved" {
                    let _ = write_http_response(
                        stream,
                        200,
                        &serde_json::json!({ "status": "approved" }),
                    );
                    return;
                }
                if matches!(
                    receipt.status.as_str(),
                    "rejected" | "fallback" | "applied" | "failed"
                ) {
                    let _ = write_http_response(
                        stream,
                        200,
                        &serde_json::json!({ "status": receipt.status }),
                    );
                    return;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    // timed out — still pending
    let _ = write_http_response(
        stream,
        200,
        &serde_json::json!({ "status": "pending_approval" }),
    );
}

struct HttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn read_http_request(stream: &mut TcpStream) -> Result<HttpRequest, String> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 1024];
    let header_end;
    loop {
        let read = stream.read(&mut buffer).map_err(|err| err.to_string())?;
        if read == 0 {
            return Err("empty bridge request".to_string());
        }
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.len() > 70_000 {
            return Err("bridge request is too large".to_string());
        }
        if let Some(pos) = find_header_end(&bytes) {
            header_end = pos;
            break;
        }
    }

    let header_text = String::from_utf8_lossy(&bytes[..header_end]);
    let mut lines = header_text.lines();
    let request_line = lines
        .next()
        .ok_or_else(|| "missing request line".to_string())?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut content_length = 0usize;
    for line in lines {
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            content_length = value
                .trim()
                .parse::<usize>()
                .map_err(|_| "invalid content-length header".to_string())?;
        }
    }
    if content_length > 65_536 {
        return Err("selection bridge payload exceeds limit".to_string());
    }

    let body_start = header_end + 4;
    let mut body = bytes[body_start..].to_vec();
    while body.len() < content_length {
        let read = stream.read(&mut buffer).map_err(|err| err.to_string())?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read]);
    }
    body.truncate(content_length);

    Ok(HttpRequest { method, path, body })
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    body: &serde_json::Value,
) -> std::io::Result<()> {
    let status_text = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "OK",
    };
    let body_text = if status == 204 {
        String::new()
    } else {
        body.to_string()
    };
    // no CORS headers: chrome mv3 service workers with matching host_permissions
    // bypass cors and do not require Access-Control-Allow-Origin. omitting these
    // headers prevents regular web pages from reading bridge responses even if
    // they somehow craft a request to the local port.
    write!(
        stream,
        "HTTP/1.1 {status} {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body_text.len(),
        body_text
    )
}

fn normalize_selected_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn truncate_selection(text: &str) -> (String, bool) {
    if text.chars().count() <= MAX_SELECTION_CHARS {
        return (text.to_string(), false);
    }
    let truncated = text.chars().take(MAX_SELECTION_CHARS).collect::<String>();
    (truncated, true)
}

fn generate_bridge_token() -> String {
    // read 16 bytes from the OS entropy source (/dev/urandom on unix).
    // this provides cryptographically strong randomness without adding a
    // new crate dependency. fallback mixes pid + multiple timestamps with
    // a stack-address component if the entropy source is unavailable.
    use std::io::Read as _;
    let mut bytes = [0u8; 16];
    let filled = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .is_ok();
    if !filled {
        let now_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id() as u128;
        let stack_addr = &bytes as *const _ as u128;
        let mut v: u128 = now_ns ^ (pid << 32) ^ stack_addr;
        for b in &mut bytes {
            // xorshift-based stretch so bytes are not trivially guessable
            v ^= v << 13;
            v ^= v >> 7;
            v ^= v << 17;
            *b = (v >> 56) as u8;
        }
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionCaptureError {
    NoPermission,
    NoFrontmostApp,
    NoFocusedElement { app_name: String },
    NoSelectedText { app_name: String },
    UnsupportedPlatform,
}

impl SelectionCaptureError {
    fn app_name(&self) -> Option<&str> {
        match self {
            Self::NoFocusedElement { app_name } | Self::NoSelectedText { app_name } => {
                Some(app_name)
            }
            _ => None,
        }
    }

    fn user_message(&self) -> String {
        match self {
            Self::NoPermission => {
                "Selection capture needs Accessibility permission before Jeff can read selected text."
                    .to_string()
            }
            Self::NoFrontmostApp => {
                "Could not capture text from the active app. Paste it manually.".to_string()
            }
            Self::NoFocusedElement { app_name } | Self::NoSelectedText { app_name } => {
                format!("Could not capture text from {app_name}. Paste it manually.")
            }
            Self::UnsupportedPlatform => {
                "Native selection capture is only available on macOS right now.".to_string()
            }
        }
    }
}

pub fn capture_native_selection(
    context_hint: Option<ActiveWindowContext>,
) -> Result<CapturedSelection, SelectionCaptureError> {
    platform::capture_native_selection(context_hint)
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{
        unix_now, ActiveWindowContext, CapturedSelection, SelectionCaptureError,
        MAX_SELECTION_CHARS,
    };
    use std::ffi::{c_char, c_void, CStr, CString};

    type AXError = i32;
    const AX_SUCCESS: AXError = 0;
    const CF_STRING_ENCODING_UTF8: u32 = 0x08000100;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
        fn AXUIElementCreateSystemWide() -> *const c_void;
        fn AXUIElementCopyAttributeValue(
            element: *const c_void,
            attribute: *const c_void,
            value: *mut *const c_void,
        ) -> AXError;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *const c_void);
        fn CFStringGetCStringPtr(the_string: *const c_void, encoding: u32) -> *const c_char;
        fn CFStringGetCString(
            the_string: *const c_void,
            buffer: *mut c_char,
            buffer_size: i64,
            encoding: u32,
        ) -> bool;
        fn CFStringGetLength(the_string: *const c_void) -> i64;
        fn CFStringGetMaximumSizeForEncoding(length: i64, encoding: u32) -> i64;
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            c_str: *const c_char,
            encoding: u32,
        ) -> *const c_void;
    }

    unsafe fn make_cf_string(s: &str) -> *const c_void {
        let Ok(c) = CString::new(s) else {
            return std::ptr::null();
        };
        CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), CF_STRING_ENCODING_UTF8)
    }

    unsafe fn cf_string_to_rust(cf: *const c_void) -> Option<String> {
        if cf.is_null() {
            return None;
        }
        let ptr = CFStringGetCStringPtr(cf, CF_STRING_ENCODING_UTF8);
        if !ptr.is_null() {
            return Some(CStr::from_ptr(ptr).to_string_lossy().into_owned());
        }

        let len = CFStringGetLength(cf);
        let max = CFStringGetMaximumSizeForEncoding(len, CF_STRING_ENCODING_UTF8)
            .saturating_add(1)
            .max((MAX_SELECTION_CHARS * 4) as i64);
        let mut buf = vec![0i8; max as usize];
        if CFStringGetCString(
            cf,
            buf.as_mut_ptr(),
            buf.len() as i64,
            CF_STRING_ENCODING_UTF8,
        ) {
            Some(CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
        } else {
            None
        }
    }

    fn get_frontmost_app() -> Option<(String, i32)> {
        use objc2::{class, msg_send, runtime::AnyObject};
        unsafe {
            let pool_cls = class!(NSAutoreleasePool);
            let pool: *mut AnyObject = msg_send![pool_cls, new];

            let workspace_cls = class!(NSWorkspace);
            let workspace: *mut AnyObject = msg_send![workspace_cls, sharedWorkspace];

            let result = if workspace.is_null() {
                None
            } else {
                let app: *mut AnyObject = msg_send![workspace, frontmostApplication];
                if app.is_null() {
                    None
                } else {
                    let name_ns: *mut AnyObject = msg_send![app, localizedName];
                    if name_ns.is_null() {
                        None
                    } else {
                        let cstr: *const c_char = msg_send![name_ns, UTF8String];
                        if cstr.is_null() {
                            None
                        } else {
                            let name = CStr::from_ptr(cstr).to_string_lossy().into_owned();
                            let pid: i32 = msg_send![app, processIdentifier];
                            Some((name, pid))
                        }
                    }
                }
            };

            let _: () = msg_send![pool, drain];
            result
        }
    }

    pub fn capture_native_selection(
        context_hint: Option<ActiveWindowContext>,
    ) -> Result<CapturedSelection, SelectionCaptureError> {
        unsafe {
            if !AXIsProcessTrustedWithOptions(std::ptr::null()) {
                return Err(SelectionCaptureError::NoPermission);
            }
        }

        let (app_name, _pid) = get_frontmost_app().ok_or(SelectionCaptureError::NoFrontmostApp)?;
        if matches!(app_name.as_str(), "Jeff" | "jeff-desktop" | "jeff") {
            return Err(SelectionCaptureError::NoFocusedElement { app_name });
        }

        let selected_text = unsafe {
            let system = AXUIElementCreateSystemWide();
            if system.is_null() {
                return Err(SelectionCaptureError::NoFocusedElement {
                    app_name: app_name.clone(),
                });
            }

            let focused_attr = make_cf_string("AXFocusedUIElement");
            let mut focused_ref: *const c_void = std::ptr::null();
            let focused_err = if !focused_attr.is_null() {
                AXUIElementCopyAttributeValue(system, focused_attr, &mut focused_ref)
            } else {
                -1
            };
            CFRelease(system);
            if !focused_attr.is_null() {
                CFRelease(focused_attr);
            }

            if focused_err != AX_SUCCESS || focused_ref.is_null() {
                return Err(SelectionCaptureError::NoFocusedElement {
                    app_name: app_name.clone(),
                });
            }

            let selected_attr = make_cf_string("AXSelectedText");
            let mut selected_ref: *const c_void = std::ptr::null();
            let selected_err = if !selected_attr.is_null() {
                AXUIElementCopyAttributeValue(focused_ref, selected_attr, &mut selected_ref)
            } else {
                -1
            };
            CFRelease(focused_ref);
            if !selected_attr.is_null() {
                CFRelease(selected_attr);
            }

            if selected_err != AX_SUCCESS || selected_ref.is_null() {
                return Err(SelectionCaptureError::NoSelectedText {
                    app_name: app_name.clone(),
                });
            }
            let text = cf_string_to_rust(selected_ref).unwrap_or_default();
            CFRelease(selected_ref);
            text
        };

        let document_title = context_hint
            .filter(|ctx| ctx.app_name == app_name || ctx.app_name.is_empty())
            .map(|ctx| ctx.document_title)
            .filter(|title| !title.trim().is_empty());

        CapturedSelection::new(
            &selected_text,
            &app_name,
            document_title,
            "native_accessibility",
            None,
            unix_now(),
        )
        .map_err(|_| SelectionCaptureError::NoSelectedText { app_name })
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::{ActiveWindowContext, CapturedSelection, SelectionCaptureError};

    pub fn capture_native_selection(
        _context_hint: Option<ActiveWindowContext>,
    ) -> Result<CapturedSelection, SelectionCaptureError> {
        Err(SelectionCaptureError::UnsupportedPlatform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_selection_normalizes_limits_and_indicates_without_text_leak() {
        let selection = CapturedSelection::new(
            " hello  \nworld ",
            "TextEdit",
            Some("Draft".to_string()),
            "native_accessibility",
            None,
            1,
        )
        .unwrap();
        assert_eq!(selection.text, "hello\nworld");
        let indicator = selection.indicator();
        assert_eq!(indicator.word_count, 2);
        assert_eq!(indicator.app_name, "TextEdit");
        assert!(!indicator.message.contains("hello"));
    }

    #[test]
    fn selection_state_consumes_context_once() {
        let state = SelectionCaptureState::new();
        let selection = CapturedSelection::new(
            "selected material",
            "Pages",
            Some("Essay".to_string()),
            "native_accessibility",
            None,
            2,
        )
        .unwrap();
        state.set_capture(selection);
        assert!(state.current_indicator().is_some());
        let prompt = state.take_prompt_context().unwrap();
        assert!(prompt.contains("selected material"));
        assert!(state.take_prompt_context().is_none());
        assert!(state.current_indicator().is_none());
    }

    #[test]
    fn dismiss_clears_selection_without_prompt_context() {
        let state = SelectionCaptureState::new();
        let selection = CapturedSelection::new(
            "selected material",
            "Pages",
            None,
            "native_accessibility",
            None,
            2,
        )
        .unwrap();
        state.set_capture(selection);
        state.dismiss();
        assert!(state.current_indicator().is_none());
        assert!(state.take_prompt_context().is_none());
    }

    #[test]
    fn bridge_token_is_available_unique_and_high_entropy() {
        let first = SelectionCaptureState::new().bridge_status().token;
        let second = SelectionCaptureState::new().bridge_status().token;
        // token must be non-empty and unique across two fresh states.
        assert!(!first.is_empty());
        assert_ne!(first, second);
        // token should be a 32-char lowercase hex string (16 bytes).
        assert_eq!(first.len(), 32, "token should be 32 hex chars (16 bytes)");
        assert!(
            first.chars().all(|c| c.is_ascii_hexdigit()),
            "token should be lowercase hex"
        );
    }
}
