// apex b1: semantic document model. turns captured document text into
// structural, semantic deltas instead of word counts and first-80-char
// comparisons.
//
// privacy contract (extends phase 31): raw paragraph text lives only inside
// this module, in memory. nothing here is persisted to sqlite, written to
// logs, or placed into an api payload. only the structural DocumentDelta and
// the counts-only DocumentStateSummary cross the module boundary — neither
// carries raw document text. the in-memory outline (first lines) exists for
// the b7 comprehension pass and is never emitted by this milestone.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::providers::EmbeddingsProvider;

// a paragraph counts as a churn hotspot once it has been rewritten this many
// times or more. below this it is ordinary editing.
const CHURN_HOTSPOT_MIN: u32 = 2;
// minimum combined (semantic-or-lexical) similarity for two paragraphs across
// polls to be treated as the same paragraph rewritten rather than a
// remove+add pair.
const REWRITE_THRESHOLD: f32 = 0.35;
// paragraph identity search only considers prior paragraphs within this index
// window plus the position penalty, keeping matching stable and local.
const POSITION_PENALTY: f32 = 0.02;
// bound on retained per-task delta history (memory only).
const MAX_DELTA_HISTORY: usize = 100;
// bound on the number of tasks tracked concurrently; oldest-touched evicted.
const MAX_TASKS: usize = 8;

pub type ParaId = u64;

// a reference to a paragraph that was added or removed. first_line is a short
// snippet retained in memory only; it is never persisted or logged.
#[derive(Debug, Clone, PartialEq)]
pub struct ParaRef {
    pub id: ParaId,
    pub first_line: String,
    pub word_count: usize,
}

// a paragraph that persisted across a poll but whose text changed.
#[derive(Debug, Clone, PartialEq)]
pub struct ParaChange {
    pub id: ParaId,
    pub first_line: String,
    pub similarity: f32,
}

// the structural diff between two consecutive observations of a document.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentDelta {
    pub added: Vec<ParaRef>,
    pub removed: Vec<ParaRef>,
    pub rewritten: Vec<ParaChange>,
    pub churn_map: HashMap<ParaId, u32>,
    pub word_count: usize,
    pub structure_changed: bool,
    pub captured_at: i64,
}

// counts-only export for the snapshot. carries no raw document text, so it is
// safe to place into ContentObservationState and, downstream, into the llm
// snapshot summary.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DocumentStateSummary {
    pub paragraph_count: usize,
    pub word_count: usize,
    pub structure_changed: bool,
    pub max_churn: u32,
    pub churn_hotspot_count: usize,
    pub added_last: usize,
    pub removed_last: usize,
    pub rewritten_last: usize,
}

#[derive(Debug, Clone)]
struct Paragraph {
    id: ParaId,
    text: String,
    hash: u64,
    embedding: Vec<f32>,
    word_count: usize,
    is_heading: bool,
    churn: u32,
}

#[derive(Debug, Default)]
struct TaskDoc {
    paragraphs: Vec<Paragraph>,
    history: VecDeque<DocumentDelta>,
    next_id: ParaId,
    touched_at: i64,
}

#[derive(Debug, Default)]
pub struct DocumentModel {
    tasks: HashMap<i64, TaskDoc>,
}

impl DocumentModel {
    pub fn new() -> Self {
        Self::default()
    }

