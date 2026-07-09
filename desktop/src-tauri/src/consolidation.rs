// apex b4: consolidation of episodic memory into durable facts.
//
// the consolidation path is deliberately local-first: craft-tier extraction and
// merge prompts are attempted, but deterministic fallbacks keep privacy gates
// and milestone checks runnable without network access.

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Row};
#[cfg(not(test))]
use serde::Deserialize;

use crate::{
    cost_governor::CONSOLIDATION_BUDGET_KEY,
    embedding::EmbeddingProvider,
    memory,
    model_router::{ModelRouter, ProviderKind},
    models::{ConsolidationReportDto, EpisodeDto, FactDto},
    similarity::cosine_similarity,
    store::{LlmUsageLogInput, TaskStore},
};
#[cfg(not(test))]
use crate::model_router::{GenerateOptions, ModelRequest, Tier};

pub const FACT_KIND_PREFERENCE: &str = "preference";
pub const FACT_KIND_CONSTRAINT: &str = "constraint";
pub const FACT_KIND_DEADLINE: &str = "deadline";
pub const FACT_KIND_PERSON: &str = "person";
pub const FACT_KIND_PATTERN: &str = "pattern";
pub const FACT_KIND_CONTEXT: &str = "context";

const MAX_FACTS: usize = 500;
const MAX_FACT_TEXT_CHARS: usize = 500;
const MERGE_SIMILARITY_THRESHOLD: f32 = 0.85;
#[cfg(not(test))]
const CONSOLIDATION_TIMEOUT_MS: u64 = 10_000;

#[cfg(not(test))]
pub const CONSOLIDATION_SYSTEM_PROMPT: &str = "Consolidate typed memory episodes into durable facts. \
Return JSON only: {\"facts\":[{\"kind\":\"preference|constraint|deadline|person|pattern|context\",\"text\":\"plain language fact\",\"confidence\":0.0-1.0,\"salience\":0.0-1.0,\"evidence_ids\":[1,2]}]}. \
Merge duplicates, preserve evidence ids, do not invent facts, and omit low-value chatter.";

#[cfg(not(test))]
pub const FACT_MERGE_SYSTEM_PROMPT: &str = "Merge two memory facts into one plain-language fact. \
Return only the merged fact text. Preserve the concrete meaning and do not add new details.";

#[derive(Debug, Clone)]
struct EpisodeWithEmbedding {
    episode: EpisodeDto,
}

#[derive(Debug, Clone)]
struct StoredFact {
    dto: FactDto,
    embedding: Vec<f32>,
}

#[derive(Debug, Clone)]
struct FactCandidate {
    kind: String,
    text: String,
    confidence: f32,
    salience: f32,
    evidence_ids: Vec<i64>,
}

impl FactCandidate {
    fn new(kind: &str, text: impl Into<String>, episode: &EpisodeDto) -> Self {
        Self {
            kind: kind.to_string(),
            text: clean_fact_text(&text.into()),
            confidence: confidence_for_episode(episode),
            salience: episode.salience.clamp(0.0, 1.0),
            evidence_ids: vec![episode.id],
        }
    }
}

