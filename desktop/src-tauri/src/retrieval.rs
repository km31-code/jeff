use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

use anyhow::{anyhow, Context, Result};

use crate::{
    artifact_parser::{parse_text_from_artifact, supported_artifact_type, SupportedArtifactType},
    chunking::{chunk_text, DEFAULT_CHUNK_OVERLAP_CHARS, DEFAULT_CHUNK_SIZE_CHARS},
    embedding::EmbeddingProvider,
    local_runtime::LocalRuntime,
    models::{ArtifactDto, ContextArtifactDto, RetrievedChunkDto, TaskContextPackDto},
    providers::local::LocalEmbeddingProvider,
    similarity::cosine_similarity,
    store::{ChunkEmbeddingInput, StoredChunkEmbedding, TaskStore},
};

const DEFAULT_TOP_K: usize = 5;

pub fn import_artifact_for_task(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    task_id: i64,
    file_path: &str,
) -> Result<ArtifactDto> {
    let source_path = PathBuf::from(file_path.trim());
    if source_path.as_os_str().is_empty() {
        return Err(anyhow!("artifact path cannot be empty"));
    }

    if !source_path.exists() {
        return Err(anyhow!(
            "artifact path does not exist: {}",
            source_path.display()
        ));
    }

    if !source_path.is_file() {
        return Err(anyhow!(
            "artifact path is not a file: {}",
            source_path.display()
        ));
    }

    let canonical_source = fs::canonicalize(&source_path)
        .with_context(|| format!("failed to canonicalize path {}", source_path.display()))?;

    let artifact_type = supported_artifact_type(&canonical_source)?;
    let parsed_text = parse_text_from_artifact(&canonical_source)?;

    let raw_chunks = chunk_text(
        &parsed_text,
        DEFAULT_CHUNK_SIZE_CHARS,
        DEFAULT_CHUNK_OVERLAP_CHARS,
    );

    if raw_chunks.is_empty() {
        return Err(anyhow!("artifact text did not produce any chunks"));
    }

    let mut chunk_rows = Vec::with_capacity(raw_chunks.len());
    for (index, chunk) in raw_chunks.iter().enumerate() {
        let embedding = embeddings
            .embed_text(chunk)
            .with_context(|| format!("failed to embed chunk index {}", index))?;

        if embedding.is_empty() {
            return Err(anyhow!(
                "embedding provider returned an empty vector for chunk {index}"
            ));
        }

        chunk_rows.push(ChunkEmbeddingInput {
            chunk_text: chunk.to_string(),
            position_index: index as i64,
            embedding,
            embedding_model: embeddings.model_id().to_string(),
        });
    }

    let workspace_path = store.get_task_workspace_path(task_id)?;
    let artifact_dir = workspace_path.join("artifacts");
    fs::create_dir_all(&artifact_dir).with_context(|| {
        format!(
            "failed to create artifact directory {}",
            artifact_dir.display()
        )
    })?;

    let file_name = canonical_source
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow!("artifact file name is invalid utf-8"))?
        .to_string();

    let sanitized_name = sanitize_file_name(&file_name);
    let destination_path = next_available_artifact_path(&artifact_dir, &sanitized_name);

    fs::copy(&canonical_source, &destination_path).with_context(|| {
        format!(
            "failed to copy artifact from {} to {}",
            canonical_source.display(),
            destination_path.display()
        )
    })?;

    let file_extension = match artifact_type {
        SupportedArtifactType::Markdown => "md",
        SupportedArtifactType::Text => "txt",
        SupportedArtifactType::Pdf => "pdf",
    }
    .to_string();

    store.insert_artifact_with_chunks(
        task_id,
        &file_name,
        &file_extension,
        &canonical_source.to_string_lossy(),
        &destination_path.to_string_lossy(),
        &chunk_rows,
    )
}

pub fn retrieve_relevant_chunks(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    task_id: i64,
    query: &str,
) -> Result<Vec<RetrievedChunkDto>> {
    retrieve_relevant_chunks_with_top_k(store, embeddings, task_id, query, DEFAULT_TOP_K)
}

