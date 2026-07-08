use std::fs;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::{
    character::{self, RevisionContext},
    chunking::{chunk_text, DEFAULT_CHUNK_OVERLAP_CHARS, DEFAULT_CHUNK_SIZE_CHARS},
    embedding::EmbeddingProvider,
    message_kind::MessageKind,
    model_router::SystemBlock,
    models::{
        ArtifactContentDto, ArtifactVersionDto, RevisionApplyResultDto, RevisionProposalDto,
        RevisionProposalResultDto, RevisionTargetDto,
    },
    reasoning::ReasoningProvider,
    relational_model,
    retrieval::build_task_context_pack,
    store::{ChunkEmbeddingInput, NewArtifactVersionInput, NewRevisionProposalInput, TaskStore},
    user_model,
};

#[derive(Debug, Clone)]
struct ResolvedTarget {
    start_offset: usize,
    end_offset: usize,
    selection_source: String,
    target_description: String,
    original_text: String,
}

#[derive(Debug, Clone)]
struct GeneratedRevision {
    proposed_text: String,
    rationale: Option<String>,
    grounding_notes: Option<String>,
    confidence: f32,
}

#[derive(Debug, Deserialize)]
struct GeneratedRevisionJson {
    proposed_text: String,
    rationale: Option<String>,
    confidence: Option<f32>,
    grounding_notes: Option<String>,
}

pub fn get_artifact_content_for_edit(
    store: &TaskStore,
    artifact_id: i64,
) -> Result<ArtifactContentDto> {
    let artifact = store.get_artifact_content(artifact_id)?;
    if !artifact.is_editable {
        return Err(anyhow!(
            "artifact '{}' is not editable in Phase 6 (only .md/.txt supported)",
            artifact.file_name
        ));
    }

    Ok(artifact)
}

pub fn propose_artifact_revision(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    reasoning: &dyn ReasoningProvider,
    task_id: i64,
    artifact_id: i64,
    selection_or_range: Option<RevisionTargetDto>,
    instruction: &str,
    instruction_source: &str,
    snapshot_summary: Option<&str>,
) -> Result<RevisionProposalResultDto> {
    let clean_instruction = instruction.trim();
    if clean_instruction.is_empty() {
        return Err(anyhow!("revision instruction cannot be empty"));
    }

    let artifact = get_artifact_content_for_edit(store, artifact_id)?;
    if artifact.task_id != task_id {
        return Err(anyhow!(
            "artifact id={} does not belong to task id={}",
            artifact_id,
            task_id
        ));
    }

    let resolved_target = resolve_target(&artifact.content, selection_or_range.as_ref())?;

    let revision_query = format!("{}\n\n{}", clean_instruction, resolved_target.original_text);
    let context_pack = build_task_context_pack(store, embeddings, task_id, &revision_query)?;
    let retrieved_chunks = context_pack.retrieved_chunks;

    let retrieval_confidence = retrieved_chunks
        .first()
        .map(|chunk| chunk.similarity_score.clamp(0.0, 1.0))
        .unwrap_or(0.0);

    let weak_context = retrieval_confidence < 0.2 || retrieved_chunks.is_empty();

    let recent_messages = store.list_recent_chat_messages(task_id, 8)?;
    let revision_prompt = build_revision_prompt(
        &context_pack.task_summary,
        clean_instruction,
        &artifact.content,
        &resolved_target,
        &recent_messages
            .iter()
            .map(|message| {
                format!(
                    "{} ({}) [{}]: {}",
                    message.role, message.message_source, message.message_kind, message.content
                )
            })
            .collect::<Vec<String>>()
            .join("\n"),
        &retrieved_chunks
            .iter()
            .enumerate()
            .map(|(index, chunk)| {
                format!(
                    "Chunk {} | {} | score {:.3}\n{}",
                    index + 1,
                    chunk.artifact_file_name,
                    chunk.similarity_score,
                    chunk.chunk_text
                )
            })
            .collect::<Vec<String>>()
            .join("\n\n"),
    );

    let revision_system_blocks = build_revision_system_blocks(
        store,
        &context_pack.task_summary,
        &resolved_target.target_description,
        clean_instruction,
        snapshot_summary,
    );
    let raw_candidate =
        reasoning.generate_response_blocks(&revision_system_blocks, &revision_prompt)?;
    let generated = parse_generated_revision(
        &raw_candidate,
        &resolved_target.original_text,
        retrieval_confidence,
        weak_context,
    );

    let normalized_source = normalize_instruction_source(instruction_source);

    let proposal = store.create_revision_proposal(&NewRevisionProposalInput {
        task_id,
        artifact_id,
        target_start_offset: resolved_target.start_offset as i64,
        target_end_offset: resolved_target.end_offset as i64,
        target_description: resolved_target.target_description.clone(),
        original_text: resolved_target.original_text.clone(),
        proposed_text: generated.proposed_text.clone(),
        instruction_text: clean_instruction.to_string(),
        instruction_source: normalized_source.to_string(),
        rationale: generated.rationale.clone(),
        grounding_notes: generated.grounding_notes.clone(),
        retrieval_confidence: generated.confidence,
        parent_revision_id: None,
    })?;

    store.append_chat_message(
        task_id,
        "assistant",
        "assistant",
        MessageKind::AssistantRevisionProposal,
        &format!(
            "Revision proposal #{} created for {} ({}).",
            proposal.revision_id, artifact.file_name, resolved_target.target_description
        ),
    )?;

    Ok(RevisionProposalResultDto {
        proposal,
        retrieved_chunks,
        active_artifact_id: artifact_id,
        used_start_offset: resolved_target.start_offset as i64,
        used_end_offset: resolved_target.end_offset as i64,
        selection_source: resolved_target.selection_source,
        confidence: generated.confidence,
        grounding_notes: generated
            .grounding_notes
            .unwrap_or_else(|| "Grounding details unavailable".to_string()),
        context_source: "direct_instruction".to_string(),
    })
}