    // observe the current full text of the active document for a task and
    // return the structural delta versus the previous observation. only
    // changed paragraphs are re-embedded (incremental).
    pub fn observe(
        &mut self,
        task_id: i64,
        text: &str,
        embeddings: &dyn EmbeddingsProvider,
    ) -> DocumentDelta {
        self.evict_if_needed(task_id);
        let now = unix_now();
        let doc = self.tasks.entry(task_id).or_default();
        if doc.next_id == 0 {
            doc.next_id = 1;
        }
        doc.touched_at = now;

        let new_units = segment_paragraphs(text);
        let prior = std::mem::take(&mut doc.paragraphs);
        let mut prior_used = vec![false; prior.len()];
        let mut resolved: Vec<Option<Paragraph>> = vec![None; new_units.len()];

        let mut added: Vec<ParaRef> = Vec::new();
        let mut rewritten: Vec<ParaChange> = Vec::new();

        // pass 1: exact-hash matches are unchanged paragraphs. reuse the prior
        // id, embedding, and churn without re-embedding.
        for (i, unit) in new_units.iter().enumerate() {
            let h = hash_text(&unit.text);
            if let Some(pi) = find_prior(&prior, &prior_used, i, |p| p.hash == h) {
                prior_used[pi] = true;
                let mut carried = prior[pi].clone();
                carried.is_heading = unit.is_heading;
                resolved[i] = Some(carried);
            }
        }

        // pass 2: remaining new paragraphs are either rewrites of an unused
        // prior paragraph (bump churn) or genuinely new (added). only these
        // get embedded.
        for (i, unit) in new_units.iter().enumerate() {
            if resolved[i].is_some() {
                continue;
            }
            let embedding = embeddings.embed_text(&unit.text).unwrap_or_default();
            match best_similar_prior(&prior, &prior_used, &embedding, &unit.text, i) {
                Some((pi, similarity)) => {
                    prior_used[pi] = true;
                    let id = prior[pi].id;
                    let churn = prior[pi].churn.saturating_add(1);
                    rewritten.push(ParaChange {
                        id,
                        first_line: first_line(&unit.text),
                        similarity,
                    });
                    resolved[i] = Some(Paragraph {
                        id,
                        text: unit.text.clone(),
                        hash: hash_text(&unit.text),
                        embedding,
                        word_count: unit.word_count,
                        is_heading: unit.is_heading,
                        churn,
                    });
                }
                None => {
                    let id = doc.next_id;
                    doc.next_id = doc.next_id.saturating_add(1);
                    added.push(ParaRef {
                        id,
                        first_line: first_line(&unit.text),
                        word_count: unit.word_count,
                    });
                    resolved[i] = Some(Paragraph {
                        id,
                        text: unit.text.clone(),
                        hash: hash_text(&unit.text),
                        embedding,
                        word_count: unit.word_count,
                        is_heading: unit.is_heading,
                        churn: 0,
                    });
                }
            }
        }

        // prior paragraphs left unmatched were removed.
        let removed: Vec<ParaRef> = prior
            .iter()
            .enumerate()
            .filter(|(pi, _)| !prior_used[*pi])
            .map(|(_, p)| ParaRef {
                id: p.id,
                first_line: first_line(&p.text),
                word_count: p.word_count,
            })
            .collect();

        let new_paragraphs: Vec<Paragraph> = resolved.into_iter().flatten().collect();

        let structure_changed = !added.is_empty()
            || !removed.is_empty()
            || heading_signature(&prior) != heading_signature(&new_paragraphs);

        let churn_map: HashMap<ParaId, u32> = new_paragraphs
            .iter()
            .map(|p| (p.id, p.churn))
            .collect();
        let word_count = new_paragraphs.iter().map(|p| p.word_count).sum();

        let delta = DocumentDelta {
            added,
            removed,
            rewritten,
            churn_map,
            word_count,
            structure_changed,
            captured_at: now,
        };

        doc.paragraphs = new_paragraphs;
        doc.history.push_back(delta.clone());
        while doc.history.len() > MAX_DELTA_HISTORY {
            doc.history.pop_front();
        }

        delta
    }

    // counts-only summary for the snapshot. none of these fields carry raw
    // document text.
    pub fn state(&self, task_id: i64) -> Option<DocumentStateSummary> {
        let doc = self.tasks.get(&task_id)?;
        let last = doc.history.back();
        let max_churn = doc.paragraphs.iter().map(|p| p.churn).max().unwrap_or(0);
        let churn_hotspot_count = doc
            .paragraphs
            .iter()
            .filter(|p| p.churn >= CHURN_HOTSPOT_MIN)
            .count();
        Some(DocumentStateSummary {
            paragraph_count: doc.paragraphs.len(),
            word_count: doc.paragraphs.iter().map(|p| p.word_count).sum(),
            structure_changed: last.map(|d| d.structure_changed).unwrap_or(false),
            max_churn,
            churn_hotspot_count,
            added_last: last.map(|d| d.added.len()).unwrap_or(0),
            removed_last: last.map(|d| d.removed.len()).unwrap_or(0),
            rewritten_last: last.map(|d| d.rewritten.len()).unwrap_or(0),
        })
    }