pub fn run_consolidation(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    router: &ModelRouter,
) -> Result<ConsolidationReportDto> {
    record_consolidation_budget_touch(store)?;
    let (decayed, mut dropped) = apply_decay(store)?;
    let episodes = list_unconsolidated_episode_embeddings(store, 250)?;
    let mut report = ConsolidationReportDto {
        processed_episode_count: episodes.len(),
        upserted_fact_count: 0,
        merged_fact_count: 0,
        decayed_fact_count: decayed,
        dropped_fact_count: dropped,
        marked_episode_count: 0,
    };

    if episodes.is_empty() {
        report.dropped_fact_count += enforce_fact_cap(store)?;
        return Ok(report);
    }

    let candidates = extract_fact_candidates_with_fallback(router, &episodes);
    for candidate in candidates {
        validate_fact_kind(&candidate.kind)?;
        if candidate.text.trim().is_empty() || candidate.evidence_ids.is_empty() {
            continue;
        }
        let embedding = embeddings
            .embed_text(&candidate.text)
            .context("failed to embed fact candidate")?;
        if let Some(existing) = best_fact_match(store, &candidate.kind, &embedding)? {
            let merged_text = merge_fact_text_with_fallback(router, &existing.dto, &candidate);
            let mut evidence_ids = parse_evidence_ids(&existing.dto.evidence_ids_json);
            evidence_ids.extend(candidate.evidence_ids.iter().copied());
            evidence_ids.sort_unstable();
            evidence_ids.dedup();
            let salience = existing
                .dto
                .salience
                .max(candidate.salience)
                .clamp(0.0, 1.0);
            let confidence = existing
                .dto
                .confidence
                .max(candidate.confidence)
                .clamp(0.0, 1.0);
            let merged_embedding = embeddings
                .embed_text(&merged_text)
                .context("failed to embed merged fact")?;
            update_fact(
                store,
                existing.dto.id,
                &merged_text,
                &candidate.kind,
                &merged_embedding,
                embeddings.model_id(),
                confidence,
                salience,
                &evidence_ids,
            )?;
            report.merged_fact_count += 1;
        } else {
            insert_fact(store, embeddings.model_id(), &candidate, &embedding)?;
            report.upserted_fact_count += 1;
        }
    }

    let ids = episodes
        .iter()
        .map(|episode| episode.episode.id)
        .collect::<Vec<_>>();
    mark_episodes_consolidated(store, &ids)?;
    report.marked_episode_count = ids.len();
    dropped = enforce_fact_cap(store)?;
    report.dropped_fact_count += dropped;
    Ok(report)
}

pub fn unconsolidated_episode_count(store: &TaskStore) -> Result<usize> {
    let conn = store.connect()?;
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM episodes WHERE consolidated_at IS NULL",
            [],
            |row| row.get(0),
        )
        .context("failed to count unconsolidated episodes")?;
    Ok(count.max(0) as usize)
}

pub fn list_facts(store: &TaskStore, limit: usize) -> Result<Vec<FactDto>> {
    let conn = store.connect()?;
    let mut stmt = conn
        .prepare(
            "SELECT id, text, kind, confidence, evidence_ids_json, salience, last_reinforced, created_at
             FROM facts
             ORDER BY salience DESC, last_reinforced DESC, id DESC
             LIMIT ?1",
        )
        .context("failed to prepare fact list query")?;
    let rows = stmt
        .query_map(params![limit as i64], fact_from_row)
        .context("failed to query facts")?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to collect facts")
}

pub fn delete_fact(store: &TaskStore, id: i64) -> Result<()> {
    let conn = store.connect()?;
    conn.execute("DELETE FROM facts WHERE id = ?1", params![id])
        .context("failed to delete fact")?;
    Ok(())
}

pub fn clear_facts(store: &TaskStore) -> Result<()> {
    let conn = store.connect()?;
    conn.execute("DELETE FROM facts", [])
        .context("failed to clear facts")?;
    Ok(())
}

pub fn build_memory_prompt_context(store: &TaskStore, limit: usize) -> Result<Option<String>> {
    let facts = list_facts(store, limit)?;
    if facts.is_empty() {
        return Ok(None);
    }
    let lines = facts
        .into_iter()
        .map(|fact| format!("- {}: {}", fact.kind, fact.text))
        .collect::<Vec<_>>();
    Ok(Some(cap_words(
        &format!("[Memory facts]\n{}", lines.join("\n")),
        140,
    )))
}

fn extract_fact_candidates_with_fallback(
    router: &ModelRouter,
    episodes: &[EpisodeWithEmbedding],
) -> Vec<FactCandidate> {
    #[cfg(test)]
    {
        let _ = router;
        return deterministic_fact_candidates(episodes);
    }

    #[cfg(not(test))]
    match extract_fact_candidates(router, episodes) {
        Ok(candidates) if !candidates.is_empty() => candidates,
        Ok(_) => deterministic_fact_candidates(episodes),
        Err(err) => {
            eprintln!("[jeff] consolidation_fallback reason={err}");
            deterministic_fact_candidates(episodes)
        }
    }
}