pub fn list_pending_revisions_for_artifact(
    store: &TaskStore,
    task_id: i64,
    artifact_id: i64,
) -> Result<Vec<RevisionProposalDto>> {
    store.list_pending_revisions(task_id, artifact_id)
}

pub fn apply_revision(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    revision_id: i64,
) -> Result<RevisionApplyResultDto> {
    let revision = store
        .get_revision_by_id(revision_id)?
        .ok_or_else(|| anyhow!("revision id={} not found", revision_id))?;

    if revision.status != "pending" {
        return Err(anyhow!(
            "revision id={} is not pending (status={})",
            revision_id,
            revision.status
        ));
    }

    let artifact_before = get_artifact_content_for_edit(store, revision.artifact_id)?;
    let updated_content = apply_targeted_change(&artifact_before.content, &revision)?;

    let version_snapshot = store.create_artifact_version(&NewArtifactVersionInput {
        task_id: revision.task_id,
        artifact_id: revision.artifact_id,
        revision_id: Some(revision.revision_id),
        version_reason: format!("before_apply_revision_{}", revision.revision_id),
        content_snapshot: artifact_before.content.clone(),
        stored_path: artifact_before.stored_path.clone(),
    })?;

    fs::write(&artifact_before.stored_path, &updated_content).with_context(|| {
        format!(
            "failed to write accepted revision to artifact path {}",
            artifact_before.stored_path
        )
    })?;

    store.touch_artifact_updated_at(revision.artifact_id)?;
    reindex_artifact_content(
        store,
        embeddings,
        revision.task_id,
        revision.artifact_id,
        &updated_content,
    )?;

    let accepted_revision = store.set_revision_status(revision.revision_id, "accepted")?;
    let artifact_after = get_artifact_content_for_edit(store, revision.artifact_id)?;

    store.append_chat_message(
        revision.task_id,
        "assistant",
        "assistant",
        MessageKind::AssistantRevisionStatus,
        &format!(
            "Applied revision #{} and saved a version snapshot.",
            revision.revision_id
        ),
    )?;

    Ok(RevisionApplyResultDto {
        revision: accepted_revision,
        artifact_content: artifact_after,
        version_snapshot,
    })
}

pub fn reject_revision(store: &TaskStore, revision_id: i64) -> Result<RevisionProposalDto> {
    let revision = store
        .get_revision_by_id(revision_id)?
        .ok_or_else(|| anyhow!("revision id={} not found", revision_id))?;

    if revision.status != "pending" {
        return Err(anyhow!(
            "revision id={} is not pending (status={})",
            revision_id,
            revision.status
        ));
    }

    let rejected = store.set_revision_status(revision_id, "rejected")?;

    store.append_chat_message(
        rejected.task_id,
        "assistant",
        "assistant",
        MessageKind::AssistantRevisionStatus,
        &format!("Rejected revision #{}.", rejected.revision_id),
    )?;

    Ok(rejected)
}

pub fn list_artifact_versions_for_artifact(
    store: &TaskStore,
    artifact_id: i64,
) -> Result<Vec<ArtifactVersionDto>> {
    store.list_artifact_versions(artifact_id)
}

pub fn revert_artifact_to_version(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    version_id: i64,
) -> Result<ArtifactContentDto> {
    let version = store
        .get_artifact_version_snapshot(version_id)?
        .ok_or_else(|| anyhow!("version id={} not found", version_id))?;

    let current = get_artifact_content_for_edit(store, version.dto.artifact_id)?;

    store.create_artifact_version(&NewArtifactVersionInput {
        task_id: version.dto.task_id,
        artifact_id: version.dto.artifact_id,
        revision_id: None,
        version_reason: format!("before_revert_to_version_{}", version.dto.version_id),
        content_snapshot: current.content.clone(),
        stored_path: current.stored_path.clone(),
    })?;

    fs::write(&version.stored_path, &version.content_snapshot).with_context(|| {
        format!(
            "failed to write version snapshot back to artifact path {}",
            version.stored_path
        )
    })?;

    store.touch_artifact_updated_at(version.dto.artifact_id)?;
    reindex_artifact_content(
        store,
        embeddings,
        version.dto.task_id,
        version.dto.artifact_id,
        &version.content_snapshot,
    )?;

    store.append_chat_message(
        version.dto.task_id,
        "assistant",
        "assistant",
        MessageKind::AssistantRevisionStatus,
        &format!("Reverted artifact to version #{}.", version.dto.version_id),
    )?;

    get_artifact_content_for_edit(store, version.dto.artifact_id)
}

