use std::{
    collections::hash_map::DefaultHasher,
    collections::HashMap,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use notify::{event::ModifyKind, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::chunking::{chunk_text, DEFAULT_CHUNK_OVERLAP_CHARS, DEFAULT_CHUNK_SIZE_CHARS};
use crate::store::ChunkEmbeddingInput;

use crate::{
    embedding::EmbeddingProvider, models::WatcherStatusDto, retrieval::auto_ingest_file_for_task,
    store::TaskStore,
};

// debounce window: ingest fires 500 ms after last event for a path.
const DEBOUNCE_MS: u64 = 500;

// poll interval for the debounce task.
const POLL_INTERVAL_MS: u64 = 200;

// clipboard poll interval.
const CLIPBOARD_POLL_MS: u64 = 2_000;

// blocked directory names anywhere in the watched path.
const IGNORED_DIRS: &[&str] = &["artifacts", "node_modules", ".git"];

// max file size for auto-ingest (2 MB).
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

// max clipboard snippet length to ingest.
const MAX_CLIPBOARD_CHARS: usize = 20_000;

// minimum clipboard snippet length — single characters are noise.
const MIN_CLIPBOARD_CHARS: usize = 10;

pub struct WatcherState {
    active: HashMap<i64, ActiveWatcher>,
    clipboard_polls: HashMap<i64, tauri::async_runtime::JoinHandle<()>>,
}

struct ActiveWatcher {
    // dropping the watcher stops filesystem event delivery.
    _watcher: RecommendedWatcher,
    // aborting the handle stops ingest processing.
    debounce_task: tauri::async_runtime::JoinHandle<()>,
    watched_path: PathBuf,
}

impl WatcherState {
    pub fn new() -> Self {
        Self {
            active: HashMap::new(),
            clipboard_polls: HashMap::new(),
        }
    }

    pub fn get_status(&self, task_id: i64) -> WatcherStatusDto {
        match self.active.get(&task_id) {
            Some(w) => WatcherStatusDto {
                task_id,
                is_watching: true,
                watched_path: Some(w.watched_path.to_string_lossy().to_string()),
            },
            None => WatcherStatusDto {
                task_id,
                is_watching: false,
                watched_path: None,
            },
        }
    }
}

// recursively walk a directory and ingest all non-ignored files for a task.
// called once at watcher startup so pre-existing files become available for
// retrieval without the user needing to modify them to trigger a watch event.
fn initial_scan_recursive(
    dir: &Path,
    watch_root: &Path,
    store: &TaskStore,
    embeddings: &Arc<dyn EmbeddingProvider>,
    task_id: i64,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let child = entry.path();
        if child.is_dir() {
            let rel = child.strip_prefix(watch_root).unwrap_or(&child);
            let ignored = rel.components().any(|c| {
                c.as_os_str()
                    .to_str()
                    .map(|name| name.starts_with('.') || IGNORED_DIRS.contains(&name))
                    .unwrap_or(false)
            });
            if !ignored {
                initial_scan_recursive(&child, watch_root, store, embeddings, task_id);
            }
        } else if !should_ignore_file(&child, watch_root) {
            if let Err(err) =
                auto_ingest_file_for_task(store, embeddings.as_ref(), task_id, &child)
            {
                eprintln!(
                    "[jeff watcher] initial scan ingest error {}: {err}",
                    child.display()
                );
            }
        }
    }
}

pub fn start_watcher(
    watcher_state: Arc<Mutex<WatcherState>>,
    task_id: i64,
    folder_path: PathBuf,
    store: TaskStore,
    embeddings: Arc<dyn EmbeddingProvider>,
) -> Result<WatcherStatusDto> {
    // stop existing watcher for this task if any.
    {
        let mut state = watcher_state.lock().expect("watcher state lock poisoned");
        if let Some(old) = state.active.remove(&task_id) {
            old.debounce_task.abort();
        }
    }

    let canonical = folder_path.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize watch path {}",
            folder_path.display()
        )
    })?;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<notify::Result<Event>>();

    let mut watcher = RecommendedWatcher::new(
        move |result| {
            let _ = tx.send(result);
        },
        notify::Config::default(),
    )
    .context("failed to create filesystem watcher")?;

    watcher
        .watch(&canonical, RecursiveMode::Recursive)
        .with_context(|| format!("failed to watch path {}", canonical.display()))?;

    let watch_root = canonical.clone();
    let debounce_task = tauri::async_runtime::spawn(async move {
        // initial scan: ingest files that already exist in the watched folder.
        // the filesystem watcher only fires for changes AFTER it starts, so
        // pre-existing files would never be ingested without this pass.
        // the scan is recursive so files in subdirectories are also picked up.
        {
            let scan_root = watch_root.clone();
            let scan_store = store.clone();
            let scan_embeddings = embeddings.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || {
                initial_scan_recursive(&scan_root, &scan_root, &scan_store, &scan_embeddings, task_id);
            })
            .await;
        }

        let mut pending: HashMap<PathBuf, Instant> = HashMap::new();
        let mut interval = tokio::time::interval(Duration::from_millis(POLL_INTERVAL_MS));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let now = Instant::now();
                    let ready: Vec<PathBuf> = pending
                        .iter()
                        .filter(|(_, t)| now.duration_since(**t) >= Duration::from_millis(DEBOUNCE_MS))
                        .map(|(p, _)| p.clone())
                        .collect();

                    for path in ready {
                        pending.remove(&path);
                        if should_ignore_file(&path, &watch_root) {
                            continue;
                        }
                        if let Err(err) = auto_ingest_file_for_task(
                            &store,
                            embeddings.as_ref(),
                            task_id,
                            &path,
                        ) {
                            eprintln!("[jeff watcher] ingest error {}: {err}", path.display());
                        }
                    }
                }

                event = rx.recv() => {
                    match event {
                        Some(Ok(ev)) => {
                            if matches!(ev.kind, EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)) {
                                for path in ev.paths {
                                    if path.is_file() {
                                        pending.insert(path, Instant::now());
                                        continue;
                                    }

                                    let treat_as_removed =
                                        matches!(ev.kind, EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_)))
                                            || !path.exists();
                                    if !treat_as_removed {
                                        continue;
                                    }

                                    if let Err(err) = handle_removed_path(&store, task_id, &path) {
                                        eprintln!("[jeff watcher] remove handling error {}: {err}", path.display());
                                    }
                                }
                            }
                        }
                        None => break,
                        Some(Err(err)) => {
                            eprintln!("[jeff watcher] notify error: {err}");
                        }
                    }
                }
            }
        }
    });

    let status = WatcherStatusDto {
        task_id,
        is_watching: true,
        watched_path: Some(canonical.to_string_lossy().to_string()),
    };

    {
        let mut state = watcher_state.lock().expect("watcher state lock poisoned");
        state.active.insert(
            task_id,
            ActiveWatcher {
                _watcher: watcher,
                debounce_task,
                watched_path: canonical,
            },
        );
    }

    Ok(status)
}