    // in-memory outline (first line per paragraph). retained for the b7
    // comprehension pass; not emitted or persisted by this milestone.
    #[allow(dead_code)]
    pub fn outline(&self, task_id: i64) -> Vec<String> {
        self.tasks
            .get(&task_id)
            .map(|doc| doc.paragraphs.iter().map(|p| first_line(&p.text)).collect())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub fn history_len(&self, task_id: i64) -> usize {
        self.tasks.get(&task_id).map(|d| d.history.len()).unwrap_or(0)
    }

    fn evict_if_needed(&mut self, incoming: i64) {
        if self.tasks.len() < MAX_TASKS || self.tasks.contains_key(&incoming) {
            return;
        }
        if let Some((&oldest, _)) = self
            .tasks
            .iter()
            .min_by_key(|(_, doc)| doc.touched_at)
        {
            self.tasks.remove(&oldest);
        }
    }
}

struct SegmentUnit {
    text: String,
    word_count: usize,
    is_heading: bool,
}

// deterministic paragraph segmentation. when the document uses blank-line
// separators, split on blank lines and isolate markdown headings; otherwise
// fall back to per-line segmentation (each non-empty line is a paragraph),
// which matches editors that put one paragraph per line without blank spacers.
fn segment_paragraphs(text: &str) -> Vec<SegmentUnit> {
    let normalized = text.replace("\r\n", "\n");
    let has_blank_separators = normalized.contains("\n\n");
    let mut units: Vec<SegmentUnit> = Vec::new();

    if has_blank_separators {
        let mut current = String::new();
        let flush = |current: &mut String, units: &mut Vec<SegmentUnit>| {
            let trimmed = current.trim();
            if !trimmed.is_empty() {
                units.push(make_unit(trimmed));
            }
            current.clear();
        };
        for line in normalized.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                flush(&mut current, &mut units);
            } else if is_heading_line(trimmed) {
                flush(&mut current, &mut units);
                units.push(make_unit(trimmed));
            } else {
                if !current.is_empty() {
                    current.push(' ');
                }
                current.push_str(trimmed);
            }
        }
        let trimmed = current.trim();
        if !trimmed.is_empty() {
            units.push(make_unit(trimmed));
        }
    } else {
        for line in normalized.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                units.push(make_unit(trimmed));
            }
        }
    }

    units
}

fn make_unit(text: &str) -> SegmentUnit {
    SegmentUnit {
        text: text.to_string(),
        word_count: text.split_whitespace().count(),
        is_heading: is_heading_line(text),
    }
}

fn is_heading_line(line: &str) -> bool {
    line.starts_with('#')
}

// a stable signature of the document's heading structure, used to flag
// structural change even when paragraph counts are unchanged.
fn heading_signature(paragraphs: &[Paragraph]) -> Vec<u64> {
    paragraphs
        .iter()
        .filter(|p| p.is_heading)
        .map(|p| p.hash)
        .collect()
}

// find the closest unused prior paragraph satisfying a predicate, preferring
// the nearest index to keep identity local when the same text appears twice.
fn find_prior<F>(prior: &[Paragraph], used: &[bool], target_index: usize, pred: F) -> Option<usize>
where
    F: Fn(&Paragraph) -> bool,
{
    let mut best: Option<(usize, usize)> = None;
    for (pi, p) in prior.iter().enumerate() {
        if used[pi] || !pred(p) {
            continue;
        }
        let distance = target_index.abs_diff(pi);
        match best {
            Some((_, best_distance)) if best_distance <= distance => {}
            _ => best = Some((pi, distance)),
        }
    }
    best.map(|(pi, _)| pi)
}

// score every unused prior paragraph as a rewrite candidate. score is the max
// of embedding cosine and lexical (token jaccard) overlap, minus a small
// position penalty. returns the best above threshold.
fn best_similar_prior(
    prior: &[Paragraph],
    used: &[bool],
    embedding: &[f32],
    text: &str,
    target_index: usize,
) -> Option<(usize, f32)> {
    let new_tokens = token_set(text);
    let mut best: Option<(usize, f32)> = None;
    for (pi, p) in prior.iter().enumerate() {
        if used[pi] {
            continue;
        }
        let semantic = cosine(embedding, &p.embedding);
        let lexical = jaccard(&new_tokens, &token_set(&p.text));
        let distance = target_index.abs_diff(pi) as f32;
        let score = semantic.max(lexical) - POSITION_PENALTY * distance;
        if score >= REWRITE_THRESHOLD {
            match best {
                Some((_, best_score)) if best_score >= score => {}
                _ => best = Some((pi, score)),
            }
        }
    }
    best
}