#[cfg(not(test))]
fn extract_fact_candidates(
    router: &ModelRouter,
    episodes: &[EpisodeWithEmbedding],
) -> Result<Vec<FactCandidate>> {
    let prompt = episodes
        .iter()
        .map(|entry| {
            format!(
                "episode_id={} kind={} salience={:.2} text={}",
                entry.episode.id, entry.episode.kind, entry.episode.salience, entry.episode.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    if prompt.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut request = ModelRequest::new(Tier::Craft, CONSOLIDATION_SYSTEM_PROMPT, prompt)
        .with_options(GenerateOptions {
            temperature: 0.0,
            max_tokens: Some(700),
            json_object: true,
            timeout_ms: Some(CONSOLIDATION_TIMEOUT_MS),
        })
        .with_budget_key(CONSOLIDATION_BUDGET_KEY);
    request.purpose = Some("consolidation".to_string());
    let raw = router.route(request)?.text;
    parse_fact_candidate_json(&raw, episodes)
}

fn deterministic_fact_candidates(episodes: &[EpisodeWithEmbedding]) -> Vec<FactCandidate> {
    let mut candidates = Vec::new();
    let mut stuck_ids = Vec::new();
    for entry in episodes {
        let episode = &entry.episode;
        let lower = episode.text.to_ascii_lowercase();
        let kind = if looks_like_deadline(&lower) || episode.kind == memory::KIND_DEADLINE_MENTION {
            FACT_KIND_DEADLINE
        } else if looks_like_pattern(&lower) {
            stuck_ids.push(episode.id);
            FACT_KIND_PATTERN
        } else if episode.kind == memory::KIND_USER_FACT && looks_like_person(&lower) {
            FACT_KIND_PERSON
        } else if episode.kind == memory::KIND_USER_FACT && looks_like_constraint(&lower) {
            FACT_KIND_CONSTRAINT
        } else if episode.kind == memory::KIND_USER_FACT {
            FACT_KIND_PREFERENCE
        } else {
            FACT_KIND_CONTEXT
        };
        let text = fact_text_from_episode(episode, kind);
        candidates.push(FactCandidate::new(kind, text, episode));
    }

    if stuck_ids.len() >= 2 {
        candidates.push(FactCandidate {
            kind: FACT_KIND_PATTERN.to_string(),
            text: "Recurring pattern: work gets stuck or churns around the same issue.".to_string(),
            confidence: 0.72,
            salience: 0.82,
            evidence_ids: stuck_ids,
        });
    }

    dedupe_candidates(candidates)
}

#[cfg(not(test))]
fn parse_fact_candidate_json(
    raw: &str,
    episodes: &[EpisodeWithEmbedding],
) -> Result<Vec<FactCandidate>> {
    let json =
        extract_json_object(raw).ok_or_else(|| anyhow!("consolidation response had no json"))?;
    #[derive(Deserialize)]
    struct Payload {
        #[serde(default)]
        facts: Vec<RawFact>,
    }
    #[derive(Deserialize)]
    struct RawFact {
        kind: String,
        text: String,
        #[serde(default = "default_confidence")]
        confidence: f32,
        #[serde(default = "default_salience")]
        salience: f32,
        #[serde(default)]
        evidence_ids: Vec<i64>,
    }
    fn default_confidence() -> f32 {
        0.65
    }
    fn default_salience() -> f32 {
        0.65
    }

    let valid_ids = episodes
        .iter()
        .map(|entry| entry.episode.id)
        .collect::<std::collections::HashSet<_>>();
    let parsed: Payload = serde_json::from_str(json).context("failed to parse fact json")?;
    let mut candidates = Vec::new();
    for raw in parsed.facts {
        validate_fact_kind(&raw.kind)?;
        let evidence_ids = raw
            .evidence_ids
            .into_iter()
            .filter(|id| valid_ids.contains(id))
            .collect::<Vec<_>>();
        if evidence_ids.is_empty() {
            continue;
        }
        let text = clean_fact_text(&raw.text);
        if text.is_empty() {
            continue;
        }
        candidates.push(FactCandidate {
            kind: raw.kind,
            text,
            confidence: raw.confidence.clamp(0.0, 1.0),
            salience: raw.salience.clamp(0.0, 1.0),
            evidence_ids,
        });
    }
    Ok(dedupe_candidates(candidates))
}

fn best_fact_match(store: &TaskStore, kind: &str, embedding: &[f32]) -> Result<Option<StoredFact>> {
    let facts = list_fact_embeddings(store, 1_000)?;
    Ok(facts
        .into_iter()
        .filter(|fact| fact.dto.kind == kind)
        .map(|fact| {
            let score = cosine_similarity(embedding, &fact.embedding);
            (score, fact)
        })
        .filter(|(score, _)| *score > MERGE_SIMILARITY_THRESHOLD)
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_, fact)| fact))
}

fn insert_fact(
    store: &TaskStore,
    embedding_model: &str,
    candidate: &FactCandidate,
    embedding: &[f32],
) -> Result<i64> {
    let conn = store.connect()?;
    conn.execute(
        "INSERT INTO facts
         (text, kind, embedding, embedding_model, confidence, evidence_ids_json, salience)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            candidate.text,
            candidate.kind,
            memory::encode_embedding(embedding),
            embedding_model,
            candidate.confidence.clamp(0.0, 1.0),
            evidence_json(&candidate.evidence_ids),
            candidate.salience.clamp(0.0, 1.0),
        ],
    )
    .context("failed to insert fact")?;
    Ok(conn.last_insert_rowid())
}

