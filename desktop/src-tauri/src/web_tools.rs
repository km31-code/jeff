// apex e2: web research tools. web.search + web.fetch as agent tools with a
// per-job source ledger, an hourly rate limit, a query log, and a user-name
// query guard (absorbed from the phase 34 spec). production resolves enabled
// MCP web.search/web.fetch tools; a configured local corpus is the explicit
// offline fallback used by deterministic evaluation.

#![cfg_attr(test, allow(dead_code))]

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::{models::WebQueryLogDto, store::TaskStore};

pub const WEB_RATE_LIMIT_PER_HOUR: i64 = 10;
pub const WEB_USER_NAME_GUARD_KEY: &str = "web:user_name_guard";
pub const WEB_CORPUS_DIR_KEY: &str = "web:corpus_dir";
pub const WEB_TOOL_SEARCH: &str = "web.search";
pub const WEB_TOOL_FETCH: &str = "web.fetch";

pub const QUERY_STATUS_OK: &str = "ok";
pub const QUERY_STATUS_RATE_LIMITED: &str = "rate_limited";
pub const QUERY_STATUS_BLOCKED: &str = "blocked";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebDocument {
    pub url: String,
    pub title: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebSource {
    pub url: String,
    pub title: String,
    pub snippet: String,
}

// the user-name guard prevents Jeff from searching the open web for the user by
// name. Configured explicitly; empty disables it.
pub fn set_user_name_guard(store: &TaskStore, name: &str) -> Result<()> {
    store.set_app_setting(WEB_USER_NAME_GUARD_KEY, name.trim())
}

pub fn query_blocked_by_user_guard(store: &TaskStore, query: &str) -> bool {
    let Some(name) = store
        .get_app_setting(WEB_USER_NAME_GUARD_KEY)
        .ok()
        .flatten()
    else {
        return false;
    };
    let name = name.trim();
    !name.is_empty()
        && query
            .to_ascii_lowercase()
            .contains(&name.to_ascii_lowercase())
}

pub fn within_rate_limit(store: &TaskStore) -> bool {
    recent_query_count(store)
        .map(|n| n < WEB_RATE_LIMIT_PER_HOUR)
        .unwrap_or(false)
}

fn recent_query_count(store: &TaskStore) -> Result<i64> {
    let conn = store.connect()?;
    conn.query_row(
        "SELECT COUNT(*) FROM web_query_log
         WHERE status = ?1 AND created_at >= datetime('now', '-1 hour')",
        params![QUERY_STATUS_OK],
        |row| row.get(0),
    )
    .context("failed to count recent web queries")
}

fn log_query(
    store: &TaskStore,
    query: &str,
    tool: &str,
    result_count: i64,
    status: &str,
) -> Result<()> {
    let conn = store.connect()?;
    conn.execute(
        "INSERT INTO web_query_log (query, tool, result_count, status) VALUES (?1, ?2, ?3, ?4)",
        params![
            query.chars().take(200).collect::<String>(),
            tool,
            result_count,
            status
        ],
    )
    .context("failed to log web query")?;
    Ok(())
}

pub fn list_web_query_log(store: &TaskStore, limit: usize) -> Result<Vec<WebQueryLogDto>> {
    let conn = store.connect()?;
    let max = limit.min(50) as i64;
    let mut stmt = conn.prepare(
        "SELECT id, query, tool, result_count, status, created_at
         FROM web_query_log ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![max], |row| {
            Ok(WebQueryLogDto {
                id: row.get(0)?,
                query: row.get(1)?,
                tool: row.get(2)?,
                result_count: row.get(3)?,
                status: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// web.search: guarded, rate-limited keyword search over the corpus. Returns
// ranked sources. The corpus is a fixture stand-in for live results.
pub fn web_search(
    store: &TaskStore,
    query: &str,
    corpus: &[WebDocument],
) -> Result<Vec<WebSource>> {
    let clean = query.trim();
    if clean.is_empty() {
        return Err(anyhow!("web search query cannot be empty"));
    }
    if query_blocked_by_user_guard(store, clean) {
        log_query(store, clean, WEB_TOOL_SEARCH, 0, QUERY_STATUS_BLOCKED)?;
        return Err(anyhow!("web search blocked by the user-name guard"));
    }
    if !within_rate_limit(store) {
        log_query(store, clean, WEB_TOOL_SEARCH, 0, QUERY_STATUS_RATE_LIMITED)?;
        return Err(anyhow!(
            "web search rate limit reached ({}/hour)",
            WEB_RATE_LIMIT_PER_HOUR
        ));
    }
    let terms = tokenize(clean);
    let mut scored: Vec<(usize, &WebDocument)> = corpus
        .iter()
        .map(|doc| (relevance(doc, &terms), doc))
        .filter(|(score, _)| *score > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    let sources: Vec<WebSource> = scored
        .into_iter()
        .take(3)
        .map(|(_, doc)| WebSource {
            url: doc.url.clone(),
            title: doc.title.clone(),
            snippet: doc.content.chars().take(160).collect(),
        })
        .collect();
    log_query(
        store,
        clean,
        WEB_TOOL_SEARCH,
        sources.len() as i64,
        QUERY_STATUS_OK,
    )?;
    Ok(sources)
}

// web.fetch: readable full-text extraction for one source url.
pub fn web_fetch(store: &TaskStore, url: &str, corpus: &[WebDocument]) -> Result<WebDocument> {
    let doc = corpus
        .iter()
        .find(|doc| doc.url == url)
        .cloned()
        .ok_or_else(|| anyhow!("web fetch: url not in reachable corpus: {url}"))?;
    log_query(store, url, WEB_TOOL_FETCH, 1, QUERY_STATUS_OK)?;
    Ok(WebDocument {
        content: readable_extract(&doc.content),
        ..doc
    })
}

// claim->source: every source used in a deliverable carries its url so the
// verification pass can enforce citation. Uncited sources cannot appear.
pub fn build_source_ledger(sources: &[WebSource]) -> Vec<serde_json::Value> {
    sources
        .iter()
        .map(|source| {
            serde_json::json!({
                "url": source.url,
                "title": source.title,
                "file_name": source.url,
            })
        })
        .collect()
}

// load a fixture corpus. Each file's first two lines may be `URL: ...` and
// `TITLE: ...`; the remainder is the body.
pub fn load_web_fixture_corpus(dir: &Path) -> Result<Vec<WebDocument>> {
    let mut docs = Vec::new();
    let mut entries = std::fs::read_dir(dir)
        .with_context(|| format!("missing web fixture corpus {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if !entry.file_type()?.is_file() {
            continue;
        }
        let raw = std::fs::read_to_string(entry.path())?;
        docs.push(parse_web_fixture(
            &raw,
            &entry.file_name().to_string_lossy(),
        ));
    }
    Ok(docs)
}

// the reachable corpus. Live web search against the open web is env-gated; a
// configured fixture corpus dir stands in deterministically.
pub fn set_corpus_dir(store: &TaskStore, dir: &str) -> Result<()> {
    store.set_app_setting(WEB_CORPUS_DIR_KEY, dir.trim())
}

pub fn configured_corpus(store: &TaskStore) -> Result<Vec<WebDocument>> {
    let dir = store
        .get_app_setting(WEB_CORPUS_DIR_KEY)?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            anyhow!("web research corpus is not configured; live web search is env-gated")
        })?;
    load_web_fixture_corpus(Path::new(dir.trim()))
}

// high-level agent-facing search: returns ranked sources plus the per-job source
// ledger the verification pass uses to enforce claim->source citation.
pub fn search(store: &TaskStore, query: &str) -> Result<(Vec<WebSource>, Vec<serde_json::Value>)> {
    let clean = validate_live_query(store, query)?;
    let sources = if crate::tool_bus::has_enabled_tool(store, &[WEB_TOOL_SEARCH, "search"])? {
        let sources = connected_search(store, clean)?;
        log_query(
            store,
            clean,
            WEB_TOOL_SEARCH,
            sources.len() as i64,
            QUERY_STATUS_OK,
        )?;
        sources
    } else {
        web_search(store, clean, &configured_corpus(store)?)?
    };
    let ledger = build_source_ledger(&sources);
    Ok((sources, ledger))
}

pub fn fetch(store: &TaskStore, url: &str) -> Result<WebDocument> {
    if crate::tool_bus::has_enabled_tool(store, &[WEB_TOOL_FETCH, "fetch"])? {
        let document = connected_fetch(store, url)?;
        log_query(store, url, WEB_TOOL_FETCH, 1, QUERY_STATUS_OK)?;
        Ok(document)
    } else {
        web_fetch(store, url, &configured_corpus(store)?)
    }
}

fn validate_live_query<'a>(store: &TaskStore, query: &'a str) -> Result<&'a str> {
    let clean = query.trim();
    if clean.is_empty() {
        return Err(anyhow!("web search query cannot be empty"));
    }
    if query_blocked_by_user_guard(store, clean) {
        log_query(store, clean, WEB_TOOL_SEARCH, 0, QUERY_STATUS_BLOCKED)?;
        return Err(anyhow!("web search blocked by the user-name guard"));
    }
    if !within_rate_limit(store) {
        log_query(store, clean, WEB_TOOL_SEARCH, 0, QUERY_STATUS_RATE_LIMITED)?;
        return Err(anyhow!(
            "web search rate limit reached ({}/hour)",
            WEB_RATE_LIMIT_PER_HOUR
        ));
    }
    Ok(clean)
}

fn connected_search(store: &TaskStore, query: &str) -> Result<Vec<WebSource>> {
    let result = crate::tool_bus::invoke_first_enabled_tool(
        store,
        &[WEB_TOOL_SEARCH, "search"],
        serde_json::json!({"query": query, "limit": 5}),
    )?;
    let payload = crate::tool_bus::tool_result_payload(&result.output)?;
    let sources = payload
        .get("sources")
        .or_else(|| payload.get("results"))
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow!("web.search result omitted sources"))?;
    sources.iter().take(5).map(parse_connected_source).collect()
}

fn parse_connected_source(value: &serde_json::Value) -> Result<WebSource> {
    let url = value
        .get("url")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("web.search source omitted url"))?;
    validate_public_source_url(url)?;
    Ok(WebSource {
        url: url.to_string(),
        title: value
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(url)
            .chars()
            .take(500)
            .collect(),
        snippet: value
            .get("snippet")
            .or_else(|| value.get("text"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .chars()
            .take(2_000)
            .collect(),
    })
}

fn connected_fetch(store: &TaskStore, url: &str) -> Result<WebDocument> {
    validate_public_source_url(url)?;
    let result = crate::tool_bus::invoke_first_enabled_tool(
        store,
        &[WEB_TOOL_FETCH, "fetch"],
        serde_json::json!({"url": url}),
    )?;
    let payload = crate::tool_bus::tool_result_payload(&result.output)?;
    let content = payload
        .get("content")
        .or_else(|| payload.get("text"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("web.fetch result omitted content"))?;
    Ok(WebDocument {
        url: url.to_string(),
        title: payload
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(url)
            .chars()
            .take(500)
            .collect(),
        content: readable_extract(&content.chars().take(2_000_000).collect::<String>()),
    })
}

fn validate_public_source_url(url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url).context("invalid web source url")?;
    if parsed.scheme() != "https" {
        return Err(anyhow!("web sources must use https"));
    }
    if matches!(
        parsed.host_str(),
        Some("localhost" | "127.0.0.1" | "::1") | None
    ) {
        return Err(anyhow!("web source host is not public"));
    }
    Ok(())
}

fn parse_web_fixture(raw: &str, fallback_name: &str) -> WebDocument {
    let mut url = format!("https://fixtures.local/{fallback_name}");
    let mut title = fallback_name.to_string();
    let mut body_lines = Vec::new();
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("URL:") {
            url = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("TITLE:") {
            title = rest.trim().to_string();
        } else {
            body_lines.push(line);
        }
    }
    WebDocument {
        url,
        title,
        content: body_lines.join("\n").trim().to_string(),
    }
}

fn tokenize(text: &str) -> Vec<String> {
    text.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 3)
        .map(str::to_string)
        .collect()
}

fn relevance(doc: &WebDocument, terms: &[String]) -> usize {
    let haystack = format!("{} {}", doc.title, doc.content).to_ascii_lowercase();
    terms
        .iter()
        .filter(|term| haystack.contains(term.as_str()))
        .count()
}

// minimal readability: collapse whitespace and drop obvious nav/boilerplate.
fn readable_extract(content: &str) -> String {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.eq_ignore_ascii_case("home") && !line.eq_ignore_ascii_case("menu"))
        .collect::<Vec<_>>()
        .join("\n")
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

    fn corpus() -> Vec<WebDocument> {
        vec![
            WebDocument {
                url: "https://example.com/local-first".to_string(),
                title: "Local-first agents".to_string(),
                content: "Local-first agents keep the world model on device.\nMenu\nThey reduce data exposure.".to_string(),
            },
            WebDocument {
                url: "https://example.com/verification".to_string(),
                title: "Verification boundaries".to_string(),
                content: "Reliable systems depend on explicit verification boundaries.".to_string(),
            },
        ]
    }

    #[test]
    fn e2_search_ranks_and_logs_and_fetch_extracts() {
        let (_dir, store) = test_store();
        let sources = web_search(&store, "local-first agents on device", &corpus()).unwrap();
        assert_eq!(sources[0].url, "https://example.com/local-first");
        let doc = web_fetch(&store, &sources[0].url, &corpus()).unwrap();
        assert!(doc.content.contains("world model on device"));
        // readability drops the "Menu" boilerplate line.
        assert!(!doc.content.contains("Menu"));
        let log = list_web_query_log(&store, 10).unwrap();
        assert!(log
            .iter()
            .any(|entry| entry.tool == WEB_TOOL_SEARCH && entry.status == QUERY_STATUS_OK));
        assert!(log.iter().any(|entry| entry.tool == WEB_TOOL_FETCH));
    }

    #[test]
    fn e2_source_ledger_carries_urls_for_citation() {
        let ledger = build_source_ledger(&[WebSource {
            url: "https://example.com/x".to_string(),
            title: "X".to_string(),
            snippet: "s".to_string(),
        }]);
        assert_eq!(ledger[0].get("url").unwrap(), "https://example.com/x");
        assert_eq!(ledger[0].get("file_name").unwrap(), "https://example.com/x");
    }

    #[test]
    fn e2_rate_limit_trips_after_ten_per_hour() {
        let (_dir, store) = test_store();
        for _ in 0..WEB_RATE_LIMIT_PER_HOUR {
            web_search(&store, "verification boundaries", &corpus()).unwrap();
        }
        assert!(!within_rate_limit(&store));
        let err = web_search(&store, "verification boundaries", &corpus());
        assert!(err.is_err());
        let log = list_web_query_log(&store, 20).unwrap();
        assert!(log
            .iter()
            .any(|entry| entry.status == QUERY_STATUS_RATE_LIMITED));
    }

    #[test]
    fn e2_user_name_guard_blocks_matching_query() {
        let (_dir, store) = test_store();
        set_user_name_guard(&store, "Ada Lovelace").unwrap();
        let err = web_search(&store, "who is ada lovelace", &corpus());
        assert!(err.is_err());
        let log = list_web_query_log(&store, 10).unwrap();
        assert!(log.iter().any(|entry| entry.status == QUERY_STATUS_BLOCKED));
        // an unrelated query is allowed.
        assert!(web_search(&store, "verification boundaries", &corpus()).is_ok());
    }

    #[test]
    fn e2_connected_web_search_and_fetch_use_discovered_mcp_tools() {
        let (_dir, store) = test_store();
        let server = r#"import json,sys
for line in sys.stdin:
 m=json.loads(line)
 if m.get('method')=='initialize': result={'protocolVersion':'2025-03-26','capabilities':{},'serverInfo':{'name':'web-fixture','version':'1'}}
 elif m.get('method')=='tools/list': result={'tools':[{'name':'web.search','description':'search','inputSchema':{'type':'object'}},{'name':'web.fetch','description':'fetch','inputSchema':{'type':'object'}}]}
 elif m.get('method')=='tools/call' and m['params']['name']=='web.search': result={'structuredContent':{'sources':[{'url':'https://example.com/research','title':'Research','snippet':'Grounded result'}]}}
 elif m.get('method')=='tools/call': result={'structuredContent':{'title':'Research','content':'Home\nGrounded full article text.\nMenu'}}
 else: continue
 print(json.dumps({'jsonrpc':'2.0','id':m['id'],'result':result}),flush=True)"#;
        let endpoint =
            serde_json::to_string(&vec!["/usr/bin/python3", "-u", "-c", server]).unwrap();
        let connection = crate::tool_bus::add_tool_connection(
            &store,
            "web-fixture",
            crate::tool_bus::TRANSPORT_STDIO,
            &endpoint,
            &[],
        )
        .unwrap();
        crate::tool_bus::discover_connection_tools(&store, connection.id).unwrap();

        let (sources, ledger) = search(&store, "grounded research").unwrap();
        assert_eq!(sources[0].url, "https://example.com/research");
        assert_eq!(ledger[0]["url"], "https://example.com/research");
        let document = fetch(&store, &sources[0].url).unwrap();
        assert!(document.content.contains("Grounded full article text"));
        assert!(!document.content.contains("Home"));
        assert!(!document.content.contains("Menu"));
    }
}
