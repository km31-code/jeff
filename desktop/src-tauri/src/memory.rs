// apex b3: typed episodic memory.
//
// episodes are the durable record of meaningful work moments. writers are
// designed to run from background tasks or after an action has already
// completed, so embedding and tagging work never sits on the response path.

use std::{sync::Arc, thread};

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, OptionalExtension, Row};
use serde::Deserialize;

use crate::{
    embedding::EmbeddingProvider,
    model_router::{GenerateOptions, ModelRequest, ModelRouter, Tier},
    models::{ChatMessageDto, EpisodeDto, EpisodeSearchResultDto},
    similarity::cosine_similarity,
    store::TaskStore,
};

pub const KIND_SESSION_SUMMARY: &str = "session_summary";
pub const KIND_DECISION: &str = "decision";
pub const KIND_PROPOSAL_OUTCOME: &str = "proposal_outcome";
pub const KIND_WORK_UNDERSTANDING: &str = "work_understanding";
pub const KIND_DEADLINE_MENTION: &str = "deadline_mention";
pub const KIND_USER_FACT: &str = "user_fact";

const MAX_EPISODE_TEXT_CHARS: usize = 900;
const MAX_SESSION_SUMMARY_WORDS: usize = 120;
const MEMORY_TAG_TIMEOUT_MS: u64 = 4000;
const SESSION_SUMMARY_TIMEOUT_MS: u64 = 8000;
const SESSION_SUMMARY_KEY_PREFIX: &str = "memory:last_session_summary_message_id:";

pub const MEMORY_TAG_SYSTEM_PROMPT: &str = "Extract durable memory episodes from a short chat transcript. \
Return JSON only: {\"episodes\":[{\"kind\":\"decision|deadline_mention|user_fact\",\"text\":\"short durable fact\",\"salience\":0.0-1.0,\"evidence_quote\":\"verbatim user quote\"}]}. \
Only include explicit user decisions, deadline/date mentions, and durable user/work facts. \
Do not include chit-chat, thanks, compliments, jokes, or generic requests.";

pub const SESSION_SUMMARY_SYSTEM_PROMPT: &str = "Summarize this work session for future memory. \
Write at most 120 words. Capture concrete decisions, progress, blockers, deadlines, and user preferences. \
Do not flatter, do not narrate the summarization, and do not invent details.";

#[derive(Debug, Clone)]
pub struct NewEpisode {
    pub task_id: i64,
    pub kind: String,
    pub text: String,
    pub salience: f32,
    pub source: String,
}

impl NewEpisode {
    pub fn new(task_id: i64, kind: &str, text: impl Into<String>, source: &str) -> Self {
        Self {
            task_id,
            kind: kind.to_string(),
            text: text.into(),
            salience: default_salience(kind),
            source: source.to_string(),
        }
    }

    pub fn with_salience(mut self, salience: f32) -> Self {
        self.salience = salience;
        self
    }
}

pub fn record_episode(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    input: &NewEpisode,
) -> Result<EpisodeDto> {
    validate_kind(&input.kind)?;
    let text = clean_episode_text(&input.text);
    if text.is_empty() {
        return Err(anyhow!("episode text cannot be empty"));
    }

    if let Some(existing) =
        find_duplicate_episode(store, input.task_id, &input.kind, &text, &input.source)?
    {
        return Ok(existing);
    }

    let embedding = embeddings
        .embed_text(&text)
        .context("failed to embed episode text")?;
    let embedding_blob = encode_embedding(&embedding);
    let conn = store.connect()?;
    conn.execute(
        "INSERT INTO episodes
         (task_id, kind, text, embedding, embedding_model, salience, source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            input.task_id,
            input.kind.trim(),
            text,
            embedding_blob,
            embeddings.model_id(),
            input.salience.clamp(0.0, 1.0),
            input.source.trim()
        ],
    )
    .context("failed to insert episode")?;
    let id = conn.last_insert_rowid();
    get_episode(store, id)?.ok_or_else(|| anyhow!("episode id={id} missing after insert"))
}

pub fn record_episode_async(
    store: TaskStore,
    embeddings: Arc<dyn EmbeddingProvider>,
    input: NewEpisode,
) {
    thread::spawn(move || {
        if let Err(err) = record_episode(&store, embeddings.as_ref(), &input) {
            eprintln!(
                "[jeff] episode_write_failed kind={} source={} error={err}",
                input.kind, input.source
            );
        }
    });
}