fn update_fact(
    store: &TaskStore,
    fact_id: i64,
    text: &str,
    kind: &str,
    embedding: &[f32],
    embedding_model: &str,
    confidence: f32,
    salience: f32,
    evidence_ids: &[i64],
) -> Result<()> {
    let conn = store.connect()?;
    conn.execute(
        "UPDATE facts
         SET text = ?1,
             kind = ?2,
             embedding = ?3,
             embedding_model = ?4,
             confidence = ?5,
             evidence_ids_json = ?6,
             salience = ?7,
             last_reinforced = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?8",
        params![
            clean_fact_text(text),
            kind,
            memory::encode_embedding(embedding),
            embedding_model,
            confidence.clamp(0.0, 1.0),
            evidence_json(evidence_ids),
            salience.clamp(0.0, 1.0),
            fact_id,
        ],
    )
    .context("failed to update fact")?;
    Ok(())
}

fn mark_episodes_consolidated(store: &TaskStore, episode_ids: &[i64]) -> Result<()> {
    let conn = store.connect()?;
    for id in episode_ids {
        conn.execute(
            "UPDATE episodes
             SET consolidated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?1",
            params![id],
        )
        .context("failed to mark episode consolidated")?;
    }
    Ok(())
}

fn list_unconsolidated_episode_embeddings(
    store: &TaskStore,
    limit: usize,
) -> Result<Vec<EpisodeWithEmbedding>> {
    let conn = store.connect()?;
    let mut stmt = conn
        .prepare(
            "SELECT id, task_id, kind, text, salience, source, created_at, consolidated_at
             FROM episodes
             WHERE consolidated_at IS NULL
             ORDER BY id ASC
             LIMIT ?1",
        )
        .context("failed to prepare unconsolidated episode query")?;
    let rows = stmt
        .query_map(params![limit as i64], |row| {
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
            Ok(EpisodeWithEmbedding { episode })
        })
        .context("failed to query unconsolidated episodes")?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to collect unconsolidated episodes")
}

fn list_fact_embeddings(store: &TaskStore, limit: usize) -> Result<Vec<StoredFact>> {
    let conn = store.connect()?;
    let mut stmt = conn
        .prepare(
            "SELECT id, text, kind, confidence, evidence_ids_json, salience, last_reinforced, created_at, embedding
             FROM facts
             ORDER BY salience DESC, last_reinforced DESC, id DESC
             LIMIT ?1",
        )
        .context("failed to prepare fact embedding query")?;
    let rows = stmt
        .query_map(params![limit as i64], |row| {
            let dto = FactDto {
                id: row.get(0)?,
                text: row.get(1)?,
                kind: row.get(2)?,
                confidence: row.get::<_, f64>(3)? as f32,
                evidence_ids_json: row.get(4)?,
                salience: row.get::<_, f64>(5)? as f32,
                last_reinforced: row.get(6)?,
                created_at: row.get(7)?,
            };
            let blob: Vec<u8> = row.get(8)?;
            Ok(StoredFact {
                dto,
                embedding: memory::decode_embedding(&blob),
            })
        })
        .context("failed to query fact embeddings")?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to collect fact embeddings")
}

