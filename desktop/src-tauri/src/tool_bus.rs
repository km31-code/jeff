// apex e1: tool bus (MCP client). One governed integration layer for external
// capability. Connections are opt-in, scoped, and listed in the Privacy Center
// with a per-tool call log; disconnecting stops calls immediately and purges
// the connection's discovered tools.
//
// Data boundary (enforced by type + validated): tool invocations accept only
// explicit ToolArguments. Ambient context (snapshot, memory, relational, or
// profile state) can never be serialized into a tool call -- connections
// receive the specific query/action payload, never a context dump.
//
// stdio and streamable-http MCP transports initialize, discover, and invoke
// real servers. http oauth tokens are resolved at call time and never stored in
// sqlite or exposed in call logs.

#![cfg_attr(test, allow(dead_code))]

use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::{
    models::{ToolCallLogDto, ToolConnectionDto, ToolDescriptorDto},
    store::TaskStore,
};

pub const TRANSPORT_STDIO: &str = "stdio";
pub const TRANSPORT_HTTP: &str = "http";
pub const TRANSPORT_LOOPBACK: &str = "loopback";

pub const CALL_STATUS_OK: &str = "ok";
pub const CALL_STATUS_REJECTED: &str = "rejected";
pub const CALL_STATUS_ERROR: &str = "error";

// keys that would smuggle ambient context into a tool call. The boundary is a
// denylist over argument keys plus a hard cap on payload size.
pub const AMBIENT_CONTEXT_KEYS: &[&str] = &[
    "snapshot",
    "situational_snapshot",
    "situationalsnapshot",
    "episodes",
    "episode",
    "memory",
    "memory_recall",
    "recall",
    "relational",
    "relational_context",
    "profile",
    "user_profile",
    "user_model",
    "work_understanding",
];

pub const MAX_TOOL_ARGUMENTS_BYTES: usize = 8 * 1024;
const MCP_PROTOCOL_VERSION: &str = "2025-03-26";
const MCP_REQUEST_TIMEOUT_SECS: u64 = 30;

// the only accepted shape for tool-call arguments. Constructed from an explicit
// JSON object and validated against the ambient-context boundary. There is no
// From<Snapshot>/From<Memory> impl anywhere -- ambient structs cannot become
// ToolArguments.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolArguments(serde_json::Map<String, serde_json::Value>);

impl ToolArguments {
    pub fn from_value(value: serde_json::Value) -> Result<Self> {
        let obj = value
            .as_object()
            .ok_or_else(|| anyhow!("tool arguments must be a JSON object"))?;
        let serialized = serde_json::to_vec(obj)?;
        if serialized.len() > MAX_TOOL_ARGUMENTS_BYTES {
            return Err(anyhow!(
                "tool arguments exceed {} bytes; ambient context dumps are not allowed",
                MAX_TOOL_ARGUMENTS_BYTES
            ));
        }
        // the boundary must hold at every depth: a nested {"q":{"memory":{...}}}
        // would otherwise smuggle a context dump past a top-level-only check.
        reject_ambient_keys(&serde_json::Value::Object(obj.clone()))?;
        Ok(Self(obj.clone()))
    }

    pub fn value(&self) -> serde_json::Value {
        serde_json::Value::Object(self.0.clone())
    }

    pub fn summary(&self) -> String {
        let keys = self.0.keys().cloned().collect::<Vec<_>>().join(", ");
        let summary = format!("{{{keys}}}");
        summary.chars().take(160).collect()
    }
}