pub fn list_episodes(store: &TaskStore, task_id: i64, limit: usize) -> Result<Vec<EpisodeDto>> {
    let conn = store.connect()?;
    let mut stmt = conn
        .prepare(
            "SELECT id, task_id, kind, text, salience, source, created_at, consolidated_at
             FROM episodes
             WHERE task_id = ?1
             ORDER BY id DESC
             LIMIT ?2",
        )
        .context("failed to prepare episode list query")?;
    let rows = stmt
        .query_map(params![task_id, limit as i64], episode_from_row)
        .context("failed to query episodes")?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to collect episodes")
}

pub fn search_episodes(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    task_id: i64,
    query: &str,
    limit: usize,
) -> Result<Vec<EpisodeSearchResultDto>> {
    let query_embedding = embeddings.embed_text(query.trim())?;
    let candidates = list_episode_embeddings(store, task_id, 500)?;
    let mut scored = candidates
        .into_iter()
        .map(|(episode, embedding)| EpisodeSearchResultDto {
            similarity_score: cosine_similarity(&query_embedding, &embedding),
            episode,
        })
        .collect::<Vec<_>>();
    scored.sort_by(|a, b| {
        b.similarity_score
            .partial_cmp(&a.similarity_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(limit);
    Ok(scored)
}

pub fn record_proposal_outcome_async(
    store: TaskStore,
    embeddings: Arc<dyn EmbeddingProvider>,
    task_id: i64,
    source: &str,
    text: impl Into<String>,
) {
    record_episode_async(
        store,
        embeddings,
        NewEpisode::new(task_id, KIND_PROPOSAL_OUTCOME, text.into(), source).with_salience(0.75),
    );
}

pub fn clear_all_episodes(store: &TaskStore) -> Result<()> {
    let conn = store.connect()?;
    conn.execute("DELETE FROM episodes", [])
        .context("failed to clear episodes")?;
    Ok(())
}

pub fn extract_memory_tags_with_fallback(
    router: &ModelRouter,
    transcript: &[ChatMessageDto],
) -> Vec<NewEpisode> {
    match extract_memory_tags(router, transcript) {
        Ok(tags) => tags,
        Err(err) => {
            eprintln!("[jeff] memory_tag_fallback reason={err}");
            heuristic_memory_tags(transcript)
        }
    }
}

pub fn record_memory_tags_for_turn(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    task_id: i64,
    tags: &[NewEpisode],
) -> Result<usize> {
    let mut written = 0usize;
    for tag in tags {
        let mut input = tag.clone();
        input.task_id = task_id;
        if record_episode(store, embeddings, &input).is_ok() {
            written += 1;
        }
    }
    Ok(written)
}

pub fn record_idle_session_summary_if_due(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    router: &ModelRouter,
    task_id: i64,
    min_idle_seconds: i64,
) -> Result<Option<EpisodeDto>> {
    let messages = store.list_recent_chat_messages(task_id, 40)?;
    let Some(last) = messages.last() else {
        return Ok(None);
    };
    let Some(last_at) = parse_sqlite_datetime_to_unix(&last.created_at) else {
        return Ok(None);
    };
    let now = chrono::Utc::now().timestamp();
    if now.saturating_sub(last_at) < min_idle_seconds.max(0) {
        return Ok(None);
    }

    let key = format!("{SESSION_SUMMARY_KEY_PREFIX}{task_id}");
    if store
        .get_app_setting(&key)?
        .and_then(|raw| raw.parse::<i64>().ok())
        == Some(last.id)
    {
        return Ok(None);
    }

    let summary = summarize_session_with_fallback(router, &messages);
    if summary.trim().is_empty() {
        return Ok(None);
    }
    let episode = record_episode(
        store,
        embeddings,
        &NewEpisode::new(task_id, KIND_SESSION_SUMMARY, summary, "session_idle")
            .with_salience(0.72),
    )?;
    store.set_app_setting(&key, &last.id.to_string())?;
    Ok(Some(episode))
}

fn extract_memory_tags(
    router: &ModelRouter,
    transcript: &[ChatMessageDto],
) -> Result<Vec<NewEpisode>> {
    let user = build_transcript_prompt(transcript);
    if user.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut request = ModelRequest::new(Tier::Reflex, MEMORY_TAG_SYSTEM_PROMPT, user).with_options(
        GenerateOptions {
            temperature: 0.0,
            max_tokens: Some(350),
            json_object: true,
            timeout_ms: Some(MEMORY_TAG_TIMEOUT_MS),
        },
    );
    request.purpose = Some("memory_tagging".to_string());
    let raw = router.route(request)?.text;
    parse_memory_tag_json(&raw, 0)
}

fn summarize_session_with_fallback(router: &ModelRouter, messages: &[ChatMessageDto]) -> String {
    let prompt = build_transcript_prompt(messages);
    if prompt.trim().is_empty() {
        return String::new();
    }
    let mut request = ModelRequest::new(Tier::Craft, SESSION_SUMMARY_SYSTEM_PROMPT, prompt)
        .with_options(GenerateOptions {
            temperature: 0.1,
            max_tokens: Some(220),
            json_object: false,
            timeout_ms: Some(SESSION_SUMMARY_TIMEOUT_MS),
        });
    request.purpose = Some("session_summary".to_string());
    match router.route(request) {
        Ok(response) => truncate_words(response.text.trim(), MAX_SESSION_SUMMARY_WORDS),
        Err(err) => {
            eprintln!("[jeff] session_summary_fallback reason={err}");
            deterministic_session_summary(messages)
        }
    }
}

fn deterministic_session_summary(messages: &[ChatMessageDto]) -> String {
    let user_turns = messages
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content.trim())
        .filter(|text| !text.is_empty())
        .take(6)
        .collect::<Vec<_>>();
    if user_turns.is_empty() {
        return String::new();
    }
    truncate_words(
        format!("Session covered: {}.", user_turns.join(" / ")),
        MAX_SESSION_SUMMARY_WORDS,
    )
}

fn heuristic_memory_tags(transcript: &[ChatMessageDto]) -> Vec<NewEpisode> {
    let mut tags = Vec::new();
    for message in transcript.iter().rev().filter(|m| m.role == "user").take(6) {
        let text = message.content.trim();
        let lower = text.to_ascii_lowercase();
        if let Some(decision) = decision_from_text(text, &lower) {
            tags.push(
                NewEpisode::new(0, KIND_DECISION, decision, "chat_decision").with_salience(0.76),
            );
        }
        if let Some(deadline) = deadline_from_text(text, &lower) {
            tags.push(
                NewEpisode::new(0, KIND_DEADLINE_MENTION, deadline, "chat_deadline")
                    .with_salience(0.82),
            );
        }
        if let Some(fact) = user_fact_from_text(text, &lower) {
            tags.push(
                NewEpisode::new(0, KIND_USER_FACT, fact, "chat_user_fact").with_salience(0.7),
            );
        }
    }
    dedupe_new_episodes(tags)
}

fn decision_from_text(text: &str, lower: &str) -> Option<String> {
    for pattern in [
        "i decided to ",
        "i've decided to ",
        "i have decided to ",
        "let's ",
        "we should ",
        "i'll go with ",
        "i will go with ",
        "go with ",
    ] {
        if let Some(tail) = capture_after(text, lower, pattern) {
            return Some(format!("Decision: {tail}"));
        }
    }
    None
}

fn deadline_from_text(text: &str, lower: &str) -> Option<String> {
    let has_deadline = lower.contains("deadline")
        || lower.contains("due ")
        || lower.contains(" by ")
        || lower.contains("tonight")
        || lower.contains("tomorrow")
        || [
            "monday",
            "tuesday",
            "wednesday",
            "thursday",
            "friday",
            "saturday",
            "sunday",
        ]
        .iter()
        .any(|day| lower.contains(day));
    has_deadline.then(|| format!("Deadline/date mentioned: {}", clean_episode_text(text)))
}

fn user_fact_from_text(text: &str, lower: &str) -> Option<String> {
    for pattern in [
        "my advisor ",
        "my boss ",
        "my client ",
        "i prefer ",
        "i usually ",
        "i always ",
        "i hate ",
        "i care about ",
    ] {
        if lower.contains(pattern) {
            return Some(format!("User fact: {}", clean_episode_text(text)));
        }
    }
    None
}

fn parse_memory_tag_json(raw: &str, task_id: i64) -> Result<Vec<NewEpisode>> {
    let json =
        extract_json_object(raw).ok_or_else(|| anyhow!("memory tag response had no json"))?;
    #[derive(Deserialize)]
    struct Payload {
        #[serde(default)]
        episodes: Vec<RawEpisode>,
    }
    #[derive(Deserialize)]
    struct RawEpisode {
        kind: String,
        text: String,
        #[serde(default = "default_raw_salience")]
        salience: f32,
    }
    fn default_raw_salience() -> f32 {
        0.6
    }

    let parsed: Payload = serde_json::from_str(json).context("failed to parse memory tag json")?;
    let mut episodes = Vec::new();
    for raw in parsed.episodes {
        validate_kind(&raw.kind)?;
        if matches!(
            raw.kind.as_str(),
            KIND_DECISION | KIND_DEADLINE_MENTION | KIND_USER_FACT
        ) {
            let text = clean_episode_text(&raw.text);
            if !text.is_empty() {
                episodes.push(
                    NewEpisode::new(task_id, &raw.kind, text, "memory_tagger")
                        .with_salience(raw.salience),
                );
            }
        }
    }
    Ok(dedupe_new_episodes(episodes))
}

fn find_duplicate_episode(
    store: &TaskStore,
    task_id: i64,
    kind: &str,
    text: &str,
    source: &str,
) -> Result<Option<EpisodeDto>> {
    let conn = store.connect()?;
    conn.query_row(
        "SELECT id, task_id, kind, text, salience, source, created_at, consolidated_at
         FROM episodes
         WHERE task_id = ?1 AND kind = ?2 AND text = ?3 AND source = ?4
         ORDER BY id DESC
         LIMIT 1",
        params![task_id, kind, text, source],
        episode_from_row,
    )
    .optional()
    .context("failed to query duplicate episode")
}

pub fn get_episode(store: &TaskStore, id: i64) -> Result<Option<EpisodeDto>> {
    let conn = store.connect()?;
    conn.query_row(
        "SELECT id, task_id, kind, text, salience, source, created_at, consolidated_at
         FROM episodes
         WHERE id = ?1",
        params![id],
        episode_from_row,
    )
    .optional()
    .context("failed to query episode by id")
}

fn list_episode_embeddings(
    store: &TaskStore,
    task_id: i64,
    limit: usize,
) -> Result<Vec<(EpisodeDto, Vec<f32>)>> {
    let conn = store.connect()?;
    let mut stmt = conn
        .prepare(
            "SELECT id, task_id, kind, text, salience, source, created_at, consolidated_at, embedding
             FROM episodes
             WHERE task_id = ?1
             ORDER BY id DESC
             LIMIT ?2",
        )
        .context("failed to prepare episode embedding query")?;
    let rows = stmt
        .query_map(params![task_id, limit as i64], |row| {
            let episode = EpisodeDto {
                id: row.get(0)?,
                task_id: row.get(1)?,
                kind: row.get(2)?,
                text: row.get(3)?,
                salience: row.get::<_, f64>(4)? as f32,
                source: row.get(5)?,
                created_at: row.get(6)?,
                consolidated_at: row.get(7)?,
            };
            let blob: Vec<u8> = row.get(8)?;
            Ok((episode, decode_embedding(&blob)))
        })
        .context("failed to query episode embeddings")?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to collect episode embeddings")
}

fn episode_from_row(row: &Row<'_>) -> rusqlite::Result<EpisodeDto> {
    Ok(EpisodeDto {
        id: row.get(0)?,
        task_id: row.get(1)?,
        kind: row.get(2)?,
        text: row.get(3)?,
        salience: row.get::<_, f64>(4)? as f32,
        source: row.get(5)?,
        created_at: row.get(6)?,
        consolidated_at: row.get(7)?,
    })
}

pub fn encode_embedding(embedding: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(embedding.len() * 4);
    for value in embedding {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

pub fn decode_embedding(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn build_transcript_prompt(transcript: &[ChatMessageDto]) -> String {
    transcript
        .iter()
        .rev()
        .take(10)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .filter(|m| !m.content.trim().is_empty())
        .map(|m| format!("{}: {}", m.role, m.content.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn capture_after(original: &str, lower: &str, pattern: &str) -> Option<String> {
    let index = lower.find(pattern)?;
    let start = index + pattern.len();
    let tail = original[start..]
        .trim()
        .trim_start_matches([':', '-', ' '])
        .trim_end_matches(['.', '!', '?'])
        .trim();
    (tail.split_whitespace().count() >= 2).then(|| clean_episode_text(tail))
}

fn clean_episode_text(text: &str) -> String {
    let trimmed = text.trim();
    let clean = if trimmed.chars().count() > MAX_EPISODE_TEXT_CHARS {
        trimmed.chars().take(MAX_EPISODE_TEXT_CHARS).collect()
    } else {
        trimmed.to_string()
    };
    clean.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_words(text: impl Into<String>, max_words: usize) -> String {
    let text = text.into();
    let words = text.split_whitespace().take(max_words).collect::<Vec<_>>();
    words.join(" ")
}

fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    (end > start).then_some(&raw[start..=end])
}

fn parse_sqlite_datetime_to_unix(dt: &str) -> Option<i64> {
    let normalized = dt.trim().replace('T', " ");
    let date_time: Vec<&str> = normalized.splitn(2, ' ').collect();
    if date_time.len() != 2 {
        return None;
    }
    let date_parts: Vec<i64> = date_time[0]
        .split('-')
        .filter_map(|part| part.parse().ok())
        .collect();
    let time_only = date_time[1]
        .split('.')
        .next()
        .unwrap_or("00:00:00")
        .trim_end_matches('Z');
    let time_parts: Vec<i64> = time_only
        .split(':')
        .filter_map(|part| part.parse().ok())
        .collect();
    if date_parts.len() < 3 || time_parts.len() < 3 {
        return None;
    }

    let (year, month, day) = (date_parts[0], date_parts[1], date_parts[2]);
    let (hour, minute, second) = (time_parts[0], time_parts[1], time_parts[2]);
    if !(1..=12).contains(&month) || day < 1 || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let leap = |y: i64| -> bool { (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 };
    let mut total_days = 0_i64;
    for y in 1970..year {
        total_days += if leap(y) { 366 } else { 365 };
    }
    let days_per_month = [31_i64, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 0..(month as usize - 1) {
        total_days += days_per_month[m];
        if m == 1 && leap(year) {
            total_days += 1;
        }
    }
    let max_day =
        days_per_month[(month - 1) as usize] + if month == 2 && leap(year) { 1 } else { 0 };
    if day > max_day {
        return None;
    }
    total_days += day - 1;
    Some(total_days * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn validate_kind(kind: &str) -> Result<()> {
    if matches!(
        kind,
        KIND_SESSION_SUMMARY
            | KIND_DECISION
            | KIND_PROPOSAL_OUTCOME
            | KIND_WORK_UNDERSTANDING
            | KIND_DEADLINE_MENTION
            | KIND_USER_FACT
    ) {
        Ok(())
    } else {
        Err(anyhow!("unsupported episode kind '{kind}'"))
    }
}

fn default_salience(kind: &str) -> f32 {
    match kind {
        KIND_DEADLINE_MENTION => 0.82,
        KIND_DECISION | KIND_PROPOSAL_OUTCOME => 0.75,
        KIND_USER_FACT => 0.70,
        KIND_SESSION_SUMMARY | KIND_WORK_UNDERSTANDING => 0.65,
        _ => 0.5,
    }
}

fn dedupe_new_episodes(mut episodes: Vec<NewEpisode>) -> Vec<NewEpisode> {
    let mut seen = std::collections::HashSet::new();
    episodes.retain(|episode| {
        seen.insert((
            episode.kind.clone(),
            episode.text.to_ascii_lowercase(),
            episode.source.clone(),
        ))
    });
    episodes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        message_kind::MessageKind,
        model_router::{ProviderKind, RouterConfig, TierConfig},
        providers::local::hash_embedding,
        store::TaskStore,
    };
    use tempfile::TempDir;

    struct TestEmbeddingProvider;

    impl EmbeddingProvider for TestEmbeddingProvider {
        fn embed_text(&self, input: &str) -> Result<Vec<f32>> {
            Ok(hash_embedding(input))
        }

        fn model_id(&self) -> &'static str {
            "test-hash"
        }
    }

    fn test_store() -> (TempDir, TaskStore, i64) {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        let task = store.create_task("Memory Test").unwrap();
        (dir, store, task.id)
    }

    fn msg(role: &str, text: &str) -> ChatMessageDto {
        ChatMessageDto {
            id: 1,
            task_id: 1,
            session_id: None,
            role: role.to_string(),
            message_source: "text".to_string(),
            message_kind: "chat".to_string(),
            content: text.to_string(),
            created_at: "2026-01-01T00:00:00.000Z".to_string(),
        }
    }

    #[test]
    fn b3_episode_round_trips_with_embedding_blob() {
        let (_dir, store, task_id) = test_store();
        let provider = TestEmbeddingProvider;
        let episode = record_episode(
            &store,
            &provider,
            &NewEpisode::new(
                task_id,
                KIND_DECISION,
                "Decision: cut the second example",
                "test",
            ),
        )
        .unwrap();
        assert_eq!(episode.kind, KIND_DECISION);
        assert_eq!(list_episodes(&store, task_id, 10).unwrap().len(), 1);
    }

    #[test]
    fn b3_episode_search_uses_cosine_candidates() {
        let (_dir, store, task_id) = test_store();
        let provider = TestEmbeddingProvider;
        record_episode(
            &store,
            &provider,
            &NewEpisode::new(
                task_id,
                KIND_USER_FACT,
                "User fact: advisor hates passive voice",
                "test",
            ),
        )
        .unwrap();
        record_episode(
            &store,
            &provider,
            &NewEpisode::new(
                task_id,
                KIND_DEADLINE_MENTION,
                "Deadline/date mentioned: slides due Friday",
                "test",
            ),
        )
        .unwrap();
        let results =
            search_episodes(&store, &provider, task_id, "passive voice advisor", 1).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn b3_heuristic_tags_decisions_deadlines_and_user_facts() {
        let tags = heuristic_memory_tags(&[
            msg(
                "user",
                "Let's cut the weak intro and the abstract is due Friday.",
            ),
            msg("user", "My advisor hates passive voice."),
        ]);
        assert!(tags.iter().any(|tag| tag.kind == KIND_DECISION));
        assert!(tags.iter().any(|tag| tag.kind == KIND_DEADLINE_MENTION));
        assert!(tags.iter().any(|tag| tag.kind == KIND_USER_FACT));
    }

    #[test]
    fn b3_embedding_blob_round_trips() {
        let values = vec![0.1, -2.0, 3.5];
        assert_eq!(decode_embedding(&encode_embedding(&values)), values);
    }

    #[test]
    fn b3_scripted_working_session_records_typed_episodes() {
        let (_dir, store, task_id) = test_store();
        let provider = TestEmbeddingProvider;
        let router = ModelRouter::new(RouterConfig {
            reflex: TierConfig {
                provider: ProviderKind::Local,
                model: "test-local".to_string(),
            },
            conversation: TierConfig {
                provider: ProviderKind::Local,
                model: "test-local".to_string(),
            },
            judgment: TierConfig {
                provider: ProviderKind::Local,
                model: "test-local".to_string(),
            },
            craft: TierConfig {
                provider: ProviderKind::Local,
                model: "test-local".to_string(),
            },
        });

        for text in [
            "Let's tighten the analysis around the sponsor risk.",
            "My advisor hates passive voice.",
            "The revised draft is due Friday.",
            "I decided to keep the shorter conclusion.",
            "Please make the accepted revision concise.",
        ] {
            store
                .append_chat_message(task_id, "user", "text", MessageKind::UserStatement, text)
                .unwrap();
        }

        let transcript = store.list_recent_chat_messages(task_id, 20).unwrap();
        let tags = heuristic_memory_tags(&transcript);
        record_memory_tags_for_turn(&store, &provider, task_id, &tags).unwrap();
        record_episode(
            &store,
            &provider,
            &NewEpisode::new(
                task_id,
                KIND_PROPOSAL_OUTCOME,
                "Accepted revision #42 for artifact #7: shortened the conclusion.",
                "revision:42:accepted",
            ),
        )
        .unwrap();
        record_idle_session_summary_if_due(&store, &provider, &router, task_id, 0)
            .unwrap()
            .unwrap();

        let episodes = list_episodes(&store, task_id, 20).unwrap();
        assert!(episodes.len() >= 3);
        for kind in [
            KIND_SESSION_SUMMARY,
            KIND_DECISION,
            KIND_PROPOSAL_OUTCOME,
            KIND_DEADLINE_MENTION,
            KIND_USER_FACT,
        ] {
            assert!(
                episodes.iter().any(|episode| episode.kind == kind),
                "{kind}"
            );
        }
    }
}