fn apply_decay(store: &TaskStore) -> Result<(usize, usize)> {
    let facts = list_facts(store, 5_000)?;
    let now = chrono::Utc::now().timestamp();
    let mut decayed = 0usize;
    let mut dropped = 0usize;
    let conn = store.connect()?;
    for fact in facts {
        let Some(last) = parse_sqlite_datetime_to_unix(&fact.last_reinforced) else {
            continue;
        };
        let elapsed_days = now.saturating_sub(last) / 86_400;
        let periods = elapsed_days / 30;
        if periods <= 0 {
            continue;
        }
        let next = fact.salience * 0.5_f32.powi(periods as i32);
        if next < 0.1 {
            conn.execute("DELETE FROM facts WHERE id = ?1", params![fact.id])
                .context("failed to drop decayed fact")?;
            dropped += 1;
        } else {
            conn.execute(
                "UPDATE facts
                 SET salience = ?1,
                     last_reinforced = strftime('%Y-%m-%dT%H:%M:%fZ','now')
                 WHERE id = ?2",
                params![next, fact.id],
            )
            .context("failed to decay fact salience")?;
            decayed += 1;
        }
    }
    Ok((decayed, dropped))
}

fn enforce_fact_cap(store: &TaskStore) -> Result<usize> {
    let conn = store.connect()?;
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM facts", [], |row| row.get(0))
        .context("failed to count facts")?;
    let over = (count as isize - MAX_FACTS as isize).max(0) as usize;
    if over == 0 {
        return Ok(0);
    }
    let mut stmt = conn
        .prepare(
            "SELECT id
             FROM facts
             ORDER BY salience ASC, last_reinforced ASC, id ASC
             LIMIT ?1",
        )
        .context("failed to prepare fact cap query")?;
    let ids = stmt
        .query_map(params![over as i64], |row| row.get::<_, i64>(0))
        .context("failed to query fact cap ids")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to collect fact cap ids")?;
    for id in &ids {
        conn.execute("DELETE FROM facts WHERE id = ?1", params![id])
            .context("failed to delete fact over cap")?;
    }
    Ok(ids.len())
}

fn merge_fact_text_with_fallback(
    router: &ModelRouter,
    existing: &FactDto,
    candidate: &FactCandidate,
) -> String {
    if normalize_for_merge(&existing.text) == normalize_for_merge(&candidate.text) {
        return existing.text.clone();
    }
    #[cfg(test)]
    {
        let _ = router;
        return if candidate.confidence > existing.confidence {
            candidate.text.clone()
        } else {
            existing.text.clone()
        };
    }

    #[cfg(not(test))]
    {
        let prompt = format!(
            "Existing fact: {}\nNew fact: {}\nMerged fact:",
            existing.text, candidate.text
        );
        let mut request = ModelRequest::new(Tier::Craft, FACT_MERGE_SYSTEM_PROMPT, prompt)
            .with_options(GenerateOptions {
                temperature: 0.0,
                max_tokens: Some(120),
                json_object: false,
                timeout_ms: Some(CONSOLIDATION_TIMEOUT_MS),
            })
            .with_budget_key(CONSOLIDATION_BUDGET_KEY);
        request.purpose = Some("consolidation_merge".to_string());
        match router.route(request) {
            Ok(response) => clean_fact_text(&response.text),
            Err(err) => {
                eprintln!("[jeff] fact_merge_fallback reason={err}");
                if candidate.confidence > existing.confidence {
                    candidate.text.clone()
                } else {
                    existing.text.clone()
                }
            }
        }
    }
}

fn record_consolidation_budget_touch(store: &TaskStore) -> Result<()> {
    store.append_llm_usage_log(&LlmUsageLogInput {
        tier: CONSOLIDATION_BUDGET_KEY.to_string(),
        model: "local-consolidator".to_string(),
        purpose: "consolidation".to_string(),
        input_tokens: 0,
        output_tokens: 0,
        cached_tokens: 0,
        est_cost_usd: crate::cost_governor::estimate_cost_usd(
            ProviderKind::Local,
            "local-consolidator",
            crate::model_router::LlmUsage::default(),
        ),
    })
}