fn reindex_artifact_content(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    task_id: i64,
    artifact_id: i64,
    content: &str,
) -> Result<()> {
    let mut raw_chunks = chunk_text(
        content,
        DEFAULT_CHUNK_SIZE_CHARS,
        DEFAULT_CHUNK_OVERLAP_CHARS,
    );
    if raw_chunks.is_empty() {
        raw_chunks.push(content.to_string());
    }

    let mut rows = Vec::new();
    for (index, chunk) in raw_chunks.iter().enumerate() {
        let embedding = embeddings
            .embed_text(chunk)
            .with_context(|| format!("failed to embed replacement chunk index {}", index))?;

        rows.push(ChunkEmbeddingInput {
            chunk_text: chunk.to_string(),
            position_index: index as i64,
            embedding,
        });
    }

    store.replace_artifact_chunks(task_id, artifact_id, &rows)
}

fn normalize_instruction_source(source: &str) -> &'static str {
    match source.trim().to_ascii_lowercase().as_str() {
        "voice" => "voice",
        "system" => "system",
        _ => "typed",
    }
}

fn parse_generated_revision(
    raw_candidate: &str,
    fallback_text: &str,
    retrieval_confidence: f32,
    weak_context: bool,
) -> GeneratedRevision {
    let parsed = serde_json::from_str::<GeneratedRevisionJson>(raw_candidate.trim());

    let (mut proposed_text, mut rationale, json_confidence, grounding_notes) = match parsed {
        Ok(value) => (
            character::strip_filler_phrases(value.proposed_text.trim()),
            value
                .rationale
                .map(|value| character::strip_filler_phrases(value.trim()))
                .filter(|value| !value.is_empty()),
            value.confidence,
            value
                .grounding_notes
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        ),
        Err(_) => (
            character::strip_filler_phrases(raw_candidate.trim()),
            None,
            None,
            None,
        ),
    };

    if rationale.is_none() {
        let extracted = extract_assessment_sentence(raw_candidate)
            .or_else(|| extract_assessment_sentence(&proposed_text));
        if let Some(assessment) = extracted {
            if let Some(remainder) = proposed_text.strip_prefix(&assessment) {
                let remainder = remainder.trim_start();
                if !remainder.is_empty() {
                    rationale = Some(assessment);
                    proposed_text = remainder.to_string();
                }
            }
        }
    }

    if proposed_text.is_empty() {
        proposed_text = fallback_text.to_string();
    }

    let mut confidence = json_confidence.unwrap_or(retrieval_confidence);
    if weak_context {
        confidence = confidence.min(0.35);
    }

    GeneratedRevision {
        proposed_text,
        rationale,
        grounding_notes: Some(grounding_notes.unwrap_or_else(|| {
            if weak_context {
                "Weak grounding: retrieved context was limited; revision is conservative."
                    .to_string()
            } else {
                "Grounded using task summary, retrieved chunks, and target text.".to_string()
            }
        })),
        confidence: confidence.clamp(0.0, 1.0),
    }
}

fn build_revision_prompt(
    task_summary: &str,
    instruction: &str,
    artifact_content: &str,
    target: &ResolvedTarget,
    recent_messages: &str,
    retrieved_chunks: &str,
) -> String {
    let artifact_preview = artifact_content.chars().take(2500).collect::<String>();

    format!(
        "Task summary:\n{}\n\nInstruction:\n{}\n\nTarget description:\n{}\nTarget range (char offsets): {}..{}\n\nTarget original text:\n{}\n\nRecent session context:\n{}\n\nRetrieved grounding chunks:\n{}\n\nActive artifact preview:\n{}\n\nReturn strict JSON only.",
        task_summary,
        instruction,
        target.target_description,
        target.start_offset,
        target.end_offset,
        target.original_text,
        if recent_messages.is_empty() { "<none>" } else { recent_messages },
        if retrieved_chunks.is_empty() {
            "<none>"
        } else {
            retrieved_chunks
        },
        artifact_preview
    )
}

fn build_revision_system_blocks(
    store: &TaskStore,
    task_summary: &str,
    target_description: &str,
    instruction: &str,
    snapshot_summary: Option<&str>,
) -> Vec<SystemBlock> {
    let profile_injection = if store
        .get_privacy_user_profile_memory_enabled()
        .unwrap_or(false)
    {
        user_model::build_profile_injection(store)
    } else {
        None
    };
    let prefers_opinions = if store
        .get_privacy_user_profile_memory_enabled()
        .unwrap_or(false)
    {
        relational_model::get_collaboration_style(store)
            .ok()
            .map(|style| style.prefers_opinions)
    } else {
        None
    };

    character::build_revision_system_blocks(&RevisionContext {
        task_summary: task_summary.to_string(),
        target_description: target_description.to_string(),
        instruction: instruction.to_string(),
        profile_injection,
        prefers_opinions,
        snapshot_summary: snapshot_summary.map(|s| s.to_string()),
    })
}