// recursively reject any object key (at any nesting depth) that matches the
// ambient-context denylist. Arrays are walked element-wise. This closes the
// nested-payload bypass where a context dump hides under an innocuous outer key.
fn reject_ambient_keys(value: &serde_json::Value) -> Result<()> {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                let lower = key.to_ascii_lowercase();
                if AMBIENT_CONTEXT_KEYS
                    .iter()
                    .any(|marker| lower == *marker || lower.contains(marker))
                {
                    return Err(anyhow!(
                        "tool arguments may not carry ambient context (key '{key}')"
                    ));
                }
                reject_ambient_keys(child)?;
            }
            Ok(())
        }
        serde_json::Value::Array(items) => {
            for item in items {
                reject_ambient_keys(item)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolInvocationResult {
    pub status: String,
    pub connection: String,
    pub tool: String,
    pub output: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectedActionResult {
    pub receipt_id: i64,
    pub status: String,
    pub tool_result: Option<ToolInvocationResult>,
}

// ---- connection manager --------------------------------------------------

pub fn add_tool_connection(
    store: &TaskStore,
    name: &str,
    transport: &str,
    endpoint: &str,
    scopes: &[String],
) -> Result<ToolConnectionDto> {
    let name = name.trim();
    if name.is_empty() {
        return Err(anyhow!("connection name cannot be empty"));
    }
    if !matches!(
        transport,
        TRANSPORT_STDIO | TRANSPORT_HTTP | TRANSPORT_LOOPBACK
    ) {
        return Err(anyhow!("unsupported transport '{transport}'"));
    }
    let conn = store.connect()?;
    conn.execute(
        "INSERT INTO tool_connections (name, transport, endpoint, scopes_json, enabled)
         VALUES (?1, ?2, ?3, ?4, 1)",
        params![
            name,
            transport,
            endpoint.trim(),
            serde_json::to_string(scopes)?
        ],
    )
    .context("failed to add tool connection")?;
    let id = conn.last_insert_rowid();
    drop(conn);
    get_connection(store, id)?.ok_or_else(|| anyhow!("connection missing after insert"))
}

// disconnect: stop calls immediately and purge connection-scoped cache (the
// discovered tool list). Call-log history is retained (connection_id -> NULL).
pub fn remove_tool_connection(store: &TaskStore, connection_id: i64) -> Result<()> {
    let conn = store.connect()?;
    // CASCADE removes tool_connection_tools; log rows keep history via SET NULL.
    conn.execute(
        "DELETE FROM tool_connections WHERE id = ?1",
        params![connection_id],
    )
    .context("failed to remove tool connection")?;
    Ok(())
}

pub fn set_tool_connection_enabled(
    store: &TaskStore,
    connection_id: i64,
    enabled: bool,
) -> Result<ToolConnectionDto> {
    let conn = store.connect()?;
    let changed = conn
        .execute(
            "UPDATE tool_connections
             SET enabled = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?2",
            params![if enabled { 1 } else { 0 }, connection_id],
        )
        .context("failed to update connection enabled state")?;
    if changed == 0 {
        return Err(anyhow!("connection id={connection_id} not found"));
    }
    if !enabled {
        // purge discovered tools when disabled; re-discovery happens on re-enable.
        conn.execute(
            "DELETE FROM tool_connection_tools WHERE connection_id = ?1",
            params![connection_id],
        )?;
    }
    drop(conn);
    get_connection(store, connection_id)?.ok_or_else(|| anyhow!("connection missing"))
}

pub fn list_tool_connections(store: &TaskStore) -> Result<Vec<ToolConnectionDto>> {
    let conn = store.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, name, transport, endpoint, scopes_json, enabled, created_at
         FROM tool_connections ORDER BY id DESC",
    )?;
    let rows = stmt
        .query_map([], connection_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// tool discovery. Live MCP discovery is env-gated; this registers the tool set a
// transport reports (loopback provides it deterministically in tests).
pub fn register_connection_tools(
    store: &TaskStore,
    connection_id: i64,
    tools: &[(String, String)],
) -> Result<()> {
    let conn = store.connect()?;
    for (name, description) in tools {
        conn.execute(
            "INSERT INTO tool_connection_tools (connection_id, tool_name, description)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(connection_id, tool_name) DO UPDATE SET description = excluded.description",
            params![connection_id, name.trim(), description.trim()],
        )
        .context("failed to register connection tool")?;
    }
    Ok(())
}

pub fn discover_connection_tools(
    store: &TaskStore,
    connection_id: i64,
) -> Result<Vec<ToolDescriptorDto>> {
    let connection = get_connection(store, connection_id)?
        .ok_or_else(|| anyhow!("connection id={connection_id} not found"))?;
    if !connection.enabled {
        return Err(anyhow!(
            "tool connection '{}' is disconnected",
            connection.name
        ));
    }
    let tools = match connection.transport.as_str() {
        TRANSPORT_LOOPBACK => Vec::new(),
        TRANSPORT_STDIO | TRANSPORT_HTTP => {
            let result = mcp_request(&connection, "tools/list", serde_json::json!({}))?;
            parse_discovered_tools(&result)?
        }
        other => return Err(anyhow!("unsupported transport '{other}'")),
    };
    let conn = store.connect()?;
    conn.execute(
        "DELETE FROM tool_connection_tools WHERE connection_id = ?1",
        params![connection_id],
    )?;
    drop(conn);
    register_connection_tools(store, connection_id, &tools)?;
    list_connection_tools(store, connection_id)
}

pub fn list_connection_tools(
    store: &TaskStore,
    connection_id: i64,
) -> Result<Vec<ToolDescriptorDto>> {
    let conn = store.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, connection_id, tool_name, description
         FROM tool_connection_tools WHERE connection_id = ?1 ORDER BY tool_name ASC",
    )?;
    let rows = stmt
        .query_map(params![connection_id], |row| {
            Ok(ToolDescriptorDto {
                id: row.get(0)?,
                connection_id: row.get(1)?,
                tool_name: row.get(2)?,
                description: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn list_tool_call_log(store: &TaskStore, limit: usize) -> Result<Vec<ToolCallLogDto>> {
    let conn = store.connect()?;
    let max = limit.min(200) as i64;
    let mut stmt = conn.prepare(
        "SELECT id, connection_name, tool_name, argument_summary, status, created_at
         FROM tool_call_log ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![max], |row| {
            Ok(ToolCallLogDto {
                id: row.get(0)?,
                connection_name: row.get(1)?,
                tool_name: row.get(2)?,
                argument_summary: row.get(3)?,
                status: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn invoke_first_enabled_tool(
    store: &TaskStore,
    tool_names: &[&str],
    arguments: serde_json::Value,
) -> Result<ToolInvocationResult> {
    let args = ToolArguments::from_value(arguments)?;
    let conn = store.connect()?;
    for tool_name in tool_names {
        let connection_name: Option<String> = conn
            .query_row(
                "SELECT c.name
                 FROM tool_connections c
                 JOIN tool_connection_tools t ON t.connection_id = c.id
                 WHERE c.enabled = 1 AND t.tool_name = ?1
                 ORDER BY c.id ASC
                 LIMIT 1",
                params![tool_name],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(connection_name) = connection_name {
            drop(conn);
            return invoke_tool(store, &connection_name, tool_name, &args);
        }
    }
    Err(anyhow!(
        "no enabled MCP connection exposes any of: {}",
        tool_names.join(", ")
    ))
}

pub fn has_enabled_tool(store: &TaskStore, tool_names: &[&str]) -> Result<bool> {
    let conn = store.connect()?;
    for tool_name in tool_names {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*)
             FROM tool_connections c
             JOIN tool_connection_tools t ON t.connection_id = c.id
             WHERE c.enabled = 1 AND t.tool_name = ?1",
            params![tool_name],
            |row| row.get(0),
        )?;
        if count > 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn tool_result_payload(output: &serde_json::Value) -> Result<serde_json::Value> {
    if let Some(payload) = output.get("structuredContent") {
        return Ok(payload.clone());
    }
    if let Some(payload) = output.get("structured_content") {
        return Ok(payload.clone());
    }
    if let Some(text) = output
        .get("content")
        .and_then(serde_json::Value::as_array)
        .and_then(|items| {
            items.iter().find_map(|item| {
                (item.get("type").and_then(serde_json::Value::as_str) == Some("text"))
                    .then(|| item.get("text").and_then(serde_json::Value::as_str))
                    .flatten()
            })
        })
    {
        return serde_json::from_str(text).context("MCP text result was not valid JSON");
    }
    Ok(output.clone())
}

pub fn persist_connected_action(
    store: &TaskStore,
    receipt_id: i64,
    task_id: i64,
    tool_names: &[&str],
    arguments: serde_json::Value,
) -> Result<()> {
    ToolArguments::from_value(arguments.clone())?;
    if tool_names.is_empty() {
        return Err(anyhow!("connected action requires at least one tool name"));
    }
    let receipt = store
        .get_action_receipt(receipt_id)?
        .ok_or_else(|| anyhow!("action receipt id={receipt_id} not found"))?;
    if receipt.task_id != task_id || receipt.status != "pending_approval" {
        return Err(anyhow!(
            "connected action receipt is not a matching pending proposal"
        ));
    }
    let conn = store.connect()?;
    conn.execute(
        "INSERT INTO connected_action_runs
         (receipt_id, task_id, tool_names_json, arguments_json)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            receipt_id,
            task_id,
            serde_json::to_string(tool_names)?,
            serde_json::to_string(&arguments)?,
        ],
    )?;
    Ok(())
}

pub fn approve_connected_action(
    store: &TaskStore,
    receipt_id: i64,
) -> Result<ConnectedActionResult> {
    let (tool_names, arguments) = claim_connected_action(store, receipt_id)?;
    match invoke_first_enabled_tool(
        store,
        &tool_names.iter().map(String::as_str).collect::<Vec<_>>(),
        arguments,
    ) {
        Ok(tool_result) => {
            finish_connected_action(store, receipt_id, "applied", Some(&tool_result), None)?;
            Ok(ConnectedActionResult {
                receipt_id,
                status: "applied".to_string(),
                tool_result: Some(tool_result),
            })
        }
        Err(error) => {
            let message = error.to_string();
            finish_connected_action(store, receipt_id, "failed", None, Some(&message))?;
            Err(error)
        }
    }
}

pub fn reject_connected_action(
    store: &TaskStore,
    receipt_id: i64,
) -> Result<ConnectedActionResult> {
    let mut conn = store.connect()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let run_changed = tx.execute(
        "UPDATE connected_action_runs
         SET status = 'rejected', resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE receipt_id = ?1 AND status = 'pending_approval'",
        params![receipt_id],
    )?;
    let receipt_changed = tx.execute(
        "UPDATE action_receipts
         SET status = 'rejected', resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1 AND status = 'pending_approval'
           AND class IN ('email.draft', 'email.label', 'calendar.propose')",
        params![receipt_id],
    )?;
    if run_changed != 1 || receipt_changed != 1 {
        return Err(anyhow!("connected action is not pending approval"));
    }
    tx.commit()?;
    Ok(ConnectedActionResult {
        receipt_id,
        status: "rejected".to_string(),
        tool_result: None,
    })
}

fn claim_connected_action(
    store: &TaskStore,
    receipt_id: i64,
) -> Result<(Vec<String>, serde_json::Value)> {
    let mut conn = store.connect()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let pending: Option<(String, String)> = tx
        .query_row(
            "SELECT r.tool_names_json, r.arguments_json
             FROM connected_action_runs r
             JOIN action_receipts a ON a.id = r.receipt_id
             WHERE r.receipt_id = ?1 AND r.status = 'pending_approval'
               AND a.status = 'pending_approval'
               AND a.class IN ('email.draft', 'email.label', 'calendar.propose')",
            params![receipt_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let (tool_names, arguments) =
        pending.ok_or_else(|| anyhow!("connected action is not pending approval"))?;
    let run_changed = tx.execute(
        "UPDATE connected_action_runs SET status = 'running'
         WHERE receipt_id = ?1 AND status = 'pending_approval'",
        params![receipt_id],
    )?;
    let receipt_changed = tx.execute(
        "UPDATE action_receipts SET status = 'approved'
         WHERE id = ?1 AND status = 'pending_approval'",
        params![receipt_id],
    )?;
    if run_changed != 1 || receipt_changed != 1 {
        return Err(anyhow!("connected action approval lost a concurrent race"));
    }
    tx.commit()?;
    Ok((
        serde_json::from_str(&tool_names)?,
        serde_json::from_str(&arguments)?,
    ))
}

fn finish_connected_action(
    store: &TaskStore,
    receipt_id: i64,
    status: &str,
    result: Option<&ToolInvocationResult>,
    error_message: Option<&str>,
) -> Result<()> {
    let mut conn = store.connect()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let result_json = result.map(serde_json::to_string).transpose()?;
    let run_changed = tx.execute(
        "UPDATE connected_action_runs
         SET status = ?1, result_json = ?2, error_message = ?3,
             resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE receipt_id = ?4 AND status = 'running'",
        params![status, result_json, error_message, receipt_id],
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
            "connected action could not be finalized consistently"
        ));
    }
    tx.commit()?;
    Ok(())
}

// ---- invocation ----------------------------------------------------------

// invoke a tool over its connection. Disabled/absent connections and out-of-
// scope tools are rejected (and logged as rejected). Every call is logged with
// an argument summary and timestamp. Live stdio/HTTP dispatch is env-gated; the
// loopback transport responds deterministically for tests and web fixtures.
pub fn invoke_tool(
    store: &TaskStore,
    connection_name: &str,
    tool_name: &str,
    arguments: &ToolArguments,
) -> Result<ToolInvocationResult> {
    let connection = connection_by_name(store, connection_name)?;
    let Some(connection) = connection else {
        log_call(
            store,
            None,
            connection_name,
            tool_name,
            arguments,
            CALL_STATUS_REJECTED,
        )?;
        return Err(anyhow!(
            "tool connection '{connection_name}' is not configured"
        ));
    };
    if !connection.enabled {
        log_call(
            store,
            Some(connection.id),
            connection_name,
            tool_name,
            arguments,
            CALL_STATUS_REJECTED,
        )?;
        return Err(anyhow!(
            "tool connection '{connection_name}' is disconnected"
        ));
    }
    // per-connection scoping: the tool must be discovered for this connection.
    let scoped = list_connection_tools(store, connection.id)?
        .iter()
        .any(|tool| tool.tool_name == tool_name);
    if !scoped {
        log_call(
            store,
            Some(connection.id),
            connection_name,
            tool_name,
            arguments,
            CALL_STATUS_REJECTED,
        )?;
        return Err(anyhow!(
            "tool '{tool_name}' is not in the scope of connection '{connection_name}'"
        ));
    }

    let result = match connection.transport.as_str() {
        TRANSPORT_LOOPBACK => loopback_dispatch(&connection.name, tool_name, arguments),
        TRANSPORT_STDIO | TRANSPORT_HTTP => mcp_request(
            &connection,
            "tools/call",
            serde_json::json!({"name": tool_name, "arguments": arguments.value()}),
        ),
        _ => Err(anyhow!("unsupported transport '{}'", connection.transport)),
    };

    match result {
        Ok(output) => {
            log_call(
                store,
                Some(connection.id),
                connection_name,
                tool_name,
                arguments,
                CALL_STATUS_OK,
            )?;
            Ok(ToolInvocationResult {
                status: CALL_STATUS_OK.to_string(),
                connection: connection.name,
                tool: tool_name.to_string(),
                output,
            })
        }
        Err(err) => {
            log_call(
                store,
                Some(connection.id),
                connection_name,
                tool_name,
                arguments,
                CALL_STATUS_ERROR,
            )?;
            Err(err)
        }
    }
}

fn mcp_request(
    connection: &ToolConnectionDto,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    match connection.transport.as_str() {
        TRANSPORT_STDIO => stdio_mcp_request(&connection.endpoint, method, params),
        TRANSPORT_HTTP => http_mcp_request(connection, method, params),
        other => Err(anyhow!("unsupported MCP transport '{other}'")),
    }
}

fn initialize_request(id: i64) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "jeff", "version": env!("CARGO_PKG_VERSION")}
        }
    })
}

fn rpc_request(id: i64, method: &str, params: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn initialized_notification() -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})
}

fn rpc_result(message: serde_json::Value, id: i64) -> Result<serde_json::Value> {
    if message.get("id").and_then(serde_json::Value::as_i64) != Some(id) {
        return Err(anyhow!("MCP response id did not match request id {id}"));
    }
    if let Some(error) = message.get("error") {
        return Err(anyhow!("MCP server error: {error}"));
    }
    message
        .get("result")
        .cloned()
        .ok_or_else(|| anyhow!("MCP response omitted result"))
}

fn stdio_mcp_request(
    endpoint: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let argv: Vec<String> = serde_json::from_str(endpoint)
        .context("stdio MCP endpoint must be a JSON array of executable arguments")?;
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| anyhow!("stdio MCP endpoint command is empty"))?;
    if !std::path::Path::new(program).is_absolute() {
        return Err(anyhow!("stdio MCP executable must use an absolute path"));
    }
    let mut child = Command::new(program)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to start stdio MCP server")?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("MCP stdin missing"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("MCP stdout missing"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("MCP stderr missing"))?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let _ = tx.send(line);
        }
    });
    std::thread::spawn(move || {
        let mut sink = Vec::new();
        let _ = stderr.take(64 * 1024).read_to_end(&mut sink);
    });
    write_json_line(&mut stdin, &initialize_request(1))?;
    let _ = receive_rpc_result(&rx, 1)?;
    write_json_line(&mut stdin, &initialized_notification())?;
    write_json_line(&mut stdin, &rpc_request(2, method, params))?;
    let result = receive_rpc_result(&rx, 2);
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn write_json_line(writer: &mut impl Write, value: &serde_json::Value) -> Result<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn receive_rpc_result(
    receiver: &mpsc::Receiver<std::io::Result<String>>,
    id: i64,
) -> Result<serde_json::Value> {
    let deadline = std::time::Instant::now() + Duration::from_secs(MCP_REQUEST_TIMEOUT_SECS);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let line = receiver
            .recv_timeout(remaining)
            .map_err(|_| anyhow!("stdio MCP request timed out"))??;
        let message: serde_json::Value =
            serde_json::from_str(&line).context("stdio MCP server emitted invalid JSON")?;
        if message.get("id").and_then(serde_json::Value::as_i64) == Some(id) {
            return rpc_result(message, id);
        }
    }
}

fn http_mcp_request(
    connection: &ToolConnectionDto,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    validate_http_endpoint(&connection.endpoint)?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(MCP_REQUEST_TIMEOUT_SECS))
        .build()?;
    let token = oauth_token(connection)?;
    let (init, session_id) = post_mcp_http(
        &client,
        connection,
        token.as_deref(),
        None,
        &initialize_request(1),
    )?;
    let _ = rpc_result(init, 1)?;
    post_mcp_http_notification(
        &client,
        connection,
        token.as_deref(),
        session_id.as_deref(),
        &initialized_notification(),
    )?;
    let (response, _) = post_mcp_http(
        &client,
        connection,
        token.as_deref(),
        session_id.as_deref(),
        &rpc_request(2, method, params),
    )?;
    rpc_result(response, 2)
}

fn validate_http_endpoint(endpoint: &str) -> Result<()> {
    let url = reqwest::Url::parse(endpoint).context("invalid MCP HTTP endpoint")?;
    let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
    if url.scheme() != "https" && !(url.scheme() == "http" && local) {
        return Err(anyhow!(
            "MCP HTTP endpoints require HTTPS except on localhost"
        ));
    }
    Ok(())
}

fn oauth_token(connection: &ToolConnectionDto) -> Result<Option<String>> {
    let key = format!(
        "JEFF_MCP_OAUTH_TOKEN_{}",
        connection
            .name
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            })
            .collect::<String>()
    );
    let token = std::env::var(&key)
        .ok()
        .filter(|value| !value.trim().is_empty());
    if connection.scopes.iter().any(|scope| scope == "oauth") && token.is_none() {
        return Err(anyhow!("OAuth token required in {key}"));
    }
    Ok(token)
}