fn fact_from_row(row: &Row<'_>) -> rusqlite::Result<FactDto> {
    Ok(FactDto {
        id: row.get(0)?,
        text: row.get(1)?,
        kind: row.get(2)?,
        confidence: row.get::<_, f64>(3)? as f32,
        evidence_ids_json: row.get(4)?,
        salience: row.get::<_, f64>(5)? as f32,
        last_reinforced: row.get(6)?,
        created_at: row.get(7)?,
    })
}

fn fact_text_from_episode(episode: &EpisodeDto, kind: &str) -> String {
    let mut text = episode.text.trim().to_string();
    for prefix in [
        "User fact:",
        "Decision:",
        "Deadline/date mentioned:",
        "Accepted",
        "Rejected",
    ] {
        if text.starts_with(prefix) && prefix != "Accepted" && prefix != "Rejected" {
            text = text[prefix.len()..].trim().to_string();
        }
    }
    match kind {
        FACT_KIND_DEADLINE => format!("Deadline/date: {}", clean_fact_text(&text)),
        FACT_KIND_PATTERN => format!("Recurring pattern: {}", clean_fact_text(&text)),
        FACT_KIND_PREFERENCE => clean_fact_text(&text),
        FACT_KIND_PERSON => clean_fact_text(&text),
        FACT_KIND_CONSTRAINT => clean_fact_text(&text),
        _ => clean_fact_text(&text),
    }
}

fn confidence_for_episode(episode: &EpisodeDto) -> f32 {
    match episode.kind.as_str() {
        memory::KIND_DEADLINE_MENTION => 0.82,
        memory::KIND_USER_FACT => 0.78,
        memory::KIND_DECISION | memory::KIND_PROPOSAL_OUTCOME => 0.72,
        memory::KIND_WORK_UNDERSTANDING => 0.68,
        _ => 0.62,
    }
}

fn clean_fact_text(text: &str) -> String {
    let trimmed = text.trim();
    let clean = if trimmed.chars().count() > MAX_FACT_TEXT_CHARS {
        trimmed.chars().take(MAX_FACT_TEXT_CHARS).collect()
    } else {
        trimmed.to_string()
    };
    clean.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn looks_like_deadline(lower: &str) -> bool {
    lower.contains("deadline")
        || lower.contains("due ")
        || lower.contains(" by ")
        || lower.contains("tonight")
        || lower.contains("tomorrow")
}

fn looks_like_person(lower: &str) -> bool {
    lower.contains("advisor")
        || lower.contains("boss")
        || lower.contains("client")
        || lower.contains("manager")
        || lower.contains("teammate")
}

fn looks_like_constraint(lower: &str) -> bool {
    lower.contains("must ")
        || lower.contains("cannot ")
        || lower.contains("can't ")
        || lower.contains("has to ")
        || lower.contains("required")
}

fn looks_like_pattern(lower: &str) -> bool {
    lower.contains("stuck")
        || lower.contains("blocked")
        || lower.contains("churn")
        || lower.contains("keeps drifting")
        || lower.contains("keeps changing")
        || lower.contains("looping")
}

fn dedupe_candidates(mut candidates: Vec<FactCandidate>) -> Vec<FactCandidate> {
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|candidate| {
        seen.insert((
            candidate.kind.clone(),
            normalize_for_merge(&candidate.text),
            evidence_json(&candidate.evidence_ids),
        ))
    });
    candidates
}

fn evidence_json(ids: &[i64]) -> String {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    serde_json::to_string(&ids).unwrap_or_else(|_| "[]".to_string())
}

fn parse_evidence_ids(raw: &str) -> Vec<i64> {
    serde_json::from_str::<Vec<i64>>(raw).unwrap_or_default()
}