fn token_set(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() > 1)
        .map(|t| t.to_ascii_lowercase())
        .collect();
    tokens.sort();
    tokens.dedup();
    tokens
}

fn jaccard(a: &[String], b: &[String]) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut intersection = 0usize;
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Equal => {
                intersection += 1;
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    let union = a.len() + b.len() - intersection;
    if union == 0 {
        0.0
    } else {
        intersection as f32 / union as f32
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for k in 0..a.len() {
        dot += a[k] * b[k];
        na += a[k] * a[k];
        nb += b[k] * b[k];
    }
    if na <= 0.0 || nb <= 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

fn hash_text(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn first_line(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(80)
        .collect()
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::local::{hash_embedding, LocalEmbeddingProvider};
    use crate::local_runtime::LocalRuntime;
    use std::sync::Arc;
    use std::time::Instant;

    // deterministic in-process embedder used by the document model tests. this
    // is the lexical hash embedder — churn/structure detection is designed to
    // work correctly even on lexical vectors; semantic quality is what the
    // b1 embedding substrate adds on top for b3/b5 recall.
    fn hash_provider() -> LocalEmbeddingProvider {
        let dir = tempfile::tempdir().unwrap();
        LocalEmbeddingProvider::new(Arc::new(LocalRuntime::new(dir.path())))
    }

    fn doc(paras: &[&str]) -> String {
        paras.join("\n\n")
    }

    #[test]
    fn b1_rewriting_one_paragraph_localizes_churn() {
        let mut model = DocumentModel::new();
        let provider = hash_provider();
        let task_id = 1;

        // establish three paragraphs.
        model.observe(
            task_id,
            &doc(&[
                "The introduction sets up the central thesis of the essay.",
                "The middle paragraph develops the supporting argument in detail.",
                "The conclusion restates the thesis and closes the discussion.",
            ]),
            &provider,
        );

        // rewrite only the middle paragraph, five times, with light edits.
        let variants = [
            "The middle paragraph develops the supporting argument in careful detail.",
            "The middle paragraph develops the main supporting argument in careful detail.",
            "The middle paragraph develops the main supporting argument in precise detail.",
            "The middle paragraph now develops the main supporting argument in precise detail.",
            "The middle paragraph now develops the core supporting argument in precise detail.",
        ];
        let mut last = None;
        for variant in variants {
            last = Some(model.observe(
                task_id,
                &doc(&[
                    "The introduction sets up the central thesis of the essay.",
                    variant,
                    "The conclusion restates the thesis and closes the discussion.",
                ]),
                &provider,
            ));
        }

        let delta = last.unwrap();
        // churn is localized: exactly one paragraph carries all five rewrites.
        let mut churny: Vec<(&ParaId, &u32)> =
            delta.churn_map.iter().filter(|(_, c)| **c > 0).collect();
        churny.sort_by_key(|(id, _)| **id);
        assert_eq!(churny.len(), 1, "churn should be localized to one paragraph");
        assert_eq!(*churny[0].1, 5, "the rewritten paragraph should have churn 5");
        // the other two paragraphs never changed.
        let zero = delta.churn_map.values().filter(|c| **c == 0).count();
        assert_eq!(zero, 2, "the untouched paragraphs keep zero churn");

        let state = model.state(task_id).unwrap();
        assert_eq!(state.max_churn, 5);
        assert_eq!(state.churn_hotspot_count, 1);
    }

    #[test]
    fn b1_adding_section_flips_structure_changed_and_lists_added() {
        let mut model = DocumentModel::new();
        let provider = hash_provider();
        let task_id = 2;

        model.observe(
            task_id,
            &doc(&[
                "# Overview",
                "The overview paragraph introduces the piece.",
            ]),
            &provider,
        );
        let delta = model.observe(
            task_id,
            &doc(&[
                "# Overview",
                "The overview paragraph introduces the piece.",
                "# Methods",
                "The methods paragraph describes the approach taken.",
            ]),
            &provider,
        );

        assert!(delta.structure_changed, "adding a section flips structure_changed");
        assert!(
            delta.added.iter().any(|p| p.first_line.contains("Methods")),
            "the new heading is listed in added"
        );
        assert!(delta.removed.is_empty());
    }

    #[test]
    fn b1_removing_paragraph_is_reported_and_flips_structure() {
        let mut model = DocumentModel::new();
        let provider = hash_provider();
        let task_id = 3;

        model.observe(
            task_id,
            &doc(&["First paragraph stands alone.", "Second paragraph to be removed."]),
            &provider,
        );
        let delta = model.observe(task_id, "First paragraph stands alone.", &provider);

        assert_eq!(delta.removed.len(), 1);
        assert!(delta.structure_changed);
    }

    #[test]
    fn b1_unchanged_document_reports_no_structural_change() {
        let mut model = DocumentModel::new();
        let provider = hash_provider();
        let task_id = 4;
        let text = doc(&[
            "A stable opening paragraph that will not change.",
            "A stable closing paragraph that will not change either.",
        ]);
        model.observe(task_id, &text, &provider);
        let delta = model.observe(task_id, &text, &provider);

        assert!(!delta.structure_changed);
        assert!(delta.added.is_empty());
        assert!(delta.removed.is_empty());
        assert!(delta.rewritten.is_empty());
        assert!(delta.churn_map.values().all(|c| *c == 0));
    }

    #[test]
    fn b1_delta_computation_under_50ms_at_5000_words() {
        let mut model = DocumentModel::new();
        let provider = hash_provider();
        let task_id = 5;

        // build a ~5,000 word document as ~100 paragraphs of 50 words.
        let mut paragraphs: Vec<String> = Vec::new();
        for p in 0..100 {
            let mut words = Vec::new();
            for w in 0..50 {
                words.push(format!("para{p}word{w}"));
            }
            paragraphs.push(words.join(" "));
        }
        let refs: Vec<&str> = paragraphs.iter().map(|s| s.as_str()).collect();
        let full = doc(&refs);
        let word_count = full.split_whitespace().count();
        assert!(word_count >= 5000, "fixture should be at least 5000 words");

        // first observation embeds every paragraph.
        let start = Instant::now();
        model.observe(task_id, &full, &provider);
        let first_elapsed = start.elapsed();
        assert!(
            first_elapsed.as_millis() < 50,
            "first-poll delta computation took {}ms (budget 50ms)",
            first_elapsed.as_millis()
        );

        // steady-state poll with a single changed paragraph.
        let mut changed = paragraphs.clone();
        changed[42].push_str(" appended");
        let refs2: Vec<&str> = changed.iter().map(|s| s.as_str()).collect();
        let start2 = Instant::now();
        model.observe(task_id, &doc(&refs2), &provider);
        let second_elapsed = start2.elapsed();
        assert!(
            second_elapsed.as_millis() < 50,
            "steady-state delta computation took {}ms (budget 50ms)",
            second_elapsed.as_millis()
        );
    }

    #[test]
    fn b1_delta_carries_no_persistence_side_effects() {
        // the model is memory-only: observing does not touch any store. this
        // test simply exercises the ring-buffer bound.
        let mut model = DocumentModel::new();
        let provider = hash_provider();
        let task_id = 6;
        for i in 0..150 {
            model.observe(task_id, &format!("Paragraph revision number {i}."), &provider);
        }
        assert_eq!(model.history_len(task_id), MAX_DELTA_HISTORY);
    }

    #[test]
    fn b1_hash_embeddings_are_semantically_blind_motivating_the_substrate() {
        // documents the exact limitation the b1 embedding substrate fixes:
        // lexical hash vectors cannot see that synonyms are related. these two
        // phrases share no tokens, so hash cosine is ~0 even though they mean
        // the same thing — a real local embedding model (bge-small) scores them
        // close. this is why b3/b5 recall requires the semantic substrate.
        let car = hash_embedding("automobile vehicle sedan");
        let auto = hash_embedding("car motorcar coupe");
        let semantic_pair = cosine(&car, &auto);
        assert!(
            semantic_pair < 0.1,
            "token-disjoint synonyms are invisible to hash embeddings, got {semantic_pair}"
        );
        // sanity: identical text is maximally similar under the same embedder.
        let same = cosine(&car, &hash_embedding("automobile vehicle sedan"));
        assert!(same > 0.99, "identical text should be self-similar, got {same}");
    }
}