fn post_mcp_http(
    client: &reqwest::blocking::Client,
    connection: &ToolConnectionDto,
    token: Option<&str>,
    session_id: Option<&str>,
    body: &serde_json::Value,
) -> Result<(serde_json::Value, Option<String>)> {
    let mut request = client
        .post(&connection.endpoint)
        .header("Accept", "application/json, text/event-stream")
        .json(body);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    if let Some(session_id) = session_id {
        request = request.header("Mcp-Session-Id", session_id);
    }
    let response = request.send()?.error_for_status()?;
    let session = response
        .headers()
        .get("Mcp-Session-Id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let text = response.text()?;
    Ok((parse_http_rpc_body(&content_type, &text)?, session))
}

fn post_mcp_http_notification(
    client: &reqwest::blocking::Client,
    connection: &ToolConnectionDto,
    token: Option<&str>,
    session_id: Option<&str>,
    body: &serde_json::Value,
) -> Result<()> {
    let mut request = client.post(&connection.endpoint).json(body);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    if let Some(session_id) = session_id {
        request = request.header("Mcp-Session-Id", session_id);
    }
    request.send()?.error_for_status()?;
    Ok(())
}

fn parse_http_rpc_body(content_type: &str, body: &str) -> Result<serde_json::Value> {
    if content_type.contains("text/event-stream") {
        let data = body
            .lines()
            .find_map(|line| line.strip_prefix("data:"))
            .map(str::trim)
            .ok_or_else(|| anyhow!("MCP event stream omitted data"))?;
        return serde_json::from_str(data).context("invalid MCP event-stream JSON");
    }
    serde_json::from_str(body).context("invalid MCP HTTP JSON")
}

fn parse_discovered_tools(result: &serde_json::Value) -> Result<Vec<(String, String)>> {
    result
        .get("tools")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow!("MCP tools/list response omitted tools"))?
        .iter()
        .map(|tool| {
            let name = tool
                .get("name")
                .and_then(serde_json::Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .ok_or_else(|| anyhow!("MCP tool omitted name"))?;
            let description = tool
                .get("description")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            Ok((name.to_string(), description.to_string()))
        })
        .collect()
}

// deterministic loopback: echoes the tool + arguments. Web fixtures (E2) and
// tests use this as a stand-in for a real MCP server.
fn loopback_dispatch(
    connection: &str,
    tool_name: &str,
    arguments: &ToolArguments,
) -> Result<serde_json::Value> {
    Ok(serde_json::json!({
        "connection": connection,
        "tool": tool_name,
        "echo": arguments.value(),
    }))
}

fn log_call(
    store: &TaskStore,
    connection_id: Option<i64>,
    connection_name: &str,
    tool_name: &str,
    arguments: &ToolArguments,
    status: &str,
) -> Result<()> {
    let conn = store.connect()?;
    conn.execute(
        "INSERT INTO tool_call_log
         (connection_id, connection_name, tool_name, argument_summary, status)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            connection_id,
            connection_name,
            tool_name,
            arguments.summary(),
            status
        ],
    )
    .context("failed to write tool call log")?;
    Ok(())
}