pub fn retrieve_relevant_chunks_with_top_k(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    task_id: i64,
    query: &str,
    top_k: usize,
) -> Result<Vec<RetrievedChunkDto>> {
    let clean_query = query.trim();
    if clean_query.is_empty() {
        return Err(anyhow!("retrieval query cannot be empty"));
    }

    let stored_chunks = store.fetch_chunk_embeddings_for_task(task_id)?;
    if stored_chunks.is_empty() {
        return Ok(Vec::new());
    }

    let active_embedding_model = embeddings.model_id();
    let mut normalized_chunks = Vec::with_capacity(stored_chunks.len());
    for mut chunk in stored_chunks {
        if chunk.embedding_model != active_embedding_model {
            // apex b1: re-embedding a stale chunk is best-effort. a transient
            // embedding failure (e.g. the local sidecar hiccuping) must not
            // fail the whole retrieval — keep the stale vector, which is still
            // usable, and re-migrate on a later touch.
            match embeddings.embed_text(&chunk.chunk_text) {
                Ok(refreshed) if !refreshed.is_empty() => {
                    store.update_chunk_embedding(
                        chunk.chunk_id,
                        &refreshed,
                        active_embedding_model,
                    )?;
                    chunk.embedding = refreshed;
                    chunk.embedding_model = active_embedding_model.to_string();
                }
                Ok(_) => {}
                Err(err) => {
                    eprintln!(
                        "[jeff] retrieval_reembed_skipped chunk_id={} reason={}",
                        chunk.chunk_id, err
                    );
                }
            }
        }
        normalized_chunks.push(chunk);
    }

    let query_embedding = embeddings
        .embed_text(clean_query)
        .context("failed to generate query embedding")?;

    let mut scored: Vec<(f32, StoredChunkEmbedding)> = normalized_chunks
        .into_iter()
        .map(|chunk| (cosine_similarity(&query_embedding, &chunk.embedding), chunk))
        .collect();

    scored.sort_by(|left, right| right.0.total_cmp(&left.0));

    Ok(scored
        .into_iter()
        .take(top_k)
        .map(|(score, chunk)| RetrievedChunkDto {
            chunk_id: chunk.chunk_id,
            task_id: chunk.task_id,
            artifact_id: chunk.artifact_id,
            artifact_file_name: chunk.artifact_file_name,
            artifact_stored_path: chunk.artifact_stored_path,
            chunk_text: chunk.chunk_text,
            position_index: chunk.position_index,
            similarity_score: score,
        })
        .collect())
}

pub fn build_task_context_pack(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    task_id: i64,
    query: &str,
) -> Result<TaskContextPackDto> {
    let summary = store.get_task_summary(task_id)?;
    let retrieved_chunks = retrieve_relevant_chunks(store, embeddings, task_id, query)?;

    let active_artifact = retrieved_chunks.first().map(|chunk| ContextArtifactDto {
        artifact_id: chunk.artifact_id,
        file_name: chunk.artifact_file_name.clone(),
        stored_path: chunk.artifact_stored_path.clone(),
    });

    Ok(TaskContextPackDto {
        task_summary: summary.summary_text,
        active_task_id: task_id,
        recent_transcript: Vec::new(),
        active_artifact,
        retrieved_chunks,
    })
}

pub fn default_embeddings_provider(local_runtime: Arc<LocalRuntime>) -> LocalEmbeddingProvider {
    LocalEmbeddingProvider::new(local_runtime)
}