fn resolve_target(content: &str, target: Option<&RevisionTargetDto>) -> Result<ResolvedTarget> {
    if content.trim().is_empty() {
        return Err(anyhow!(
            "artifact content is empty; cannot propose revision"
        ));
    }

    if let Some(target) = target {
        let start = target.start_offset.unwrap_or(0);
        let end = target.end_offset.unwrap_or(start);

        if end > start {
            let start_char = clamp_char_offset(content, start);
            let end_char = clamp_char_offset(content, end);
            let (start_char, end_char) = if end_char > start_char {
                (start_char, end_char)
            } else {
                (start_char, (start_char + 1).min(content.chars().count()))
            };

            let original_text = slice_by_char_offsets(content, start_char, end_char)?;
            return Ok(ResolvedTarget {
                start_offset: start_char,
                end_offset: end_char,
                selection_source: "explicit_range".to_string(),
                target_description: format!("selected range {}..{}", start_char, end_char),
                original_text,
            });
        }

        let cursor = clamp_char_offset(content, start);
        let (para_start, para_end) = find_paragraph_bounds(content, cursor);
        let original_text = slice_by_char_offsets(content, para_start, para_end)?;

        return Ok(ResolvedTarget {
            start_offset: para_start,
            end_offset: para_end,
            selection_source: "cursor_paragraph".to_string(),
            target_description: format!("paragraph around cursor {}", cursor),
            original_text,
        });
    }

    let (start, end) = find_paragraph_bounds(content, 0);
    let original_text = slice_by_char_offsets(content, start, end)?;

    Ok(ResolvedTarget {
        start_offset: start,
        end_offset: end,
        selection_source: "default_first_paragraph".to_string(),
        target_description: "first paragraph".to_string(),
        original_text,
    })
}

fn apply_targeted_change(content: &str, revision: &RevisionProposalDto) -> Result<String> {
    let start_char = clamp_char_offset(content, revision.target_start_offset);
    let end_char = clamp_char_offset(content, revision.target_end_offset);

    if end_char > start_char {
        let current_target = slice_by_char_offsets(content, start_char, end_char)?;
        if current_target == revision.original_text {
            return replace_by_char_offsets(content, start_char, end_char, &revision.proposed_text);
        }
    }

    if let Some(found_at) = content.find(&revision.original_text) {
        let found_start = content[..found_at].chars().count();
        let found_end = found_start + revision.original_text.chars().count();
        return replace_by_char_offsets(content, found_start, found_end, &revision.proposed_text);
    }

    Err(anyhow!(
        "could not apply revision id={} because target text no longer matches artifact",
        revision.revision_id
    ))
}

fn replace_by_char_offsets(
    content: &str,
    start_char: usize,
    end_char: usize,
    replacement: &str,
) -> Result<String> {
    let start_byte = char_to_byte_offset(content, start_char);
    let end_byte = char_to_byte_offset(content, end_char);

    if end_byte < start_byte || end_byte > content.len() {
        return Err(anyhow!("invalid replacement bounds"));
    }

    let mut updated = String::new();
    updated.push_str(&content[..start_byte]);
    updated.push_str(replacement);
    updated.push_str(&content[end_byte..]);
    Ok(updated)
}

fn slice_by_char_offsets(content: &str, start_char: usize, end_char: usize) -> Result<String> {
    let start_byte = char_to_byte_offset(content, start_char);
    let end_byte = char_to_byte_offset(content, end_char);

    if end_byte < start_byte || end_byte > content.len() {
        return Err(anyhow!("invalid slice bounds"));
    }

    Ok(content[start_byte..end_byte].to_string())
}

fn find_paragraph_bounds(content: &str, cursor_char: usize) -> (usize, usize) {
    let cursor_byte = char_to_byte_offset(content, cursor_char.min(content.chars().count()));
    let start_byte = content[..cursor_byte]
        .rfind("\n\n")
        .map(|value| value + 2)
        .unwrap_or(0);
    let end_byte = content[cursor_byte..]
        .find("\n\n")
        .map(|value| cursor_byte + value)
        .unwrap_or(content.len());

    let start_char = content[..start_byte].chars().count();
    let end_char = content[..end_byte].chars().count();
    (
        start_char,
        end_char.max(start_char + 1).min(content.chars().count()),
    )
}

fn clamp_char_offset(content: &str, offset: i64) -> usize {
    let total = content.chars().count();
    if offset <= 0 {
        0
    } else {
        (offset as usize).min(total)
    }
}

fn char_to_byte_offset(content: &str, char_index: usize) -> usize {
    if char_index == 0 {
        return 0;
    }

    for (current, (byte_idx, _)) in content.char_indices().enumerate() {
        if current == char_index {
            return byte_idx;
        }
    }

    content.len()
}

/// Extracts a first-person assessment sentence from LLM output.
/// Heuristic: the first sentence (up to `.`, `?`, or `!`) qualifies when it:
/// - is under 120 characters
/// - contains no markdown formatting (no `*`, `#`, `:` pairs, or backticks)
/// - contains a first-person pronoun ("I " or "my ")
pub fn extract_assessment_sentence(output: &str) -> Option<String> {
    let trimmed = output.trim();
    let end = trimmed
        .find(|c| c == '.' || c == '?' || c == '!')
        .map(|i| i + 1)
        .unwrap_or(trimmed.len());
    let sentence = trimmed[..end].trim();

    if sentence.is_empty() || sentence.len() > 120 {
        return None;
    }

    // reject sentences with markdown structural markers
    if sentence.contains('*')
        || sentence.contains('#')
        || sentence.contains('`')
        || sentence.contains("**")
    {
        return None;
    }

    let lower = sentence.to_ascii_lowercase();
    if lower.contains("i ")
        || lower.contains("i'm")
        || lower.contains("i've")
        || lower.starts_with("i ")
        || lower.contains(" my ")
        || lower.starts_with("my ")
    {
        Some(sentence.to_string())
    } else {
        None
    }
}