// start the clipboard poll task for a task. safe to call even if no filesystem
// watcher is active (clipboard capture is independent of folder watching).
pub fn start_clipboard_poll(
    watcher_state: Arc<Mutex<WatcherState>>,
    task_id: i64,
    store: TaskStore,
    embeddings: Arc<dyn EmbeddingProvider>,
) {
    // last-seen hash; used to deduplicate repeated clipboard reads.
    let last_hash: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));

    let handle = tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(CLIPBOARD_POLL_MS));

        loop {
            interval.tick().await;

            // check if clipboard capture is still enabled.
            let enabled = store.get_clipboard_capture(task_id).unwrap_or(false);
            if !enabled {
                continue;
            }

            // read clipboard on a blocking thread to avoid blocking the async runtime.
            let content_result = tauri::async_runtime::spawn_blocking(|| {
                let mut cb = arboard::Clipboard::new().ok()?;
                cb.get_text().ok()
            })
            .await;

            let content = match content_result {
                Ok(Some(text)) => text,
                _ => continue,
            };

            // length guards.
            let trimmed = content.trim();
            if trimmed.len() < MIN_CLIPBOARD_CHARS || trimmed.len() > MAX_CLIPBOARD_CHARS {
                continue;
            }

            // dedup by content hash.
            let hash = {
                let mut h = DefaultHasher::new();
                trimmed.hash(&mut h);
                h.finish()
            };

            {
                let mut guard = last_hash.lock().expect("clipboard hash lock poisoned");
                if *guard == Some(hash) {
                    continue;
                }
                *guard = Some(hash);
            }

            let preview: String = trimmed.chars().take(150).collect();
            let label = format!("clipboard snippet ({} chars)", trimmed.len());

            // ingest as a text artifact.
            let ingest_result = ingest_clipboard_snippet(
                &store,
                embeddings.as_ref(),
                task_id,
                trimmed,
                &label,
                &preview,
            );
            if let Err(err) = ingest_result {
                eprintln!("[jeff clipboard] ingest error: {err}");
            }
        }
    });

    let mut state = watcher_state.lock().expect("watcher state lock poisoned");
    if let Some(old) = state.clipboard_polls.insert(task_id, handle) {
        old.abort();
    }
}

