// apex e5: Drive and Docs remote read. Documents that never touch the local
// folder become Jeff's context on demand: "pull in the shared doc" ingests a
// remote Doc's text into retrieval as an artifact, tagged with provenance
// (source + url). Jobs can then cite it like any local source. Per-item removal
// purges the ingested chunks (artifact delete cascades to artifact_chunks).
//
// Live Drive/Docs access (MCP/OAuth export) is env-gated; the ingestion,
// provenance tagging, retrieval grounding, and purge-on-removal are
// deterministic and tested over provided content.

#![cfg_attr(test, allow(dead_code))]

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::store::{ChunkEmbeddingInput, TaskStore};

#[allow(dead_code)]
pub const PROVENANCE_DRIVE: &str = "google_drive";
#[allow(dead_code)]
pub const PROVENANCE_DOCS: &str = "google_docs";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RemoteDocDto {
    pub id: i64,
    pub task_id: i64,
    pub title: String,
    pub url: String,
    pub provenance: String,
    pub artifact_id: Option<i64>,
    pub created_at: String,
}

// ingest a remote document into retrieval, tagged with provenance. Content is
// chunked by paragraph like local ingestion; the returned record links the
// artifact so removal can purge it.
pub fn ingest_remote_doc(
    store: &TaskStore,
    task_id: i64,
    title: &str,
    url: &str,
    provenance: &str,
    content: &str,
) -> Result<RemoteDocDto> {
    let title = title.trim();
    if title.is_empty() {
        return Err(anyhow::anyhow!("remote doc title cannot be empty"));
    }
    let chunks = content
        .split("\n\n")
        .map(str::trim)
        .filter(|chunk| !chunk.is_empty())
        .enumerate()
        .map(|(index, chunk)| ChunkEmbeddingInput {
            chunk_text: chunk.to_string(),
            position_index: index as i64,
            embedding: Vec::new(),
            embedding_model: "remote-ingest".to_string(),
        })
        .collect::<Vec<_>>();

    // stored/original path carry the remote url so provenance survives end to end.
    let artifact = store.insert_artifact_with_chunks(
        task_id,
        title,
        "remote",
        url,
        url,
        &chunks,
    )?;

    let conn = store.connect()?;
    conn.execute(
        "INSERT INTO remote_ingested_docs (task_id, title, url, provenance, artifact_id)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![task_id, title, url.trim(), provenance.trim(), artifact.id],
    )
    .context("failed to record remote ingested doc")?;
    let id = conn.last_insert_rowid();
    drop(conn);
    get_remote_doc(store, id)?.ok_or_else(|| anyhow::anyhow!("remote doc missing after ingest"))
}

pub fn list_remote_docs(store: &TaskStore) -> Result<Vec<RemoteDocDto>> {
    let conn = store.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, task_id, title, url, provenance, artifact_id, created_at
         FROM remote_ingested_docs ORDER BY id DESC",
    )?;
    let rows = stmt
        .query_map([], remote_doc_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// removal purges the ingested chunks: deleting the artifact cascades to
// artifact_chunks (foreign_keys ON), so the content leaves retrieval.
pub fn remove_remote_doc(store: &TaskStore, id: i64) -> Result<()> {
    let doc = get_remote_doc(store, id)?
        .ok_or_else(|| anyhow::anyhow!("remote doc id={id} not found"))?;
    let conn = store.connect()?;
    if let Some(artifact_id) = doc.artifact_id {
        conn.execute("DELETE FROM artifacts WHERE id = ?1", params![artifact_id])
            .context("failed to delete ingested artifact")?;
    }
    conn.execute("DELETE FROM remote_ingested_docs WHERE id = ?1", params![id])
        .context("failed to delete remote doc record")?;
    Ok(())
}

fn get_remote_doc(store: &TaskStore, id: i64) -> Result<Option<RemoteDocDto>> {
    let conn = store.connect()?;
    conn.query_row(
        "SELECT id, task_id, title, url, provenance, artifact_id, created_at
         FROM remote_ingested_docs WHERE id = ?1",
        params![id],
        remote_doc_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn remote_doc_from_row(row: &Row<'_>) -> rusqlite::Result<RemoteDocDto> {
    Ok(RemoteDocDto {
        id: row.get(0)?,
        task_id: row.get(1)?,
        title: row.get(2)?,
        url: row.get(3)?,
        provenance: row.get(4)?,
        artifact_id: row.get(5)?,
        created_at: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_store() -> (TempDir, TaskStore, i64) {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        let task = store.create_task("drive").unwrap();
        (dir, store, task.id)
    }

    fn chunk_count(store: &TaskStore, artifact_id: i64) -> i64 {
        let conn = store.connect().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM artifact_chunks WHERE artifact_id = ?1",
            params![artifact_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn e5_ingest_remote_doc_grounds_with_provenance() {
        let (_dir, store, task_id) = test_store();
        let doc = ingest_remote_doc(
            &store,
            task_id,
            "Shared spec",
            "https://docs.google.com/document/d/abc",
            PROVENANCE_DOCS,
            "The shared spec defines the migration plan.\n\nIt sets the deadline for Friday.",
        )
        .unwrap();
        assert_eq!(doc.provenance, PROVENANCE_DOCS);
        assert_eq!(doc.url, "https://docs.google.com/document/d/abc");
        let artifact_id = doc.artifact_id.unwrap();
        // the content is in retrieval (a job could cite it).
        assert_eq!(chunk_count(&store, artifact_id), 2);
        assert!(list_remote_docs(&store).unwrap().len() == 1);
    }

    #[test]
    fn e5_removal_purges_ingested_chunks() {
        let (_dir, store, task_id) = test_store();
        let doc = ingest_remote_doc(
            &store,
            task_id,
            "Shared doc",
            "https://drive.google.com/file/xyz",
            PROVENANCE_DRIVE,
            "Remote content one.\n\nRemote content two.",
        )
        .unwrap();
        let artifact_id = doc.artifact_id.unwrap();
        assert_eq!(chunk_count(&store, artifact_id), 2);

        remove_remote_doc(&store, doc.id).unwrap();
        // artifact deletion cascaded to its chunks; retrieval is purged.
        assert_eq!(chunk_count(&store, artifact_id), 0);
        assert!(list_remote_docs(&store).unwrap().is_empty());
    }
}