fn get_connection(store: &TaskStore, id: i64) -> Result<Option<ToolConnectionDto>> {
    let conn = store.connect()?;
    conn.query_row(
        "SELECT id, name, transport, endpoint, scopes_json, enabled, created_at
         FROM tool_connections WHERE id = ?1",
        params![id],
        connection_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn connection_by_name(store: &TaskStore, name: &str) -> Result<Option<ToolConnectionDto>> {
    let conn = store.connect()?;
    conn.query_row(
        "SELECT id, name, transport, endpoint, scopes_json, enabled, created_at
         FROM tool_connections WHERE name = ?1",
        params![name],
        connection_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn connection_from_row(row: &Row<'_>) -> rusqlite::Result<ToolConnectionDto> {
    let scopes_json: String = row.get(4)?;
    Ok(ToolConnectionDto {
        id: row.get(0)?,
        name: row.get(1)?,
        transport: row.get(2)?,
        endpoint: row.get(3)?,
        scopes: serde_json::from_str(&scopes_json).unwrap_or_default(),
        enabled: row.get::<_, i64>(5)? != 0,
        created_at: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;
    use tempfile::TempDir;

    fn test_store() -> (TempDir, TaskStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        (dir, store)
    }

    fn args(json: serde_json::Value) -> ToolArguments {
        ToolArguments::from_value(json).unwrap()
    }

    #[test]
    fn e1_data_boundary_rejects_ambient_context() {
        // explicit args are fine.
        assert!(ToolArguments::from_value(serde_json::json!({"query": "weather"})).is_ok());
        // ambient context dumps are refused.
        for key in [
            "snapshot",
            "memory_recall",
            "relational_context",
            "user_profile",
            "episodes",
        ] {
            let payload = serde_json::json!({ key: {"anything": 1} });
            assert!(
                ToolArguments::from_value(payload).is_err(),
                "must reject ambient key {key}"
            );
        }
        // oversized payloads (context dumps) are refused.
        let big = "x".repeat(MAX_TOOL_ARGUMENTS_BYTES + 1);
        assert!(ToolArguments::from_value(serde_json::json!({ "q": big })).is_err());
        // the boundary holds at depth: an ambient key nested under an innocuous
        // outer key, or inside an array, is still refused.
        assert!(
            ToolArguments::from_value(serde_json::json!({ "q": { "memory": {"x": 1} } })).is_err(),
            "must reject ambient key nested in an object"
        );
        assert!(
            ToolArguments::from_value(serde_json::json!({ "items": [ {"user_profile": 1} ] }))
                .is_err(),
            "must reject ambient key nested in an array"
        );
        // a deep, innocuous payload is still allowed.
        assert!(
            ToolArguments::from_value(serde_json::json!({ "q": { "filters": ["recent"] } }))
                .is_ok()
        );
    }

    #[test]
    fn e1_invocation_is_logged_with_summary_and_timestamp() {
        let (_dir, store) = test_store();
        let connection =
            add_tool_connection(&store, "test-mcp", TRANSPORT_LOOPBACK, "loopback://", &[])
                .unwrap();
        register_connection_tools(
            &store,
            connection.id,
            &[("search".to_string(), "search".to_string())],
        )
        .unwrap();

        let result = invoke_tool(
            &store,
            "test-mcp",
            "search",
            &args(serde_json::json!({"query": "x"})),
        )
        .unwrap();
        assert_eq!(result.status, CALL_STATUS_OK);

        let log = list_tool_call_log(&store, 10).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].tool_name, "search");
        assert!(log[0].argument_summary.contains("query"));
        assert!(!log[0].created_at.is_empty());
        assert_eq!(log[0].status, CALL_STATUS_OK);
    }

    #[test]
    fn e1_disconnect_stops_calls_and_purges_tools() {
        let (_dir, store) = test_store();
        let connection =
            add_tool_connection(&store, "gmail", TRANSPORT_LOOPBACK, "loopback://", &[]).unwrap();
        register_connection_tools(
            &store,
            connection.id,
            &[("list".to_string(), "list".to_string())],
        )
        .unwrap();
        invoke_tool(
            &store,
            "gmail",
            "list",
            &args(serde_json::json!({"q": "a"})),
        )
        .unwrap();

        // disconnect via disable: purges discovered tools, blocks calls.
        set_tool_connection_enabled(&store, connection.id, false).unwrap();
        assert!(list_connection_tools(&store, connection.id)
            .unwrap()
            .is_empty());
        let err = invoke_tool(
            &store,
            "gmail",
            "list",
            &args(serde_json::json!({"q": "b"})),
        );
        assert!(err.is_err());
        // the rejected attempt is logged (never dispatched).
        let log = list_tool_call_log(&store, 10).unwrap();
        assert!(log.iter().any(|entry| entry.status == CALL_STATUS_REJECTED));

        // full removal purges the connection.
        remove_tool_connection(&store, connection.id).unwrap();
        assert!(list_tool_connections(&store).unwrap().is_empty());
    }

    #[test]
    fn e1_out_of_scope_tool_is_rejected() {
        let (_dir, store) = test_store();
        let connection =
            add_tool_connection(&store, "drive", TRANSPORT_LOOPBACK, "loopback://", &[]).unwrap();
        register_connection_tools(
            &store,
            connection.id,
            &[("read".to_string(), "read".to_string())],
        )
        .unwrap();
        // a tool not discovered for this connection cannot be called.
        assert!(invoke_tool(&store, "drive", "delete", &args(serde_json::json!({}))).is_err());
    }

    #[test]
    fn e1_stdio_transport_initializes_discovers_and_invokes() {
        let (_dir, store) = test_store();
        let server = r#"import json,sys
for line in sys.stdin:
 m=json.loads(line)
 if m.get('method')=='initialize': r={'jsonrpc':'2.0','id':m['id'],'result':{'protocolVersion':'2025-03-26','capabilities':{},'serverInfo':{'name':'fixture','version':'1'}}}
 elif m.get('method')=='tools/list': r={'jsonrpc':'2.0','id':m['id'],'result':{'tools':[{'name':'echo','description':'echo input','inputSchema':{'type':'object'}}]}}
 elif m.get('method')=='tools/call': r={'jsonrpc':'2.0','id':m['id'],'result':{'content':[{'type':'text','text':m['params']['arguments']['text']}]}}
 else: continue
 print(json.dumps(r),flush=True)"#;
        let endpoint =
            serde_json::to_string(&vec!["/usr/bin/python3", "-u", "-c", server]).unwrap();
        let connection =
            add_tool_connection(&store, "stdio-fixture", TRANSPORT_STDIO, &endpoint, &[]).unwrap();
        let tools = discover_connection_tools(&store, connection.id).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool_name, "echo");
        let result = invoke_tool(
            &store,
            "stdio-fixture",
            "echo",
            &args(serde_json::json!({"text": "hello"})),
        )
        .unwrap();
        assert_eq!(result.output["content"][0]["text"], "hello");
    }

    #[test]
    fn e1_http_transport_requires_secure_remote_endpoint_and_oauth_token() {
        assert!(validate_http_endpoint("https://mcp.example.test/rpc").is_ok());
        assert!(validate_http_endpoint("http://127.0.0.1:3000/mcp").is_ok());
        assert!(validate_http_endpoint("http://mcp.example.test/rpc").is_err());

        let (_dir, store) = test_store();
        let connection = add_tool_connection(
            &store,
            "missing-token-fixture",
            TRANSPORT_HTTP,
            "https://mcp.example.test/rpc",
            &["oauth".to_string()],
        )
        .unwrap();
        assert!(oauth_token(&connection).is_err());
    }

    #[test]
    fn e1_http_transport_initializes_discovers_and_invokes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..6 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 4096];
                let header_end;
                loop {
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&buffer[..count]);
                    if let Some(position) = request.windows(4).position(|part| part == b"\r\n\r\n")
                    {
                        header_end = position + 4;
                        break;
                    }
                }
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                while request.len() < header_end + content_length {
                    let count = stream.read(&mut buffer).unwrap();
                    request.extend_from_slice(&buffer[..count]);
                }
                let message: serde_json::Value =
                    serde_json::from_slice(&request[header_end..header_end + content_length])
                        .unwrap();
                let method = message.get("method").and_then(serde_json::Value::as_str);
                if message.get("id").is_none() {
                    stream
                        .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
                        .unwrap();
                    continue;
                }
                let result = match method {
                    Some("initialize") => serde_json::json!({
                        "protocolVersion": MCP_PROTOCOL_VERSION,
                        "capabilities": {},
                        "serverInfo": {"name": "http-fixture", "version": "1"}
                    }),
                    Some("tools/list") => serde_json::json!({
                        "tools": [{"name": "echo", "description": "echo input", "inputSchema": {"type": "object"}}]
                    }),
                    Some("tools/call") => serde_json::json!({
                        "content": [{"type": "text", "text": message["params"]["arguments"]["text"]}]
                    }),
                    _ => panic!("unexpected method"),
                };
                let response =
                    serde_json::json!({"jsonrpc": "2.0", "id": message["id"], "result": result})
                        .to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: fixture-session\r\nContent-Length: {}\r\n\r\n{}",
                    response.len(),
                    response
                )
                .unwrap();
            }
        });

        let (_dir, store) = test_store();
        let connection = add_tool_connection(
            &store,
            "http-fixture",
            TRANSPORT_HTTP,
            &format!("http://{address}/mcp"),
            &[],
        )
        .unwrap();
        let tools = discover_connection_tools(&store, connection.id).unwrap();
        assert_eq!(tools[0].tool_name, "echo");
        let result = invoke_tool(
            &store,
            "http-fixture",
            "echo",
            &args(serde_json::json!({"text": "hello-http"})),
        )
        .unwrap();
        assert_eq!(result.output["content"][0]["text"], "hello-http");
        server.join().unwrap();
    }
}