pub fn stop_clipboard_poll(watcher_state: Arc<Mutex<WatcherState>>, task_id: i64) {
    let mut state = watcher_state.lock().expect("watcher state lock poisoned");
    if let Some(h) = state.clipboard_polls.remove(&task_id) {
        h.abort();
    }
}

pub fn stop_watcher(watcher_state: Arc<Mutex<WatcherState>>, task_id: i64) -> WatcherStatusDto {
    let mut state = watcher_state.lock().expect("watcher state lock poisoned");
    if let Some(old) = state.active.remove(&task_id) {
        old.debounce_task.abort();
    }
    WatcherStatusDto {
        task_id,
        is_watching: false,
        watched_path: None,
    }
}

pub fn get_watcher_status(
    watcher_state: Arc<Mutex<WatcherState>>,
    task_id: i64,
) -> WatcherStatusDto {
    let state = watcher_state.lock().expect("watcher state lock poisoned");
    state.get_status(task_id)
}

pub fn stop_all_except(watcher_state: Arc<Mutex<WatcherState>>, keep_task_id: Option<i64>) {
    let mut state = watcher_state.lock().expect("watcher state lock poisoned");

    let watcher_ids: Vec<i64> = state.active.keys().copied().collect();
    for id in watcher_ids {
        if keep_task_id == Some(id) {
            continue;
        }
        if let Some(old) = state.active.remove(&id) {
            old.debounce_task.abort();
        }
    }

    let clipboard_ids: Vec<i64> = state.clipboard_polls.keys().copied().collect();
    for id in clipboard_ids {
        if keep_task_id == Some(id) {
            continue;
        }
        if let Some(handle) = state.clipboard_polls.remove(&id) {
            handle.abort();
        }
    }
}

fn handle_removed_path(store: &TaskStore, task_id: i64, path: &Path) -> Result<()> {
    let path_key = path.to_string_lossy().to_string();
    let Some(entry) = store.get_file_registry_entry(task_id, &path_key)? else {
        return Ok(());
    };

    if let Some(artifact_id) = entry.artifact_id {
        // Keep artifact metadata/history, but remove chunks so deleted files
        // no longer influence retrieval.
        store.replace_artifact_chunks(task_id, artifact_id, &[])?;
    }
    store.remove_file_registry_entry(task_id, &path_key)?;

    let label = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("removed file")
        .to_string();
    let preview = "file removed from watched workspace";
    let _ = store.append_recently_learned(task_id, "file", &label, preview);

    Ok(())
}