// phase 13: idempotent auto-ingest for the filesystem watcher.
// if the file has been ingested before and mtime is unchanged, this is a no-op.
// if mtime changed, existing chunks are replaced without creating a new artifact row.
// on first ingest, a new artifact is created via the standard pipeline.
pub fn auto_ingest_file_for_task(
    store: &crate::store::TaskStore,
    embeddings: &dyn EmbeddingProvider,
    task_id: i64,
    file_path: &Path,
) -> Result<()> {
    let canonical = fs::canonicalize(file_path)
        .with_context(|| format!("failed to canonicalize {}", file_path.display()))?;

    let file_version = fs::metadata(&canonical)
        .ok()
        .map(|metadata| {
            let modified_nanos = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or_default();
            let content_hash = fs::read(&canonical)
                .ok()
                .map(|bytes| {
                    let mut hasher = DefaultHasher::new();
                    bytes.hash(&mut hasher);
                    hasher.finish()
                })
                .unwrap_or_default();
            format!("{modified_nanos}:{}:{content_hash}", metadata.len())
        })
        .unwrap_or_default();

    let canonical_str = canonical.to_string_lossy().to_string();

    // check registry to detect whether file is already up to date.
    let existing = store.get_file_registry_entry(task_id, &canonical_str)?;
    if let Some(ref entry) = existing {
        if entry.last_modified_at == file_version && !file_version.is_empty() {
            return Ok(());
        }
    }

    // parse + chunk + embed.
    let parsed_text = crate::artifact_parser::parse_text_from_artifact(&canonical)?;
    let raw_chunks = crate::chunking::chunk_text(
        &parsed_text,
        crate::chunking::DEFAULT_CHUNK_SIZE_CHARS,
        crate::chunking::DEFAULT_CHUNK_OVERLAP_CHARS,
    );
    if raw_chunks.is_empty() {
        return Err(anyhow::anyhow!("auto-ingest: file produced no chunks"));
    }

    let mut chunk_rows = Vec::with_capacity(raw_chunks.len());
    for (index, chunk) in raw_chunks.iter().enumerate() {
        let embedding = embeddings
            .embed_text(chunk)
            .with_context(|| format!("failed to embed chunk {index}"))?;
        if embedding.is_empty() {
            return Err(anyhow::anyhow!(
                "embedding provider returned empty vector for chunk {index}"
            ));
        }
        chunk_rows.push(crate::store::ChunkEmbeddingInput {
            chunk_text: chunk.to_string(),
            position_index: index as i64,
            embedding,
            embedding_model: embeddings.model_id().to_string(),
        });
    }

    let preview_text = raw_chunks
        .first()
        .map(|c| c.chars().take(150).collect::<String>())
        .unwrap_or_default();

    let file_name = canonical
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();

    if let Some(entry) = existing {
        // re-ingest: replace chunks for the existing artifact.
        if let Some(artifact_id) = entry.artifact_id {
            // overwrite the stored copy so artifact content stays in sync.
            let artifact_list = store.list_artifacts(task_id)?;
            if let Some(artifact) = artifact_list.iter().find(|a| a.id == artifact_id) {
                let _ = fs::copy(&canonical, &artifact.stored_path);
            }

            store.replace_artifact_chunks(task_id, artifact_id, &chunk_rows)?;
            store.upsert_file_registry_entry(
                task_id,
                &canonical_str,
                Some(artifact_id),
                &file_version,
            )?;
            store.append_recently_learned(task_id, "file", &file_name, &preview_text)?;
            return Ok(());
        }
    }

    // first ingest: create artifact via standard pipeline and register it.
    let artifact = import_artifact_for_task(store, embeddings, task_id, &canonical_str)?;
    store.upsert_file_registry_entry(task_id, &canonical_str, Some(artifact.id), &file_version)?;
    store.append_recently_learned(task_id, "file", &file_name, &preview_text)?;
    Ok(())
}

fn sanitize_file_name(file_name: &str) -> String {
    let mut output = String::new();

    for character in file_name.chars() {
        if character.is_ascii_alphanumeric()
            || character == '.'
            || character == '-'
            || character == '_'
        {
            output.push(character);
        } else {
            output.push('-');
        }
    }

    let trimmed = output.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "artifact".to_string()
    } else {
        trimmed
    }
}