fn normalize_for_merge(text: &str) -> String {
    text.to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn validate_fact_kind(kind: &str) -> Result<()> {
    if matches!(
        kind,
        FACT_KIND_PREFERENCE
            | FACT_KIND_CONSTRAINT
            | FACT_KIND_DEADLINE
            | FACT_KIND_PERSON
            | FACT_KIND_PATTERN
            | FACT_KIND_CONTEXT
    ) {
        Ok(())
    } else {
        Err(anyhow!("unsupported fact kind '{kind}'"))
    }
}

fn cap_words(text: &str, max_words: usize) -> String {
    text.split_whitespace()
        .take(max_words)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(not(test))]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cost_governor, memory::NewEpisode, model_router::RouterConfig,
        providers::local::hash_embedding,
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
        let task = store.create_task("Consolidation Test").unwrap();
        (dir, store, task.id)
    }

    fn router() -> ModelRouter {
        ModelRouter::new(RouterConfig::default())
    }

    #[test]
    fn b4_near_duplicate_episodes_merge_to_one_fact_with_all_evidence() {
        let (_dir, store, task_id) = test_store();
        let embeddings = TestEmbeddingProvider;
        for index in 0..5 {
            memory::record_episode(
                &store,
                &embeddings,
                &NewEpisode::new(
                    task_id,
                    memory::KIND_USER_FACT,
                    "User fact: I prefer concise revisions.",
                    &format!("seed:{index}"),
                ),
            )
            .unwrap();
        }

        let report = run_consolidation(&store, &embeddings, &router()).unwrap();
        assert_eq!(report.processed_episode_count, 5);
        let facts = list_facts(&store, 10).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].kind, FACT_KIND_PREFERENCE);
        assert_eq!(parse_evidence_ids(&facts[0].evidence_ids_json).len(), 5);
    }

    #[test]
    fn b4_decayed_fact_is_dropped_by_job() {
        let (_dir, store, _task_id) = test_store();
        let embeddings = TestEmbeddingProvider;
        let embedding = embeddings.embed_text("stale fact").unwrap();
        let conn = store.connect().unwrap();
        conn.execute(
            "INSERT INTO facts
             (text, kind, embedding, embedding_model, confidence, evidence_ids_json, salience, last_reinforced, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now','-90 days'), datetime('now','-90 days'))",
            params![
                "stale fact",
                FACT_KIND_CONTEXT,
                memory::encode_embedding(&embedding),
                embeddings.model_id(),
                0.6,
                "[1]",
                0.2,
            ],
        )
        .unwrap();
        drop(conn);

        let report = run_consolidation(&store, &embeddings, &router()).unwrap();
        assert_eq!(report.dropped_fact_count, 1);
        assert!(list_facts(&store, 10).unwrap().is_empty());
    }

    #[test]
    fn b4_delete_fact_removes_it_from_prompt_preview() {
        let (_dir, store, task_id) = test_store();
        let embeddings = TestEmbeddingProvider;
        memory::record_episode(
            &store,
            &embeddings,
            &NewEpisode::new(
                task_id,
                memory::KIND_USER_FACT,
                "User fact: I prefer source-heavy answers.",
                "seed",
            ),
        )
        .unwrap();
        run_consolidation(&store, &embeddings, &router()).unwrap();
        let fact = list_facts(&store, 10).unwrap().remove(0);
        assert!(build_memory_prompt_context(&store, 10)
            .unwrap()
            .unwrap()
            .contains("source-heavy"));
        delete_fact(&store, fact.id).unwrap();
        assert!(build_memory_prompt_context(&store, 10).unwrap().is_none());
    }

    #[test]
    fn b4_consolidation_spend_appears_under_named_sub_budget() {
        let (_dir, store, _task_id) = test_store();
        let embeddings = TestEmbeddingProvider;
        run_consolidation(&store, &embeddings, &router()).unwrap();
        let status = cost_governor::status(&store).unwrap();
        assert!(status
            .tiers
            .iter()
            .any(|tier| tier.budget_key == CONSOLIDATION_BUDGET_KEY));
    }

    #[test]
    fn b4_stuck_churn_episodes_promote_pattern_fact() {
        let (_dir, store, task_id) = test_store();
        let embeddings = TestEmbeddingProvider;
        for (index, text) in [
            "User fact: I am stuck in a revision loop.",
            "User fact: This section keeps changing and churning.",
        ]
        .iter()
        .enumerate()
        {
            memory::record_episode(
                &store,
                &embeddings,
                &NewEpisode::new(
                    task_id,
                    memory::KIND_USER_FACT,
                    *text,
                    &format!("pattern:{index}"),
                ),
            )
            .unwrap();
        }
        run_consolidation(&store, &embeddings, &router()).unwrap();
        assert!(list_facts(&store, 10)
            .unwrap()
            .iter()
            .any(|fact| fact.kind == FACT_KIND_PATTERN));
    }
}
