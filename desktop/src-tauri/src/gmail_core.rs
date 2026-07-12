// apex e3: Gmail read and draft. The inbox becomes context Jeff can triage and
// a surface it can prepare -- never send on. Triage decides which messages
// matter from the world model (deadlines, known people, active goals, urgency);
// threads summarize on demand; replies are drafted through the action bus as
// email.draft (email.send stays hard-capped at L1 in trust). "Tell me when
// Sarah replies" registers a reply watch that fires the AwaitedReplyLanded
// crisis class (crisis_core) when a matching message lands.
//
// Live Gmail (OAuth read/draft/label) is env-gated; triage, summarization,
// reply-watch matching, and the draft action are deterministic and tested,
// including a 50-message triage eval with labeled ground truth.

#![cfg_attr(test, allow(dead_code))]

use anyhow::{Context, Result};
use rusqlite::{params, Row};
use serde::{Deserialize, Serialize};

use crate::{
    model_router::{GenerateOptions, ModelRouter, Tier},
    models::{ActionReceiptDto, EmailReplyWatchDto, EmailTriageFlagDto},
    store::TaskStore,
};

pub const WATCH_STATUS_WATCHING: &str = "watching";
pub const WATCH_STATUS_LANDED: &str = "landed";
pub const TRIAGE_MATTERS_THRESHOLD: i32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmailMessage {
    pub id: String,
    #[serde(default)]
    pub thread_id: String,
    pub sender: String,
    pub subject: String,
    #[serde(default)]
    pub snippet: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TriageContext {
    #[serde(default)]
    pub known_people: Vec<String>,
    #[serde(default)]
    pub active_goal_terms: Vec<String>,
}

const DEADLINE_TERMS: &[&str] = &[
    "deadline",
    "due",
    "by friday",
    "by monday",
    "tomorrow",
    "eod",
    "end of day",
    "by end of",
    "expires",
    "closes",
];
const URGENCY_TERMS: &[&str] = &[
    "urgent",
    "asap",
    "action required",
    "please review",
    "needs your reply",
    "response needed",
    "reply needed",
    "time sensitive",
    "important",
];
const NOISE_SENDERS: &[&str] = &[
    "no-reply",
    "noreply",
    "newsletter",
    "notifications",
    "digest",
    "updates@",
    "marketing",
    "promo",
];
const NOISE_TERMS: &[&str] = &[
    "unsubscribe",
    "% off",
    "sale",
    "webinar",
    "new features",
    "weekly digest",
    "your receipt",
    "promotion",
    "limited time",
];

// deterministic triage score. The Judgment-tier upgrade (env-gated) refines this
// from the full world model; the heuristic is the tested fallback.
pub fn triage_score(message: &EmailMessage, context: &TriageContext) -> (i32, String) {
    let haystack =
        format!("{} {} {}", message.sender, message.subject, message.snippet).to_ascii_lowercase();
    let sender_lower = message.sender.to_ascii_lowercase();
    let mut score = 0i32;
    let mut reasons: Vec<String> = Vec::new();

    if context.known_people.iter().any(|person| {
        !person.trim().is_empty() && sender_lower.contains(&person.to_ascii_lowercase())
    }) {
        score += 2;
        reasons.push("from someone you work with".to_string());
    }
    if DEADLINE_TERMS.iter().any(|term| haystack.contains(term)) {
        score += 2;
        reasons.push("mentions a deadline".to_string());
    }
    if URGENCY_TERMS.iter().any(|term| haystack.contains(term)) {
        score += 2;
        reasons.push("is time-sensitive".to_string());
    }
    if context
        .active_goal_terms
        .iter()
        .any(|term| !term.trim().is_empty() && haystack.contains(&term.to_ascii_lowercase()))
    {
        score += 1;
        reasons.push("touches an active goal".to_string());
    }
    if NOISE_SENDERS
        .iter()
        .any(|marker| sender_lower.contains(marker))
    {
        score -= 3;
    }
    if NOISE_TERMS.iter().any(|term| haystack.contains(term)) {
        score -= 2;
    }

    let reason = if reasons.is_empty() {
        "no clear signal".to_string()
    } else {
        reasons.join("; ")
    };
    (score, reason)
}

#[allow(dead_code)]
pub fn message_matters(message: &EmailMessage, context: &TriageContext) -> bool {
    triage_score(message, context).0 >= TRIAGE_MATTERS_THRESHOLD
}

pub fn triage_inbox(messages: &[EmailMessage], context: &TriageContext) -> Vec<EmailTriageFlagDto> {
    messages
        .iter()
        .filter_map(|message| {
            let (score, reason) = triage_score(message, context);
            (score >= TRIAGE_MATTERS_THRESHOLD).then(|| EmailTriageFlagDto {
                message_id: message.id.clone(),
                subject: message.subject.clone(),
                matters_because: reason,
            })
        })
        .collect()
}

// precision on the "matters" label: of the messages flagged, how many truly
// matter. Used by the triage eval's >80% bar.
#[allow(dead_code)]
pub fn triage_precision(
    messages: &[EmailMessage],
    labels: &[bool],
    context: &TriageContext,
) -> (f32, usize, usize) {
    let mut true_positive = 0usize;
    let mut flagged = 0usize;
    for (message, matters) in messages.iter().zip(labels.iter()) {
        if message_matters(message, context) {
            flagged += 1;
            if *matters {
                true_positive += 1;
            }
        }
    }
    let precision = if flagged == 0 {
        0.0
    } else {
        true_positive as f32 / flagged as f32
    };
    (precision, true_positive, flagged)
}

// extractive thread summary: first sentence of each message, newest last.
pub fn summarize_thread(messages: &[EmailMessage]) -> String {
    messages
        .iter()
        .map(|message| {
            let first = message
                .snippet
                .split(['.', '!', '?'])
                .next()
                .unwrap_or(&message.snippet)
                .trim();
            format!("{}: {}", message.sender, first)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn connected_messages(store: &TaskStore, query: &str) -> Result<Vec<EmailMessage>> {
    let result = crate::tool_bus::invoke_first_enabled_tool(
        store,
        &["gmail.search_messages", "gmail.list_messages"],
        serde_json::json!({"query": query, "limit": 100}),
    )?;
    let payload = crate::tool_bus::tool_result_payload(&result.output)?;
    let messages = payload
        .get("messages")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("Gmail tool result omitted messages"))?;
    messages
        .iter()
        .take(100)
        .map(|message| serde_json::from_value(message.clone()).context("invalid Gmail message"))
        .collect()
}

pub fn connected_thread(store: &TaskStore, thread_id: &str) -> Result<Vec<EmailMessage>> {
    let thread_id = thread_id.trim();
    if thread_id.is_empty() {
        return Err(anyhow::anyhow!("Gmail thread id cannot be empty"));
    }
    let result = crate::tool_bus::invoke_first_enabled_tool(
        store,
        &["gmail.get_thread", "gmail.read_thread"],
        serde_json::json!({"thread_id": thread_id}),
    )?;
    let payload = crate::tool_bus::tool_result_payload(&result.output)?;
    let messages = payload
        .get("messages")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("Gmail thread result omitted messages"))?;
    messages
        .iter()
        .take(100)
        .map(|message| serde_json::from_value(message.clone()).context("invalid Gmail message"))
        .collect()
}

pub fn connected_triage(
    store: &TaskStore,
    router: &ModelRouter,
    task_id: i64,
    query: &str,
) -> Result<Vec<EmailTriageFlagDto>> {
    let messages = connected_messages(store, query)?;
    let active_goal = crate::relational_model::latest_active_goal_text(store, task_id);
    let context = TriageContext {
        known_people: Vec::new(),
        active_goal_terms: active_goal
            .as_deref()
            .unwrap_or("")
            .split(|ch: char| !ch.is_ascii_alphanumeric())
            .filter(|term| term.len() >= 3)
            .map(str::to_string)
            .collect(),
    };
    let fallback = triage_inbox(&messages, &context);
    if !router.tier_available(Tier::Judgment) {
        return Ok(fallback);
    }
    let response = router.generate_with(
        Tier::Judgment,
        "Identify only email that materially affects the active goal, a known deadline, or requires a timely response. Return JSON {\"flags\":[{\"message_id\":\"...\",\"matters_because\":\"...\"}]}. Use only supplied message ids. Do not treat marketing or newsletters as material.",
        &serde_json::json!({"active_goal": active_goal, "messages": messages}).to_string(),
        GenerateOptions {
            temperature: 0.0,
            max_tokens: Some(1_200),
            json_object: true,
            timeout_ms: Some(60_000),
        },
    )?;
    let parsed: serde_json::Value = serde_json::from_str(&response)
        .context("Judgment-tier Gmail triage returned invalid JSON")?;
    let flags = parsed
        .get("flags")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("Judgment-tier Gmail triage omitted flags"))?;
    let by_id = messages
        .iter()
        .map(|message| (message.id.as_str(), message))
        .collect::<std::collections::HashMap<_, _>>();
    flags
        .iter()
        .take(100)
        .map(|flag| {
            let message_id = flag
                .get("message_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("triage flag omitted message_id"))?;
            let message = by_id
                .get(message_id)
                .ok_or_else(|| anyhow::anyhow!("triage fabricated message id"))?;
            let matters_because = flag
                .get("matters_because")
                .and_then(serde_json::Value::as_str)
                .filter(|reason| !reason.trim().is_empty())
                .ok_or_else(|| anyhow::anyhow!("triage flag omitted matters_because"))?;
            Ok(EmailTriageFlagDto {
                message_id: message_id.to_string(),
                subject: message.subject.clone(),
                matters_because: matters_because.chars().take(500).collect(),
            })
        })
        .collect()
}

pub fn register_connected_reply_watch(
    store: &TaskStore,
    task_id: i64,
    message_id: &str,
) -> Result<EmailReplyWatchDto> {
    let messages = connected_messages(store, &format!("id:{message_id}"))?;
    let message = messages
        .iter()
        .find(|message| message.id == message_id)
        .ok_or_else(|| anyhow::anyhow!("Gmail did not return the requested message id"))?;
    register_reply_watch(store, task_id, &message.sender, &message.thread_id)
}

pub fn resolve_connected_reply_watches(store: &TaskStore) -> Result<Vec<i64>> {
    let messages = connected_messages(store, "newer_than:1d")?;
    let mut resolved = Vec::new();
    for message in messages {
        resolved.extend(resolve_landed_reply(store, &message)?);
    }
    resolved.sort_unstable();
    resolved.dedup();
    Ok(resolved)
}

// ---- reply watches (AwaitedReplyLanded) ----------------------------------

pub fn register_reply_watch(
    store: &TaskStore,
    task_id: i64,
    sender: &str,
    thread_hint: &str,
) -> Result<EmailReplyWatchDto> {
    let sender = sender.trim();
    let sender = normalized_sender_identity(sender)
        .ok_or_else(|| anyhow::anyhow!("reply watch requires an exact sender email address"))?;
    let conn = store.connect()?;
    conn.execute(
        "INSERT INTO email_reply_watches (task_id, sender, thread_hint, status)
         VALUES (?1, ?2, ?3, ?4)",
        params![task_id, sender, thread_hint.trim(), WATCH_STATUS_WATCHING],
    )
    .context("failed to register reply watch")?;
    let id = conn.last_insert_rowid();
    drop(conn);
    get_reply_watch(store, id)?.ok_or_else(|| anyhow::anyhow!("reply watch missing after insert"))
}

pub fn list_reply_watches(store: &TaskStore) -> Result<Vec<EmailReplyWatchDto>> {
    let conn = store.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, task_id, sender, thread_hint, status, created_at
         FROM email_reply_watches ORDER BY id DESC",
    )?;
    let rows = stmt
        .query_map([], watch_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn message_matches_watch(message: &EmailMessage, watch: &EmailReplyWatchDto) -> bool {
    let matches_sender = normalized_sender_identity(&message.sender)
        .map(|sender| sender == watch.sender.to_ascii_lowercase())
        .unwrap_or(false);
    let matches_thread = watch.thread_hint.is_empty()
        || message.thread_id.eq_ignore_ascii_case(&watch.thread_hint)
        || message
            .subject
            .to_ascii_lowercase()
            .contains(&watch.thread_hint.to_ascii_lowercase());
    matches_sender && matches_thread
}

fn normalized_sender_identity(sender: &str) -> Option<String> {
    let clean = sender.trim().to_ascii_lowercase();
    let candidate = clean
        .split_once('<')
        .and_then(|(_, rest)| rest.split_once('>').map(|(email, _)| email.trim()))
        .unwrap_or(clean.as_str());
    let (local, domain) = candidate.split_once('@')?;
    if local.is_empty()
        || domain.is_empty()
        || domain.starts_with('.')
        || domain.ends_with('.')
        || !domain.contains('.')
        || !candidate
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "._%+-@".contains(ch))
    {
        return None;
    }
    Some(candidate.to_string())
}

// returns the ids of watches a landed message resolves (for firing the crisis).
pub fn resolve_landed_reply(store: &TaskStore, message: &EmailMessage) -> Result<Vec<i64>> {
    let watches = list_reply_watches(store)?;
    let mut resolved = Vec::new();
    for watch in watches.iter().filter(|w| w.status == WATCH_STATUS_WATCHING) {
        if message_matches_watch(message, watch) {
            let conn = store.connect()?;
            conn.execute(
                "UPDATE email_reply_watches
                 SET status = ?1, resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
                 WHERE id = ?2",
                params![WATCH_STATUS_LANDED, watch.id],
            )?;
            resolved.push(watch.id);
        }
    }
    Ok(resolved)
}

// ---- draft through the action bus ----------------------------------------

// create a reply draft. Recorded as an email.draft action receipt at L1; the
// actual Gmail draft creation is env-gated. email.send stays hard-capped at L1
// (trust) so Jeff can never send.
pub fn draft_reply(
    store: &TaskStore,
    task_id: i64,
    thread_hint: &str,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<ActionReceiptDto> {
    let action_class = crate::action_bus::ActionClass::EmailDraft;
    let class = action_class.as_str();
    // drafts are propose-only; never above L1.
    crate::trust::assert_runtime_level_allowed(&class, crate::trust::TRUST_LEVEL_L1)?;
    let payload = serde_json::json!({
        "to": to,
        "subject": subject,
        "thread_hint": thread_hint,
        "body_excerpt": body.chars().take(200).collect::<String>(),
    });
    let receipt = crate::action_bus::ActionBus::dispatch_proposal(
        store,
        &crate::action_bus::ActionRequest {
            task_id,
            class: action_class,
            surface: "gmail".to_string(),
            description: format!("Draft reply to {to}"),
            payload,
            reversibility: crate::action_bus::Reversibility::Guided,
        },
    )?;
    if let Err(error) = crate::tool_bus::persist_connected_action(
        store,
        receipt.id,
        task_id,
        &["gmail.create_draft", "gmail.draft_reply"],
        serde_json::json!({
            "thread_id": thread_hint,
            "to": to,
            "subject": subject,
            "body": body,
        }),
    ) {
        let _ = store.update_action_receipt_status(
            receipt.id,
            "failed",
            Some("failed to persist exact Gmail draft proposal"),
            None,
        );
        return Err(error);
    }
    Ok(receipt)
}

pub fn propose_message_label(
    store: &TaskStore,
    task_id: i64,
    message_id: &str,
    add_labels: &[String],
    remove_labels: &[String],
) -> Result<ActionReceiptDto> {
    let message_id = message_id.trim();
    if message_id.is_empty() || (add_labels.is_empty() && remove_labels.is_empty()) {
        return Err(anyhow::anyhow!(
            "Gmail label proposal requires a message and at least one label change"
        ));
    }
    let receipt = crate::action_bus::ActionBus::dispatch_proposal(
        store,
        &crate::action_bus::ActionRequest {
            task_id,
            class: crate::action_bus::ActionClass::EmailLabel,
            surface: "gmail".to_string(),
            description: format!("Change labels on Gmail message {message_id}"),
            payload: serde_json::json!({
                "message_id": message_id,
                "add_labels": add_labels,
                "remove_labels": remove_labels,
            }),
            reversibility: crate::action_bus::Reversibility::Reversible,
        },
    )?;
    crate::tool_bus::persist_connected_action(
        store,
        receipt.id,
        task_id,
        &["gmail.modify_labels", "gmail.label_message"],
        serde_json::json!({
            "message_id": message_id,
            "add_labels": add_labels,
            "remove_labels": remove_labels,
        }),
    )?;
    Ok(receipt)
}

fn get_reply_watch(store: &TaskStore, id: i64) -> Result<Option<EmailReplyWatchDto>> {
    let conn = store.connect()?;
    conn.query_row(
        "SELECT id, task_id, sender, thread_hint, status, created_at
         FROM email_reply_watches WHERE id = ?1",
        params![id],
        watch_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn watch_from_row(row: &Row<'_>) -> rusqlite::Result<EmailReplyWatchDto> {
    Ok(EmailReplyWatchDto {
        id: row.get(0)?,
        task_id: row.get(1)?,
        sender: row.get(2)?,
        thread_hint: row.get(3)?,
        status: row.get(4)?,
        created_at: row.get(5)?,
    })
}

use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_store() -> (TempDir, TaskStore, i64) {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        let task = store.create_task("gmail").unwrap();
        (dir, store, task.id)
    }

    fn msg(id: &str, sender: &str, subject: &str, snippet: &str) -> EmailMessage {
        EmailMessage {
            id: id.to_string(),
            thread_id: format!("t-{id}"),
            sender: sender.to_string(),
            subject: subject.to_string(),
            snippet: snippet.to_string(),
        }
    }

    fn context() -> TriageContext {
        TriageContext {
            known_people: vec!["Sarah".to_string(), "advisor".to_string()],
            active_goal_terms: vec!["thesis".to_string(), "chapter".to_string()],
        }
    }

    #[test]
    fn e3_triage_flags_matters_and_skips_noise() {
        let ctx = context();
        let matters = msg(
            "1",
            "Sarah Lee",
            "Draft review deadline",
            "The review is due Friday.",
        );
        let noise = msg(
            "2",
            "no-reply@newsletter.com",
            "Weekly digest",
            "Unsubscribe anytime.",
        );
        assert!(message_matters(&matters, &ctx));
        assert!(!message_matters(&noise, &ctx));
        let flags = triage_inbox(&[matters, noise], &ctx);
        assert_eq!(flags.len(), 1);
        assert_eq!(flags[0].message_id, "1");
        assert!(!flags[0].matters_because.is_empty());
    }

    #[test]
    fn e3_thread_summarizes_on_demand() {
        let thread = vec![
            msg("1", "Sarah", "Q", "Can you send the draft. Thanks."),
            msg("2", "You", "Re: Q", "Yes by Friday. Working on it."),
        ];
        let summary = summarize_thread(&thread);
        assert!(summary.contains("Sarah: Can you send the draft"));
        assert!(summary.contains("You: Yes by Friday"));
    }

    #[test]
    fn e3_reply_watch_registers_and_landed_reply_resolves() {
        let (_dir, store, task_id) = test_store();
        register_reply_watch(&store, task_id, "sarah@example.com", "").unwrap();
        let unrelated = msg("1", "Bob", "hi", "hello");
        assert!(resolve_landed_reply(&store, &unrelated).unwrap().is_empty());
        let landed = msg(
            "2",
            "Sarah Lee <sarah@example.com>",
            "Re: draft",
            "here it is",
        );
        let resolved = resolve_landed_reply(&store, &landed).unwrap();
        assert_eq!(resolved.len(), 1);
        // the watch is marked landed and does not fire twice.
        assert!(resolve_landed_reply(&store, &landed).unwrap().is_empty());
        assert!(list_reply_watches(&store)
            .unwrap()
            .iter()
            .any(|w| w.status == WATCH_STATUS_LANDED));
    }

    #[test]
    fn e3_reply_watch_rejects_name_only_and_sender_substring_spoofing() {
        let (_dir, store, task_id) = test_store();
        assert!(register_reply_watch(&store, task_id, "Sarah", "").is_err());
        let watch = register_reply_watch(&store, task_id, "sarah@example.com", "").unwrap();
        let spoof = msg(
            "spoof",
            "sarah@example.com.attacker.test",
            "Re: draft",
            "fake",
        );
        assert!(!message_matches_watch(&spoof, &watch));
    }

    #[test]
    fn e3_draft_reply_is_email_draft_receipt_never_send() {
        let (_dir, store, task_id) = test_store();
        let connection = crate::tool_bus::add_tool_connection(
            &store,
            "gmail-fixture",
            crate::tool_bus::TRANSPORT_LOOPBACK,
            "loopback://",
            &[],
        )
        .unwrap();
        crate::tool_bus::register_connection_tools(
            &store,
            connection.id,
            &[(
                "gmail.create_draft".to_string(),
                "create a Gmail draft".to_string(),
            )],
        )
        .unwrap();
        let receipt = draft_reply(
            &store,
            task_id,
            "",
            "sarah@x.com",
            "Re: draft",
            "Slips to Thursday.",
        )
        .unwrap();
        assert_eq!(receipt.class, "email.draft");
        assert_eq!(receipt.level, "L1");
        assert_eq!(receipt.status, "pending_approval");
        let applied = crate::tool_bus::approve_connected_action(&store, receipt.id).unwrap();
        assert_eq!(applied.status, "applied");
        assert!(crate::tool_bus::approve_connected_action(&store, receipt.id).is_err());
        // email.send can never be raised above L1.
        assert!(crate::trust::assert_runtime_level_allowed("email.send", "L2").is_err());
    }

    #[test]
    fn e3_connected_gmail_registers_and_resolves_watch_from_trusted_messages() {
        let (_dir, store, task_id) = test_store();
        let server = r#"import json,sys
for line in sys.stdin:
 m=json.loads(line)
 if m.get('method')=='initialize': result={'protocolVersion':'2025-03-26','capabilities':{},'serverInfo':{'name':'gmail-fixture','version':'1'}}
 elif m.get('method')=='tools/list': result={'tools':[{'name':'gmail.search_messages','description':'search','inputSchema':{'type':'object'}}]}
 elif m.get('method')=='tools/call': result={'structuredContent':{'messages':[{'id':'message-1','thread_id':'thread-1','sender':'Sarah <sarah@example.com>','subject':'Re: draft','snippet':'Here it is.'}]}}
 else: continue
 print(json.dumps({'jsonrpc':'2.0','id':m['id'],'result':result}),flush=True)"#;
        let endpoint =
            serde_json::to_string(&vec!["/usr/bin/python3", "-u", "-c", server]).unwrap();
        let connection = crate::tool_bus::add_tool_connection(
            &store,
            "gmail-read-fixture",
            crate::tool_bus::TRANSPORT_STDIO,
            &endpoint,
            &[],
        )
        .unwrap();
        crate::tool_bus::discover_connection_tools(&store, connection.id).unwrap();
        let watch = register_connected_reply_watch(&store, task_id, "message-1").unwrap();
        assert_eq!(watch.sender, "sarah@example.com");
        assert_eq!(watch.thread_hint, "thread-1");
        assert_eq!(
            resolve_connected_reply_watches(&store).unwrap(),
            vec![watch.id]
        );
    }
}