fn ingest_clipboard_snippet(
    store: &TaskStore,
    embeddings: &dyn EmbeddingProvider,
    task_id: i64,
    text: &str,
    label: &str,
    preview: &str,
) -> Result<()> {
    let chunks = chunk_text(text, DEFAULT_CHUNK_SIZE_CHARS, DEFAULT_CHUNK_OVERLAP_CHARS);
    if chunks.is_empty() {
        return Ok(());
    }

    let mut chunk_rows = Vec::with_capacity(chunks.len());
    for (index, chunk) in chunks.iter().enumerate() {
        let embedding = embeddings
            .embed_text(chunk)
            .with_context(|| format!("failed to embed clipboard chunk {index}"))?;
        if embedding.is_empty() {
            return Err(anyhow::anyhow!(
                "empty embedding for clipboard chunk {index}"
            ));
        }
        chunk_rows.push(ChunkEmbeddingInput {
            chunk_text: chunk.to_string(),
            position_index: index as i64,
            embedding,
        });
    }

    let workspace_path = store.get_task_workspace_path(task_id)?;
    let clipboard_dir = workspace_path.join("clipboard");
    std::fs::create_dir_all(&clipboard_dir)
        .context("failed to create clipboard snippet directory")?;

    // write the snippet as a temp file so import_artifact_for_task can use it.
    let file_name = format!(
        "clip_{}.txt",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    let snippet_path = clipboard_dir.join(&file_name);
    std::fs::write(&snippet_path, text).context("failed to write clipboard snippet file")?;

    let stored_path = snippet_path.to_string_lossy().to_string();
    store.insert_artifact_with_chunks(
        task_id,
        &file_name,
        "txt",
        &stored_path,
        &stored_path,
        &chunk_rows,
    )?;
    store.append_recently_learned(task_id, "clipboard", label, preview)?;

    Ok(())
}

pub fn should_ignore_file(path: &Path, watch_root: &Path) -> bool {
    // must exist as a regular file.
    if !path.is_file() {
        return true;
    }

    // hidden file (name starts with '.').
    if path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with('.'))
        .unwrap_or(false)
    {
        return true;
    }

    // check only the relative path components (inside the watch root).
    // this avoids false-positives from the watch root path itself having
    // hidden components (e.g. tempfile directories like .tmpXXXX in tests,
    // or user-selected dirs under hidden parent paths).
    let rel = path.strip_prefix(watch_root).unwrap_or(path);
    for component in rel.components() {
        if let Some(name) = component.as_os_str().to_str() {
            if name.starts_with('.') {
                return true;
            }
            if IGNORED_DIRS.contains(&name) {
                return true;
            }
        }
    }

    // file too large.
    if let Ok(meta) = path.metadata() {
        if meta.len() > MAX_FILE_BYTES {
            return true;
        }
    }

    // unsupported extension (not parseable by artifact_parser).
    if crate::artifact_parser::supported_artifact_type(path).is_err() {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    use super::{should_ignore_file, DEBOUNCE_MS};

    fn tmp_file(dir: &TempDir, rel: &str) -> PathBuf {
        let path = dir.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(&path, "test content").ok();
        path
    }

    #[test]
    fn ignore_rules_block_binary_extensions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let path = tmp_file(&dir, "image.png");
        assert!(should_ignore_file(&path, &root), "png should be ignored");
        let exe = tmp_file(&dir, "program.exe");
        assert!(should_ignore_file(&exe, &root), "exe should be ignored");
    }

    #[test]
    fn ignore_rules_block_hidden_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let path = tmp_file(&dir, ".hidden_config");
        assert!(
            should_ignore_file(&path, &root),
            ".hidden_config should be ignored"
        );
    }

    #[test]
    fn ignore_rules_block_artifacts_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let path = tmp_file(&dir, "artifacts/notes.md");
        assert!(
            should_ignore_file(&path, &root),
            "artifacts/ subdir should be ignored"
        );
    }

    #[test]
    fn ignore_rules_block_node_modules() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let path = tmp_file(&dir, "node_modules/some-pkg/index.js");
        assert!(
            should_ignore_file(&path, &root),
            "node_modules should be ignored"
        );
    }

    #[test]
    fn ignore_rules_allow_plain_text_and_markdown() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let md = tmp_file(&dir, "notes.md");
        let txt = tmp_file(&dir, "outline.txt");
        assert!(
            !should_ignore_file(&md, &root),
            "notes.md should pass ignore rules"
        );
        assert!(
            !should_ignore_file(&txt, &root),
            "outline.txt should pass ignore rules"
        );
    }

    #[test]
    fn ignore_rules_block_file_in_hidden_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let path = tmp_file(&dir, ".obsidian/config.md");
        assert!(
            should_ignore_file(&path, &root),
            "file in hidden dir should be ignored"
        );
    }

    #[test]
    fn watcher_debounces_rapid_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = tmp_file(&dir, "notes.md");
        let debounce_window = Duration::from_millis(DEBOUNCE_MS);

        let mut pending: HashMap<PathBuf, Instant> = HashMap::new();
        let first_seen = Instant::now();
        pending.insert(path.clone(), first_seen);

        // second event inside the debounce window should replace the timestamp.
        let second_seen = first_seen + Duration::from_millis(50);
        pending.insert(path.clone(), second_seen);
        assert_eq!(
            pending.len(),
            1,
            "duplicate rapid events should dedupe into one pending path"
        );

        let before_ready = second_seen + Duration::from_millis(DEBOUNCE_MS - 1);
        let ready_before: Vec<PathBuf> = pending
            .iter()
            .filter(|(_, seen_at)| before_ready.duration_since(**seen_at) >= debounce_window)
            .map(|(p, _)| p.clone())
            .collect();
        assert!(
            ready_before.is_empty(),
            "path should not be ready before the debounce window elapses"
        );

        let at_window = second_seen + debounce_window;
        let ready_after: Vec<PathBuf> = pending
            .iter()
            .filter(|(_, seen_at)| at_window.duration_since(**seen_at) >= debounce_window)
            .map(|(p, _)| p.clone())
            .collect();
        assert_eq!(
            ready_after,
            vec![path],
            "path should become ready once debounce window has elapsed"
        );
    }
}