fn next_available_artifact_path(directory: &Path, file_name: &str) -> PathBuf {
    let candidate = directory.join(file_name);
    if !candidate.exists() {
        return candidate;
    }

    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("artifact");
    let extension = Path::new(file_name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();

    for suffix in 2..1000 {
        let candidate = directory.join(format!("{stem}-{suffix}{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }

    directory.join(format!("{stem}-fallback{extension}"))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::Arc,
    };

    use anyhow::Result;

    use crate::{
        embedding::EmbeddingProvider,
        local_runtime::{LocalRuntime, LOCAL_EMBEDDING_MODEL_ID},
        providers::local::LocalEmbeddingProvider,
        store::TaskStore,
    };

    use super::{
        auto_ingest_file_for_task, build_task_context_pack, import_artifact_for_task,
        retrieve_relevant_chunks_with_top_k,
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
                score(&["storymap"]),
                score(&["requirement", "requirements", "required"]),
                score(&["structure", "section", "sections"]),
                score(&["evidence", "primary source", "primary sources"]),
                score(&["timeline", "map"]),
                (lower.len() as f32) / 1000.0,
            ])
        }
    }

    struct PanicEmbeddingProvider;

    impl EmbeddingProvider for PanicEmbeddingProvider {
        fn embed_text(&self, _input: &str) -> Result<Vec<f32>> {
            panic!("embedding provider should not be called when no chunks exist")
        }
    }

    // apex b1: errors only for chunks whose text contains POISON, embeds
    // everything else via the keyword provider. reports a new model id so
    // stored chunks look stale and trigger re-embedding.
    struct SelectiveFailEmbeddingProvider;

    impl EmbeddingProvider for SelectiveFailEmbeddingProvider {
        fn embed_text(&self, input: &str) -> Result<Vec<f32>> {
            if input.contains("POISON") {
                return Err(anyhow::anyhow!("simulated transient embedding failure"));
            }
            KeywordEmbeddingProvider.embed_text(input)
        }

        fn model_id(&self) -> &'static str {
            "keyword-embed-v2"
        }
    }

    fn write_file(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("failed to create parent directory");
        }

        fs::write(path, body).expect("failed to write file");
    }

    #[test]
    fn import_file_stores_artifact_in_workspace_and_metadata() {
        let temp = tempfile::tempdir().expect("failed to create temp directory");
        let base_path = temp.path().join("app_local_data");
        let store = TaskStore::initialize(&base_path).expect("failed to initialize store");

        let task = store
            .create_task("History StoryMap")
            .expect("failed to create task");

        let source_path = temp.path().join("fixtures").join("notes.md");
        write_file(
            &source_path,
            "# StoryMap Notes\n\nThe project requires clear structure and evidence.",
        );

        let artifact = import_artifact_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task.id,
            &source_path.to_string_lossy(),
        )
        .expect("failed to import artifact");

        assert_eq!(artifact.task_id, task.id);
        assert!(artifact.chunk_count > 0);
        assert!(PathBuf::from(&artifact.stored_path).exists());

        let artifacts = store
            .list_artifacts(task.id)
            .expect("failed to list artifacts for task");
        assert_eq!(artifacts.len(), 1);
    }

    #[test]
    fn import_creates_chunks_and_stores_embeddings() {
        let temp = tempfile::tempdir().expect("failed to create temp directory");
        let base_path = temp.path().join("app_local_data");
        let store = TaskStore::initialize(&base_path).expect("failed to initialize store");

        let task = store
            .create_task("History StoryMap")
            .expect("failed to create task");

        let source_path = temp.path().join("fixtures").join("rubric.txt");
        write_file(
            &source_path,
            "StoryMap requirements include a clear structure, sections, and evidence expectations.",
        );

        import_artifact_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task.id,
            &source_path.to_string_lossy(),
        )
        .expect("failed to import artifact");

        let chunks = store
            .fetch_chunk_embeddings_for_task(task.id)
            .expect("failed to load chunk embeddings");

        assert!(!chunks.is_empty());
        assert!(chunks.iter().all(|chunk| !chunk.embedding.is_empty()));
    }

    #[test]
    fn retrieval_ranks_relevant_chunks_higher() {
        let temp = tempfile::tempdir().expect("failed to create temp directory");
        let base_path = temp.path().join("app_local_data");
        let store = TaskStore::initialize(&base_path).expect("failed to initialize store");

        let task = store
            .create_task("History StoryMap")
            .expect("failed to create task");

        let notes_path = temp.path().join("fixtures").join("notes.md");
        let unrelated_path = temp.path().join("fixtures").join("other.txt");

        write_file(
            &notes_path,
            "StoryMap requirements:\n1. Include clear structure\n2. Add sections\n3. Provide primary source evidence.",
        );
        write_file(&unrelated_path, "Shopping list: apples, milk, bread.");

        import_artifact_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task.id,
            &notes_path.to_string_lossy(),
        )
        .expect("failed to import notes");

        import_artifact_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task.id,
            &unrelated_path.to_string_lossy(),
        )
        .expect("failed to import unrelated file");

        let retrieved = retrieve_relevant_chunks_with_top_k(
            &store,
            &KeywordEmbeddingProvider,
            task.id,
            "what are the storymap requirements",
            3,
        )
        .expect("failed to retrieve chunks");

        assert!(!retrieved.is_empty());

        let top_text = retrieved[0].chunk_text.to_lowercase();
        assert!(top_text.contains("structure") || top_text.contains("sections"));
        assert!(top_text.contains("evidence") || top_text.contains("primary source"));
    }

    #[test]
    fn retrieval_skips_embedding_call_when_task_has_no_chunks() {
        let temp = tempfile::tempdir().expect("failed to create temp directory");
        let base_path = temp.path().join("app_local_data");
        let store = TaskStore::initialize(&base_path).expect("failed to initialize store");
        let task = store
            .create_task("Empty Task")
            .expect("failed to create task");

        let retrieved = retrieve_relevant_chunks_with_top_k(
            &store,
            &PanicEmbeddingProvider,
            task.id,
            "what should I do next",
            3,
        )
        .expect("empty task retrieval should succeed");

        assert!(retrieved.is_empty());
    }

    #[test]
    fn a3_retrieval_reembeds_stale_embedding_model_on_touch() {
        let temp = tempfile::tempdir().expect("failed to create temp directory");
        let base_path = temp.path().join("app_local_data");
        let store = TaskStore::initialize(&base_path).expect("failed to initialize store");
        let provider = LocalEmbeddingProvider::new(Arc::new(LocalRuntime::new(&base_path)));
        let task = store
            .create_task("Local Embedding")
            .expect("failed to create task");
        let source_path = temp.path().join("fixtures").join("local.md");
        write_file(
            &source_path,
            "Alpha bridge notes with concrete local retrieval terms.",
        );

        import_artifact_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task.id,
            &source_path.to_string_lossy(),
        )
        .expect("failed to import with legacy provider");
        let before = store
            .fetch_chunk_embeddings_for_task(task.id)
            .expect("failed to fetch chunks before retrieval");
        assert!(before
            .iter()
            .all(|chunk| chunk.embedding_model == "unknown"));

        let retrieved = retrieve_relevant_chunks_with_top_k(
            &store,
            &provider,
            task.id,
            "alpha bridge retrieval",
            1,
        )
        .expect("local retrieval failed");
        assert_eq!(retrieved.len(), 1);

        let after = store
            .fetch_chunk_embeddings_for_task(task.id)
            .expect("failed to fetch chunks after retrieval");
        assert!(after
            .iter()
            .all(|chunk| chunk.embedding_model == LOCAL_EMBEDDING_MODEL_ID));
    }

    #[test]
    fn b1_reembed_failure_is_nonfatal_and_keeps_stale_chunk() {
        let temp = tempfile::tempdir().expect("failed to create temp directory");
        let base_path = temp.path().join("app_local_data");
        let store = TaskStore::initialize(&base_path).expect("failed to initialize store");
        let task = store.create_task("Reembed").expect("failed to create task");

        // seed two artifacts under the legacy ("unknown") embedding model.
        let poison_path = temp.path().join("fixtures").join("poison.md");
        let good_path = temp.path().join("fixtures").join("good.md");
        write_file(&poison_path, "POISON structure section requirements notes.");
        write_file(&good_path, "Good structure section requirements overview.");
        import_artifact_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task.id,
            &poison_path.to_string_lossy(),
        )
        .expect("failed to import poison");
        import_artifact_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task.id,
            &good_path.to_string_lossy(),
        )
        .expect("failed to import good");

        // the new provider errors while re-embedding the POISON chunk; the query
        // itself embeds fine. retrieval must still succeed and keep the stale
        // chunk rather than failing the whole request.
        let retrieved = retrieve_relevant_chunks_with_top_k(
            &store,
            &SelectiveFailEmbeddingProvider,
            task.id,
            "structure section requirements",
            5,
        )
        .expect("retrieval must not fail when one chunk cannot be re-embedded");
        assert!(!retrieved.is_empty());

        // the good chunk migrated to the new model; the poison chunk stayed stale.
        let chunks = store
            .fetch_chunk_embeddings_for_task(task.id)
            .expect("failed to fetch chunks");
        assert!(chunks
            .iter()
            .any(|c| c.embedding_model == "keyword-embed-v2"));
        assert!(chunks.iter().any(|c| c.embedding_model == "unknown"));
    }

    #[test]
    fn a3_local_embedding_smoke_returns_same_seed_top1() {
        let temp = tempfile::tempdir().expect("failed to create temp directory");
        let base_path = temp.path().join("app_local_data");
        let store = TaskStore::initialize(&base_path).expect("failed to initialize store");
        let provider = LocalEmbeddingProvider::new(Arc::new(LocalRuntime::new(&base_path)));
        let task = store
            .create_task("Seeded Retrieval")
            .expect("failed to create task");

        let alpha_path = temp.path().join("fixtures").join("alpha.md");
        let beta_path = temp.path().join("fixtures").join("beta.md");
        write_file(
            &alpha_path,
            "Alpha bridge planning notes. The bridge milestone needs pylons, deck sequencing, and permits.",
        );
        write_file(
            &beta_path,
            "Kitchen garden notes. The garden milestone needs basil, mint, soil, and watering.",
        );

        import_artifact_for_task(&store, &provider, task.id, &alpha_path.to_string_lossy())
            .expect("failed to import alpha");
        import_artifact_for_task(&store, &provider, task.id, &beta_path.to_string_lossy())
            .expect("failed to import beta");

        let retrieved = retrieve_relevant_chunks_with_top_k(
            &store,
            &provider,
            task.id,
            "bridge pylons permits sequencing",
            2,
        )
        .expect("local retrieval failed");
        assert!(!retrieved.is_empty());
        assert_eq!(retrieved[0].artifact_file_name, "alpha.md");
    }

    #[test]
    fn context_pack_contains_summary_and_retrieved_chunks() {
        let temp = tempfile::tempdir().expect("failed to create temp directory");
        let base_path = temp.path().join("app_local_data");
        let store = TaskStore::initialize(&base_path).expect("failed to initialize store");

        let task = store
            .create_task("History StoryMap")
            .expect("failed to create task");

        let notes_path = temp.path().join("fixtures").join("notes.md");
        write_file(
            &notes_path,
            "The StoryMap rubric requires structure, sections, and supporting evidence.",
        );

        import_artifact_for_task(
            &store,
            &KeywordEmbeddingProvider,
            task.id,
            &notes_path.to_string_lossy(),
        )
        .expect("failed to import notes");

        let context_pack = build_task_context_pack(
            &store,
            &KeywordEmbeddingProvider,
            task.id,
            "storymap requirements",
        )
        .expect("failed to build context pack");

        assert_eq!(context_pack.active_task_id, task.id);
        assert!(!context_pack.task_summary.is_empty());
        assert!(!context_pack.retrieved_chunks.is_empty());
        assert!(context_pack.recent_transcript.is_empty());
    }

    #[test]
    fn auto_ingest_is_idempotent_and_reindexes_existing_artifact() {
        let temp = tempfile::tempdir().expect("failed to create temp directory");
        let base_path = temp.path().join("app_local_data");
        let store = TaskStore::initialize(&base_path).expect("failed to initialize store");

        let task = store
            .create_task("Watcher Task")
            .expect("failed to create task");
        let source_path = temp.path().join("fixtures").join("watch.md");
        write_file(
            &source_path,
            "# Draft\n\nThe first draft needs tighter evidence integration.",
        );

        auto_ingest_file_for_task(&store, &KeywordEmbeddingProvider, task.id, &source_path)
            .expect("first auto-ingest failed");

        let artifacts_after_first = store
            .list_artifacts(task.id)
            .expect("failed to list artifacts");
        assert_eq!(
            artifacts_after_first.len(),
            1,
            "first ingest should create one artifact"
        );
        let chunks_after_first = store
            .fetch_chunk_embeddings_for_task(task.id)
            .expect("failed to load chunks after first ingest");
        assert!(
            !chunks_after_first.is_empty(),
            "first ingest should store chunks"
        );

        auto_ingest_file_for_task(&store, &KeywordEmbeddingProvider, task.id, &source_path)
            .expect("second auto-ingest failed");

        let artifacts_after_second = store
            .list_artifacts(task.id)
            .expect("failed to list artifacts");
        assert_eq!(
            artifacts_after_second.len(),
            1,
            "idempotent ingest should not create a duplicate artifact"
        );
        let chunks_after_second = store
            .fetch_chunk_embeddings_for_task(task.id)
            .expect("failed to load chunks after second ingest");
        assert_eq!(
            chunks_after_second.len(),
            chunks_after_first.len(),
            "idempotent ingest should keep chunk count stable for unchanged content"
        );

        write_file(
            &source_path,
            "# Draft\n\nUpdated draft now anchors each claim in a primary source and course reading.",
        );

        auto_ingest_file_for_task(&store, &KeywordEmbeddingProvider, task.id, &source_path)
            .expect("reindex after file update failed");

        let artifacts_after_update = store
            .list_artifacts(task.id)
            .expect("failed to list artifacts");
        assert_eq!(
            artifacts_after_update.len(),
            1,
            "updated ingest should reindex existing artifact, not create a new row"
        );

        let updated_chunks = store
            .fetch_chunk_embeddings_for_task(task.id)
            .expect("failed to load chunks after update");
        assert!(
            updated_chunks
                .iter()
                .any(|chunk| chunk.chunk_text.to_lowercase().contains("updated draft")),
            "updated content should be visible in reindexed chunks"
        );
    }
}
