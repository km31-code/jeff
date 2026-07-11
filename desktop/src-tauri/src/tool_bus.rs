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
// Live MCP transport (stdio + HTTP) dispatch is env-gated (needs a running MCP
// server / OAuth); the connection manager, scoping, data boundary, call log,
// and disconnect-purge are deterministic and tested via a loopback transport.

#![cfg_attr(test, allow(dead_code))]

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
        for key in obj.keys() {
            let lower = key.to_ascii_lowercase();
            if AMBIENT_CONTEXT_KEYS
                .iter()
                .any(|marker| lower == *marker || lower.contains(marker))
            {
                return Err(anyhow!(
                    "tool arguments may not carry ambient context (key '{key}')"
                ));
            }
        }
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolInvocationResult {
    pub status: String,
    pub connection: String,
    pub tool: String,
    pub output: serde_json::Value,
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
    if !matches!(transport, TRANSPORT_STDIO | TRANSPORT_HTTP | TRANSPORT_LOOPBACK) {
        return Err(anyhow!("unsupported transport '{transport}'"));
    }
    let conn = store.connect()?;
    conn.execute(
        "INSERT INTO tool_connections (name, transport, endpoint, scopes_json, enabled)
         VALUES (?1, ?2, ?3, ?4, 1)",
        params![name, transport, endpoint.trim(), serde_json::to_string(scopes)?],
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
        log_call(store, None, connection_name, tool_name, arguments, CALL_STATUS_REJECTED)?;
        return Err(anyhow!("tool connection '{connection_name}' is not configured"));
    };
    if !connection.enabled {
        log_call(store, Some(connection.id), connection_name, tool_name, arguments, CALL_STATUS_REJECTED)?;
        return Err(anyhow!("tool connection '{connection_name}' is disconnected"));
    }
    // per-connection scoping: the tool must be discovered for this connection.
    let scoped = list_connection_tools(store, connection.id)?
        .iter()
        .any(|tool| tool.tool_name == tool_name);
    if !scoped {
        log_call(store, Some(connection.id), connection_name, tool_name, arguments, CALL_STATUS_REJECTED)?;
        return Err(anyhow!(
            "tool '{tool_name}' is not in the scope of connection '{connection_name}'"
        ));
    }

    let result = match connection.transport.as_str() {
        TRANSPORT_LOOPBACK => loopback_dispatch(&connection.name, tool_name, arguments),
        // live transports are env-gated: the connection is governed and logged,
        // but dispatch requires a running MCP server / OAuth session.
        _ => Err(anyhow!(
            "live MCP dispatch for transport '{}' is not available in this environment",
            connection.transport
        )),
    };

    match result {
        Ok(output) => {
            log_call(store, Some(connection.id), connection_name, tool_name, arguments, CALL_STATUS_OK)?;
            Ok(ToolInvocationResult {
                status: CALL_STATUS_OK.to_string(),
                connection: connection.name,
                tool: tool_name.to_string(),
                output,
            })
        }
        Err(err) => {
            log_call(store, Some(connection.id), connection_name, tool_name, arguments, CALL_STATUS_ERROR)?;
            Err(err)
        }
    }
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
        params![connection_id, connection_name, tool_name, arguments.summary(), status],
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
        for key in ["snapshot", "memory_recall", "relational_context", "user_profile", "episodes"] {
            let payload = serde_json::json!({ key: {"anything": 1} });
            assert!(
                ToolArguments::from_value(payload).is_err(),
                "must reject ambient key {key}"
            );
        }
        // oversized payloads (context dumps) are refused.
        let big = "x".repeat(MAX_TOOL_ARGUMENTS_BYTES + 1);
        assert!(ToolArguments::from_value(serde_json::json!({ "q": big })).is_err());
    }

    #[test]
    fn e1_invocation_is_logged_with_summary_and_timestamp() {
        let (_dir, store) = test_store();
        let connection = add_tool_connection(&store, "test-mcp", TRANSPORT_LOOPBACK, "loopback://", &[])
            .unwrap();
        register_connection_tools(&store, connection.id, &[("search".to_string(), "search".to_string())])
            .unwrap();

        let result = invoke_tool(&store, "test-mcp", "search", &args(serde_json::json!({"query": "x"})))
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
        let connection = add_tool_connection(&store, "gmail", TRANSPORT_LOOPBACK, "loopback://", &[])
            .unwrap();
        register_connection_tools(&store, connection.id, &[("list".to_string(), "list".to_string())])
            .unwrap();
        invoke_tool(&store, "gmail", "list", &args(serde_json::json!({"q": "a"}))).unwrap();

        // disconnect via disable: purges discovered tools, blocks calls.
        set_tool_connection_enabled(&store, connection.id, false).unwrap();
        assert!(list_connection_tools(&store, connection.id).unwrap().is_empty());
        let err = invoke_tool(&store, "gmail", "list", &args(serde_json::json!({"q": "b"})));
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
        let connection = add_tool_connection(&store, "drive", TRANSPORT_LOOPBACK, "loopback://", &[])
            .unwrap();
        register_connection_tools(&store, connection.id, &[("read".to_string(), "read".to_string())])
            .unwrap();
        // a tool not discovered for this connection cannot be called.
        assert!(invoke_tool(&store, "drive", "delete", &args(serde_json::json!({}))).is_err());
    }
}