pub fn generate_revision_alternative(
    store: &TaskStore,
    reasoning: &dyn crate::reasoning::ReasoningProvider,
    task_id: i64,
    revision_id: i64,
    snapshot_summary: Option<&str>,
) -> Result<RevisionProposalDto> {
    let original = store
        .get_revision_by_id(revision_id)?
        .ok_or_else(|| anyhow!("revision id={} not found", revision_id))?;

    if original.task_id != task_id {
        return Err(anyhow!(
            "revision id={} does not belong to task id={}",
            revision_id,
            task_id
        ));
    }

    // only allow generating an alternative for an original (not already an alternative)
    if original.parent_revision_id.is_some() {
        return Err(anyhow!(
            "revision id={} is already an alternative; cannot generate alternative of an alternative",
            revision_id
        ));
    }

    // verify no alternative already exists
    let existing = store.list_alternative_revisions(revision_id)?;
    if !existing.is_empty() {
        return Err(anyhow!(
            "revision id={} already has an alternative (id={})",
            revision_id,
            existing[0].revision_id
        ));
    }

    let prior_rationale = original.rationale.as_deref().unwrap_or("a direct approach");

    let artifact = get_artifact_content_for_edit(store, original.artifact_id)?;
    let profile_injection = if store
        .get_privacy_user_profile_memory_enabled()
        .unwrap_or(false)
    {
        user_model::build_profile_injection(store)
    } else {
        None
    };
    let prefers_opinions = if store
        .get_privacy_user_profile_memory_enabled()
        .unwrap_or(false)
    {
        relational_model::get_collaboration_style(store)
            .ok()
            .map(|style| style.prefers_opinions)
    } else {
        None
    };

    let system_blocks = character::build_revision_system_blocks(&character::RevisionContext {
        task_summary: artifact.file_name.clone(),
        target_description: original.target_description.clone(),
        instruction: format!(
            "Generate an ALTERNATIVE approach to what was described in the prior assessment: \"{}\". \
             The prior revision took that path. Now take a meaningfully different path — different structure, \
             different emphasis, or different tradeoff. Your assessment sentence must describe what makes \
             this approach different.",
            prior_rationale
        ),
        profile_injection,
        prefers_opinions,
        snapshot_summary: snapshot_summary.map(|s| s.to_string()),
    });

    let user_prompt = format!(
        "Original text:\n{}\n\nPrior proposed revision:\n{}\n\nPrior assessment: {}\n\nNow produce an alternative approach. Return strict JSON only.",
        original.original_text,
        original.proposed_text,
        prior_rationale
    );

    let raw_candidate = reasoning.generate_response_blocks(&system_blocks, &user_prompt)?;
    let generated = parse_generated_revision(&raw_candidate, &original.proposed_text, 0.5, false);

    let proposal = store.create_revision_proposal(&NewRevisionProposalInput {
        task_id,
        artifact_id: original.artifact_id,
        target_start_offset: original.target_start_offset,
        target_end_offset: original.target_end_offset,
        target_description: original.target_description.clone(),
        original_text: original.original_text.clone(),
        proposed_text: generated.proposed_text,
        instruction_text: format!("alternative to revision #{}", revision_id),
        instruction_source: "system".to_string(),
        rationale: generated.rationale,
        grounding_notes: generated.grounding_notes,
        retrieval_confidence: generated.confidence,
        parent_revision_id: Some(revision_id),
    })?;

    Ok(proposal)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use anyhow::Result;

    use crate::{
        chat::send_message_for_task,
        coworking::{evaluate_proactive_nudge_for_task, CoworkingRuntime},
        embedding::EmbeddingProvider,
        message_kind::MessageKind,
        reasoning::ReasoningProvider,
        retrieval::{import_artifact_for_task, retrieve_relevant_chunks},
        store::TaskStore,
    };

    use super::{
        apply_revision, build_revision_prompt, get_artifact_content_for_edit,
        list_artifact_versions_for_artifact, parse_generated_revision, propose_artifact_revision,
        reject_revision, resolve_target, revert_artifact_to_version, RevisionTargetDto,
    };

    #[derive(Clone)]
    struct KeywordEmbeddingProvider;

    impl EmbeddingProvider for KeywordEmbeddingProvider {
        fn embed_text(&self, input: &str) -> Result<Vec<f32>> {
            let lower = input.to_lowercase();
            let score = |terms: &[&str]| -> f32 {
                terms
                    .iter()
                    .map(|term| lower.matches(term).count() as f32)
                    .sum()
            };

            Ok(vec![
                score(&["primary", "source", "evidence"]),
                score(&["citizenship", "debates", "history", "analytical"]),
                score(&["thesis", "intro", "introduction"]),
                score(&["reading", "readings", "course"]),
                (lower.len() as f32) / 1000.0,
            ])
        }
    }

    #[derive(Clone)]
    struct RevisionReasoningProvider;

    impl ReasoningProvider for RevisionReasoningProvider {
        fn generate_response(&self, _system_prompt: &str, user_prompt: &str) -> Result<String> {
            let lower = user_prompt.to_lowercase();

            if lower.contains("tighten this thesis")
                || lower.contains("broader citizenship debates")
            {
                return Ok(
                    r#"{"proposed_text":"This thesis argues that contested definitions of citizenship shaped political participation, linking local events to broader civic debates.","rationale":"Tightened scope and explicit civic frame.","confidence":0.81,"grounding_notes":"Linked thesis language to retrieved citizenship framing."}"#
                        .to_string(),
                );
            }

            if lower.contains("make this more analytical") {
                return Ok(
                    r#"{"proposed_text":"Rather than only describing events, this section analyzes how competing citizenship claims influenced policy decisions and social inclusion.","rationale":"Shifted from summary to analysis.","confidence":0.78,"grounding_notes":"Uses rubric emphasis on analysis and evidence."}"#
                        .to_string(),
                );
            }

            if lower.contains("primary source requirement") {
                return Ok("Use primary sources, course readings, and evidence requirements from the rubric.".to_string());
            }

            Ok(
                r#"{"proposed_text":"This paragraph can be made more analytical by linking claims directly to evidence and course framing.","rationale":"Conservative fallback.","confidence":0.52,"grounding_notes":"General grounded fallback."}"#
                    .to_string(),
            )
        }
    }

    #[derive(Clone)]
    struct NudgeReasoningProvider;

    impl ReasoningProvider for NudgeReasoningProvider {
        fn generate_response(&self, _system_prompt: &str, _user_prompt: &str) -> Result<String> {
            Ok(
                "Your draft still needs a primary source tied to course readings in the intro."
                    .to_string(),
            )
        }
    }

    fn write_file(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("failed to create parent directory");
        }

        fs::write(path, body).expect("failed to write file");
    }

    fn setup_revision_fixture() -> (tempfile::TempDir, TaskStore, i64, i64) {
        let temp = tempfile::tempdir().expect("failed to create temp directory");
        let base = temp.path().join("app_local_data");
        let store = TaskStore::initialize(&base).expect("failed to initialize store");

        let task = store
            .create_task("history storymap")
            .expect("failed to create task");
        store
            .set_active_task(task.id)
            .expect("failed to set active task");

        let notes = temp.path().join("fixtures").join("notes.md");
        write_file(
            &notes,
            "The intro summarizes events.\n\nThe thesis is broad and needs clearer citizenship framing.\n\nEvidence should tie to course readings and primary sources.",
        );

        let rubric = temp.path().join("fixtures").join("rubric.txt");
        write_file(
            &rubric,
            "Rubric: each section should use primary source evidence, connect to course readings, and develop analytical arguments about citizenship.",
        );

        let imported_notes = import_artifact_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task.id,
            &notes.to_string_lossy(),
        )
        .expect("failed to import notes artifact");

        import_artifact_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task.id,
            &rubric.to_string_lossy(),
        )
        .expect("failed to import rubric artifact");

        (temp, store, task.id, imported_notes.id)
    }

    #[test]
    fn revision_object_lifecycle_pending_accept_reject_statuses() {
        let (_temp, store, task_id, artifact_id) = setup_revision_fixture();

        let proposal = propose_artifact_revision(
            &store,
            &KeywordEmbeddingProvider,
            &RevisionReasoningProvider,
            task_id,
            artifact_id,
            Some(RevisionTargetDto {
                start_offset: Some(0),
                end_offset: Some(32),
            }),
            "make this more analytical",
            "typed",
            None,
        )
        .expect("failed to propose revision")
        .proposal;

        assert_eq!(proposal.status, "pending");

        let applied = apply_revision(&store, &KeywordEmbeddingProvider, proposal.revision_id)
            .expect("failed to apply revision");
        assert_eq!(applied.revision.status, "accepted");

        let second = propose_artifact_revision(
            &store,
            &KeywordEmbeddingProvider,
            &RevisionReasoningProvider,
            task_id,
            artifact_id,
            None,
            "make this more analytical",
            "typed",
            None,
        )
        .expect("failed to propose second revision")
        .proposal;

        let rejected =
            reject_revision(&store, second.revision_id).expect("failed to reject revision");
        assert_eq!(rejected.status, "rejected");
    }

    #[test]
    fn range_targeting_extracts_correct_original_text() {
        let (_temp, store, task_id, artifact_id) = setup_revision_fixture();
        let artifact =
            get_artifact_content_for_edit(&store, artifact_id).expect("failed to load artifact");

        let target_text = "thesis is broad";
        let start = artifact
            .content
            .find(target_text)
            .expect("failed to find target text") as i64;
        let end = start + target_text.len() as i64;

        let result = propose_artifact_revision(
            &store,
            &KeywordEmbeddingProvider,
            &RevisionReasoningProvider,
            task_id,
            artifact_id,
            Some(RevisionTargetDto {
                start_offset: Some(start),
                end_offset: Some(end),
            }),
            "tighten this thesis and connect it to broader citizenship debates",
            "typed",
            None,
        )
        .expect("failed to propose targeted revision");

        assert!(result.proposal.original_text.contains("thesis is broad"));
        assert_eq!(result.used_start_offset, start);
        assert_eq!(result.used_end_offset, end);
    }

    #[test]
    fn grounding_prompt_builder_contains_context_target_and_instruction() {
        let target = resolve_target(
            "Paragraph one.\n\nParagraph two about citizenship.",
            Some(&RevisionTargetDto {
                start_offset: Some(14),
                end_offset: Some(28),
            }),
        )
        .expect("failed to resolve target");

        let prompt = build_revision_prompt(
            "Task summary text",
            "make this more analytical",
            "Paragraph one.\n\nParagraph two about citizenship.",
            &target,
            "user (text): improve this",
            "Chunk 1 | rubric | score 0.8",
        );

        assert!(prompt.contains("Task summary text"));
        assert!(prompt.contains("make this more analytical"));
        assert!(prompt.contains("Target original text"));
        assert!(prompt.contains("Chunk 1 | rubric"));
    }

    #[test]
    fn versioning_and_revert_restore_exact_prior_content() {
        let (_temp, store, task_id, artifact_id) = setup_revision_fixture();

        let before = get_artifact_content_for_edit(&store, artifact_id)
            .expect("failed to load before content");

        let proposal = propose_artifact_revision(
            &store,
            &KeywordEmbeddingProvider,
            &RevisionReasoningProvider,
            task_id,
            artifact_id,
            None,
            "make this more analytical",
            "typed",
            None,
        )
        .expect("failed to propose revision")
        .proposal;

        let applied = apply_revision(&store, &KeywordEmbeddingProvider, proposal.revision_id)
            .expect("failed to apply revision");
        assert_ne!(applied.artifact_content.content, before.content);

        let versions = list_artifact_versions_for_artifact(&store, artifact_id)
            .expect("failed to list artifact versions");
        assert!(!versions.is_empty());

        let restored =
            revert_artifact_to_version(&store, &KeywordEmbeddingProvider, versions[0].version_id)
                .expect("failed to revert artifact version");
        assert_eq!(restored.content, before.content);
    }

    #[test]
    fn reject_path_preserves_artifact_content() {
        let (_temp, store, task_id, artifact_id) = setup_revision_fixture();

        let before = get_artifact_content_for_edit(&store, artifact_id)
            .expect("failed to load before content");

        let proposal = propose_artifact_revision(
            &store,
            &KeywordEmbeddingProvider,
            &RevisionReasoningProvider,
            task_id,
            artifact_id,
            None,
            "tighten this thesis and connect it to broader citizenship debates",
            "typed",
            None,
        )
        .expect("failed to propose revision")
        .proposal;

        reject_revision(&store, proposal.revision_id).expect("failed to reject revision");

        let after = get_artifact_content_for_edit(&store, artifact_id)
            .expect("failed to load after content");
        assert_eq!(after.content, before.content);
    }

    #[test]
    fn voice_triggered_revision_creates_proposal_with_voice_source() {
        let (_temp, store, task_id, artifact_id) = setup_revision_fixture();

        let result = propose_artifact_revision(
            &store,
            &KeywordEmbeddingProvider,
            &RevisionReasoningProvider,
            task_id,
            artifact_id,
            None,
            "tighten this thesis and connect it to broader citizenship debates",
            "voice",
            None,
        )
        .expect("failed to propose voice revision");

        assert_eq!(result.proposal.instruction_source, "voice");
        assert_eq!(result.proposal.status, "pending");
    }

    #[test]
    fn message_typing_distinguishes_revision_outputs() {
        let (_temp, store, task_id, artifact_id) = setup_revision_fixture();

        let result = propose_artifact_revision(
            &store,
            &KeywordEmbeddingProvider,
            &RevisionReasoningProvider,
            task_id,
            artifact_id,
            None,
            "make this more analytical",
            "typed",
            None,
        )
        .expect("failed to propose revision");

        let _ = apply_revision(
            &store,
            &KeywordEmbeddingProvider,
            result.proposal.revision_id,
        )
        .expect("failed to apply revision");

        let messages = store
            .list_chat_messages(task_id)
            .expect("failed to list chat messages");

        assert!(
            messages
                .iter()
                .any(|message| message.message_kind
                    == MessageKind::AssistantRevisionProposal.as_str())
        );
        assert!(messages
            .iter()
            .any(|message| message.message_kind == MessageKind::AssistantRevisionStatus.as_str()));
    }

    #[test]
    fn history_storymap_instruction_make_more_analytical_creates_grounded_pending_revision() {
        let (_temp, store, task_id, artifact_id) = setup_revision_fixture();

        let result = propose_artifact_revision(
            &store,
            &KeywordEmbeddingProvider,
            &RevisionReasoningProvider,
            task_id,
            artifact_id,
            None,
            "make this section more analytical",
            "typed",
            None,
        )
        .expect("failed to propose analytical revision");

        assert_eq!(result.proposal.status, "pending");
        let lower = result.proposal.proposed_text.to_lowercase();
        assert!(lower.contains("citizenship") || lower.contains("analy"));
        assert!(!result.retrieved_chunks.is_empty());
    }

    #[test]
    fn history_storymap_instruction_tighten_thesis_keeps_original_until_accept() {
        let (_temp, store, task_id, artifact_id) = setup_revision_fixture();

        let before =
            get_artifact_content_for_edit(&store, artifact_id).expect("failed to load content");

        let result = propose_artifact_revision(
            &store,
            &KeywordEmbeddingProvider,
            &RevisionReasoningProvider,
            task_id,
            artifact_id,
            None,
            "tighten this thesis and connect it to broader citizenship debates",
            "typed",
            None,
        )
        .expect("failed to propose thesis revision");

        assert_eq!(result.proposal.status, "pending");
        assert!(result
            .proposal
            .proposed_text
            .to_lowercase()
            .contains("citizenship"));

        let after =
            get_artifact_content_for_edit(&store, artifact_id).expect("failed to reload content");
        assert_eq!(before.content, after.content);
    }

    #[test]
    fn ask_answer_and_proactive_runtime_still_work_after_revision_flow() {
        let (_temp, store, task_id, artifact_id) = setup_revision_fixture();

        let proposed = propose_artifact_revision(
            &store,
            &KeywordEmbeddingProvider,
            &RevisionReasoningProvider,
            task_id,
            artifact_id,
            None,
            "make this section more analytical",
            "typed",
            None,
        )
        .expect("failed to propose revision");

        let _ = apply_revision(
            &store,
            &KeywordEmbeddingProvider,
            proposed.proposal.revision_id,
        )
        .expect("failed to apply revision");

        let answer = send_message_for_task(
            &store,
            &KeywordEmbeddingProvider,
            &RevisionReasoningProvider,
            task_id,
            "What is the primary source requirement?",
            "text",
            None,
            None,
            || false,
        )
        .expect("failed to send ask/answer message");

        assert!(answer
            .assistant_response
            .to_lowercase()
            .contains("primary source"));

        let mut runtime = CoworkingRuntime::default();
        runtime.note_user_message(MessageKind::UserStatement, 0);
        let proactive = evaluate_proactive_nudge_for_task(
            &store,
            &KeywordEmbeddingProvider,
            &NudgeReasoningProvider,
            &mut runtime,
            task_id,
            20,
        )
        .expect("failed proactive evaluation");

        assert!(proactive.nudge.is_some() || proactive.decision_reason.contains("pause"));
    }

    #[test]
    fn regression_retrieval_query_still_returns_primary_source_chunks_after_revision() {
        let (_temp, store, task_id, artifact_id) = setup_revision_fixture();

        let proposal = propose_artifact_revision(
            &store,
            &KeywordEmbeddingProvider,
            &RevisionReasoningProvider,
            task_id,
            artifact_id,
            None,
            "make this section more analytical",
            "typed",
            None,
        )
        .expect("failed to propose revision");
        let _ = apply_revision(
            &store,
            &KeywordEmbeddingProvider,
            proposal.proposal.revision_id,
        )
        .expect("failed to apply revision");

        let chunks = retrieve_relevant_chunks(
            &store,
            &KeywordEmbeddingProvider,
            task_id,
            "primary source requirement",
        )
        .expect("failed to retrieve regression query chunks");

        assert!(!chunks.is_empty());
        let combined = chunks
            .iter()
            .map(|chunk| chunk.chunk_text.to_lowercase())
            .collect::<Vec<String>>()
            .join("\n");
        assert!(combined.contains("primary") || combined.contains("evidence"));
    }

    #[test]
    fn parse_generated_revision_handles_non_json_and_weak_context() {
        let parsed = parse_generated_revision(
            "This is plain text proposal",
            "fallback original",
            0.9,
            true,
        );

        assert_eq!(parsed.proposed_text, "This is plain text proposal");
        assert!(parsed.confidence <= 0.35);
    }

    #[test]
    fn parse_generated_revision_extracts_assessment_from_plain_text() {
        let parsed = parse_generated_revision(
            "I moved the claim forward. Revised paragraph text.",
            "fallback original",
            0.9,
            false,
        );

        assert_eq!(
            parsed.rationale.as_deref(),
            Some("I moved the claim forward.")
        );
        assert_eq!(parsed.proposed_text, "Revised paragraph text.");
    }

    #[test]
    fn extract_assessment_sentence_extracts_first_person_sentence() {
        let input =
            "I moved the argument to the front. The conclusion now:\n\nYour new argument here.";
        let result = super::extract_assessment_sentence(input);
        assert_eq!(
            result,
            Some("I moved the argument to the front.".to_string())
        );
    }

    #[test]
    fn extract_assessment_sentence_returns_none_for_no_first_person() {
        let input = "The revision restructures the paragraph. Original below.";
        let result = super::extract_assessment_sentence(input);
        assert!(result.is_none());
    }

    #[test]
    fn revision_system_prompt_includes_assessment_instruction() {
        use crate::character::{self, RevisionContext};
        let blocks = character::build_revision_system_blocks(&RevisionContext {
            task_summary: "test task".to_string(),
            target_description: "intro paragraph".to_string(),
            instruction: "tighten this".to_string(),
            profile_injection: None,
            prefers_opinions: None,
            snapshot_summary: None,
        });
        let prompt = crate::model_router::join_system_blocks(&blocks);
        let lower = prompt.to_ascii_lowercase();
        // the assessment instruction tells jeff to lead with the judgment it made
        assert!(
            lower.contains("assessment")
                || lower.contains("tradeoff")
                || lower.contains("before presenting"),
            "revision system prompt does not contain the assessment instruction"
        );
        assert!(
            lower.contains("do not invent")
                && lower.contains("remove the weakness")
                && lower.contains("false specificity"),
            "revision system prompt does not include grounding and instruction-literal guardrails"
        );
    }
}
