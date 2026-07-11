use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use serde_json::json;

use crate::{
    message_kind::MessageKind,
    models::{
        ActionReceiptDto, ArtifactContentDto, ArtifactDto, ArtifactVersionDto, ChatMessageDto,
        EventLogEntryDto, FileWriteProposalDto, OpenResourceDto, ProactiveAuditEntryDto,
        RecentlyLearnedItemDto, RevisionProposalDto, SessionModeStateDto, SubTaskDto,
        SubTaskStepDto, SuggestionDto, SynthesisLogEntryDto, TaskDto, TaskSummaryDto,
        WatchedFileRegistryEntry, WatchedFolderDto, WorkspaceInfoDto, WriteAuditEntryDto,
    },
    onboarding::{
        APP_SETTING_ONBOARDING_COMPLETE, APP_SETTING_ONBOARDING_LAST_COMPLETED_AT,
        APP_SETTING_PREFERRED_WORKSPACE_FOLDER, APP_SETTING_WORKSPACE_PROMPT_DISMISSED,
    },
    workspace::slugify_title,
};

const SQLITE_NOW_EXPR: &str = "strftime('%Y-%m-%dT%H:%M:%fZ','now')";

// phase 19: session persistence setting keys
pub const APP_SETTING_LAUNCH_AT_LOGIN: &str = "launch_at_login";
pub const APP_SETTING_OVERLAY_MODE: &str = "overlay_mode";
pub const APP_SETTING_QUIET_MODE: &str = "quiet_mode";
pub const APP_SETTING_SESSION_RESTORED_AT: &str = "session_restored_at";

// phase 21: privacy center app setting keys
pub const APP_SETTING_PRIVACY_WORKSPACE_WATCHER_ENABLED: &str = "privacy_workspace_watcher_enabled";
pub const APP_SETTING_PRIVACY_CLIPBOARD_CAPTURE_ENABLED: &str = "privacy_clipboard_capture_enabled";
pub const APP_SETTING_PRIVACY_ACTIVE_WINDOW_CONTEXT_ENABLED: &str =
    "privacy_active_window_context_enabled";
pub const APP_SETTING_PRIVACY_PROACTIVE_TRIGGERS_ENABLED: &str =
    "privacy_proactive_triggers_enabled";
pub const APP_SETTING_PRIVACY_USER_PROFILE_MEMORY_ENABLED: &str =
    "privacy_user_profile_memory_enabled";
pub const APP_SETTING_PRIVACY_CALENDAR_CONTEXT_ENABLED: &str = "privacy_calendar_context_enabled";
pub const APP_SETTING_PRIVACY_SELECTION_CAPTURE_ENABLED: &str = "privacy_selection_capture_enabled";
pub const APP_SETTING_PRIVACY_TYPING_ACTIVITY_ENABLED: &str = "privacy_typing_activity_enabled";
pub const APP_SETTING_TTS_VOICE: &str = "tts_voice";

// prefix for per-task last reorientation summary (key = prefix + task_id)
const LAST_REORIENTATION_SUMMARY_PREFIX: &str = "last_reorientation_summary:";

#[derive(Debug, Clone)]
pub struct StorePaths {
    pub db_path: PathBuf,
    pub workspace_root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct TaskStore {
    pub paths: StorePaths,
}

#[derive(Debug, Clone)]
pub struct ChunkEmbeddingInput {
    pub chunk_text: String,
    pub position_index: i64,
    pub embedding: Vec<f32>,
    pub embedding_model: String,
}

#[derive(Debug, Clone)]
pub struct StoredChunkEmbedding {
    pub chunk_id: i64,
    pub task_id: i64,
    pub artifact_id: i64,
    pub artifact_file_name: String,
    pub artifact_stored_path: String,
    pub chunk_text: String,
    pub position_index: i64,
    pub embedding: Vec<f32>,
    pub embedding_model: String,
}

#[derive(Debug, Clone)]
pub struct LlmUsageLogInput {
    pub tier: String,
    pub model: String,
    pub purpose: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub est_cost_usd: f64,
}

// apex c2: a delivered interjection and how it landed.
#[derive(Debug, Clone)]
pub struct InterruptionLedgerRow {
    pub reason_type: String,
    pub focus_score: f32,
    pub reaction: Option<String>,
    pub delivered_at_unix: i64,
}

#[derive(Debug, Clone)]
pub struct LlmSpendByTier {
    pub tier: String,
    pub est_cost_usd: f64,
}

#[derive(Debug, Clone)]
pub struct LlmSpendHistoryRow {
    pub date: String,
    pub est_cost_usd: f64,
}

#[derive(Debug, Clone)]
pub struct NewRevisionProposalInput {
    pub task_id: i64,
    pub artifact_id: i64,
    pub target_start_offset: i64,
    pub target_end_offset: i64,
    pub target_description: String,
    pub original_text: String,
    pub proposed_text: String,
    pub instruction_text: String,
    pub instruction_source: String,
    pub rationale: Option<String>,
    pub grounding_notes: Option<String>,
    pub retrieval_confidence: f32,
    pub parent_revision_id: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct NewArtifactVersionInput {
    pub task_id: i64,
    pub artifact_id: i64,
    pub revision_id: Option<i64>,
    pub version_reason: String,
    pub content_snapshot: String,
    pub stored_path: String,
}

#[derive(Debug, Clone)]
pub struct ArtifactVersionSnapshot {
    pub dto: ArtifactVersionDto,
    pub content_snapshot: String,
    pub stored_path: String,
}

#[derive(Debug, Clone)]
pub struct NewSubTaskInput {
    pub task_id: i64,
    pub title: String,
    pub description: String,
    pub execution_type: String,
    pub instruction_source: String,
    pub parent_context_snapshot: String,
}

#[derive(Debug, Clone)]
pub struct SessionModeUpdateInput {
    pub task_id: i64,
    pub current_mode: String,
    pub mode_reason: String,
    pub waiting_on_user_decision: bool,
    pub last_engine_decision: String,
    pub active_artifact_id: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct NewSuggestionInput {
    pub task_id: i64,
    pub title: String,
    pub description: String,
    pub suggestion_type: String,
    pub source_reason: String,
    pub suggestion_key: String,
    pub linked_context: Option<String>,
    pub linked_subtask_type: Option<String>,
    pub linked_revision_intent: Option<String>,
}

impl TaskStore {
    pub fn initialize(base_dir: &Path) -> Result<Self> {
        fs::create_dir_all(base_dir)
            .with_context(|| format!("failed to create store base dir {}", base_dir.display()))?;

        let db_path = base_dir.join("jeff_store.sqlite3");
        let workspace_root = base_dir.join("tasks");
        fs::create_dir_all(&workspace_root).with_context(|| {
            format!(
                "failed to create workspace root {}",
                workspace_root.display()
            )
        })?;

        let store = Self {
            paths: StorePaths {
                db_path,
                workspace_root,
            },
        };

        store.initialize_schema()?;
        Ok(store)
    }

    pub(crate) fn connect(&self) -> Result<Connection> {
        let conn = Connection::open(&self.paths.db_path).with_context(|| {
            format!("failed to open sqlite db {}", self.paths.db_path.display())
        })?;
        conn.execute_batch(
            "
            PRAGMA foreign_keys = ON;
            PRAGMA journal_mode = WAL;
            PRAGMA busy_timeout = 10000;
            ",
        )
        .context("failed to initialize sqlite pragmas")?;
        Ok(conn)
    }

    pub fn action_undo_root(&self) -> PathBuf {
        self.paths
            .db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.paths.workspace_root.clone())
            .join("undo")
    }

    // apex d9: root for self-built tool staging/installed directories.
    pub fn custom_tools_root(&self) -> PathBuf {
        self.paths
            .db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.paths.workspace_root.clone())
            .join("tools")
    }

    // apex d9: the workspace root confines self-built text-script execution.
    pub fn workspace_root_path(&self) -> PathBuf {
        self.paths.workspace_root.clone()
    }

    fn initialize_schema(&self) -> Result<()> {
        let conn = self.connect()?;
        conn.execute_batch(&format!(
            r#"
            CREATE TABLE IF NOT EXISTS tasks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                slug TEXT NOT NULL UNIQUE,
                workspace_path TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT ({now}),
                updated_at TEXT NOT NULL DEFAULT ({now}),
                is_active INTEGER NOT NULL DEFAULT 0
            );

            CREATE UNIQUE INDEX IF NOT EXISTS idx_tasks_single_active
                ON tasks(is_active)
                WHERE is_active = 1;

            CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                created_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE TABLE IF NOT EXISTS task_summaries (
                task_id INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
                summary_text TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE TABLE IF NOT EXISTS open_resources (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                resource_type TEXT NOT NULL,
                resource_path_or_url TEXT NOT NULL,
                label TEXT NOT NULL,
                position_index INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS artifacts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                file_name TEXT NOT NULL,
                file_extension TEXT NOT NULL,
                original_path TEXT NOT NULL,
                stored_path TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT ({now}),
                updated_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE TABLE IF NOT EXISTS artifact_chunks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                artifact_id INTEGER NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
                chunk_text TEXT NOT NULL,
                position_index INTEGER NOT NULL,
                embedding_json TEXT NOT NULL,
                embedding_model TEXT NOT NULL DEFAULT '{embedding_model}'
            );

            CREATE INDEX IF NOT EXISTS idx_artifact_chunks_task ON artifact_chunks(task_id);
            CREATE INDEX IF NOT EXISTS idx_artifact_chunks_artifact ON artifact_chunks(artifact_id, position_index);

            CREATE TABLE IF NOT EXISTS chat_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                session_id INTEGER,
                role TEXT NOT NULL,
                message_source TEXT NOT NULL,
                message_kind TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_chat_messages_task ON chat_messages(task_id, id);

            CREATE TABLE IF NOT EXISTS artifact_revisions (
                revision_id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                artifact_id INTEGER NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
                target_start_offset INTEGER NOT NULL,
                target_end_offset INTEGER NOT NULL,
                target_description TEXT NOT NULL,
                original_text TEXT NOT NULL,
                proposed_text TEXT NOT NULL,
                instruction_text TEXT NOT NULL,
                instruction_source TEXT NOT NULL,
                rationale TEXT,
                grounding_notes TEXT,
                retrieval_confidence REAL NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                created_at TEXT NOT NULL DEFAULT ({now}),
                updated_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_revisions_task ON artifact_revisions(task_id, status, revision_id);
            CREATE INDEX IF NOT EXISTS idx_revisions_artifact ON artifact_revisions(artifact_id, status, revision_id);

            CREATE TABLE IF NOT EXISTS artifact_versions (
                version_id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                artifact_id INTEGER NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
                revision_id INTEGER,
                version_reason TEXT NOT NULL,
                content_snapshot TEXT NOT NULL,
                stored_path TEXT NOT NULL,
                content_preview TEXT NOT NULL,
                content_length INTEGER NOT NULL,
                created_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_versions_artifact ON artifact_versions(artifact_id, version_id DESC);

            CREATE TABLE IF NOT EXISTS subtasks (
                subtask_id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                title TEXT NOT NULL,
                description TEXT NOT NULL,
                execution_type TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                result_review_status TEXT NOT NULL DEFAULT 'unreviewed',
                created_at TEXT NOT NULL DEFAULT ({now}),
                updated_at TEXT NOT NULL DEFAULT ({now}),
                result_summary TEXT,
                result_payload TEXT,
                instruction_source TEXT NOT NULL,
                parent_context_snapshot TEXT NOT NULL,
                error_message TEXT,
                max_steps INTEGER NOT NULL DEFAULT 5,
                current_step INTEGER NOT NULL DEFAULT 0
            );

            CREATE INDEX IF NOT EXISTS idx_subtasks_task ON subtasks(task_id, subtask_id DESC);
            CREATE INDEX IF NOT EXISTS idx_subtasks_status ON subtasks(task_id, status);

            CREATE TABLE IF NOT EXISTS session_mode_state (
                task_id INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
                current_mode TEXT NOT NULL,
                mode_reason TEXT NOT NULL,
                waiting_on_user_decision INTEGER NOT NULL DEFAULT 0,
                last_engine_decision TEXT NOT NULL,
                active_artifact_id INTEGER,
                updated_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE TABLE IF NOT EXISTS suggestions (
                suggestion_id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                title TEXT NOT NULL,
                description TEXT NOT NULL,
                suggestion_type TEXT NOT NULL,
                source_reason TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                suggestion_key TEXT NOT NULL,
                linked_context TEXT,
                linked_subtask_type TEXT,
                linked_revision_intent TEXT,
                created_at TEXT NOT NULL DEFAULT ({now}),
                updated_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_suggestions_task_status ON suggestions(task_id, status, suggestion_id DESC);
            CREATE INDEX IF NOT EXISTS idx_suggestions_task_key ON suggestions(task_id, suggestion_key);

            CREATE TABLE IF NOT EXISTS app_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE TABLE IF NOT EXISTS llm_usage_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tier TEXT NOT NULL,
                model TEXT NOT NULL,
                purpose TEXT NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                cached_tokens INTEGER NOT NULL,
                est_cost_usd REAL NOT NULL,
                created_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_llm_usage_log_created_at
                ON llm_usage_log(created_at);
            CREATE INDEX IF NOT EXISTS idx_llm_usage_log_tier_created_at
                ON llm_usage_log(tier, created_at);

            CREATE TABLE IF NOT EXISTS event_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                event_type TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_event_log_task ON event_log(task_id, id DESC);

            CREATE TABLE IF NOT EXISTS watched_folders (
                task_id INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
                folder_path TEXT NOT NULL,
                is_active INTEGER NOT NULL DEFAULT 1,
                ignore_rules_json TEXT NOT NULL DEFAULT '[]',
                created_at TEXT NOT NULL DEFAULT ({now}),
                updated_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE TABLE IF NOT EXISTS watched_file_registry (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                canonical_path TEXT NOT NULL,
                artifact_id INTEGER,
                last_modified_at TEXT NOT NULL DEFAULT '',
                ingested_at TEXT NOT NULL DEFAULT ({now}),
                UNIQUE(task_id, canonical_path)
            );

            CREATE INDEX IF NOT EXISTS idx_file_registry_task ON watched_file_registry(task_id, id DESC);

            CREATE TABLE IF NOT EXISTS clipboard_capture_settings (
                task_id INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
                enabled INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE TABLE IF NOT EXISTS recently_learned_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                source TEXT NOT NULL,
                display_label TEXT NOT NULL,
                preview_text TEXT NOT NULL,
                ingested_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_recently_learned_task ON recently_learned_log(task_id, id DESC);

            CREATE TABLE IF NOT EXISTS proactive_trigger_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                trigger_type TEXT NOT NULL,
                fired_at TEXT NOT NULL DEFAULT ({now}),
                suppressed INTEGER NOT NULL DEFAULT 0
            );

            CREATE INDEX IF NOT EXISTS idx_proactive_trigger_task_type
                ON proactive_trigger_log(task_id, trigger_type, id DESC);

            CREATE TABLE IF NOT EXISTS synthesis_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER REFERENCES tasks(id) ON DELETE CASCADE,
                reason_type TEXT NOT NULL,
                reason_detail TEXT,
                snapshot_confidence REAL NOT NULL,
                snapshot_attention_state TEXT NOT NULL,
                message TEXT,
                delivered INTEGER NOT NULL DEFAULT 0,
                delivered_at TEXT,
                created_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_synthesis_log_task
                ON synthesis_log(task_id, id DESC);
            CREATE INDEX IF NOT EXISTS idx_synthesis_log_delivered
                ON synthesis_log(task_id, delivered, delivered_at DESC);

            CREATE TABLE IF NOT EXISTS task_focus_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                focused_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_task_focus_task ON task_focus_log(task_id, id DESC);

            CREATE TABLE IF NOT EXISTS subtask_steps (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                subtask_id INTEGER NOT NULL REFERENCES subtasks(subtask_id) ON DELETE CASCADE,
                step_index INTEGER NOT NULL,
                step_type TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                description TEXT NOT NULL DEFAULT '',
                result_summary TEXT,
                result_payload TEXT,
                error_message TEXT,
                started_at TEXT,
                completed_at TEXT,
                UNIQUE(subtask_id, step_index)
            );

            CREATE INDEX IF NOT EXISTS idx_subtask_steps_subtask ON subtask_steps(subtask_id, step_index);

            CREATE TABLE IF NOT EXISTS subtask_file_write_proposals (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                subtask_id INTEGER NOT NULL REFERENCES subtasks(subtask_id) ON DELETE CASCADE,
                step_id INTEGER REFERENCES subtask_steps(id) ON DELETE SET NULL,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                proposed_path TEXT NOT NULL,
                proposed_content TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending_approval',
                proposed_at TEXT NOT NULL DEFAULT ({now}),
                resolved_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_file_write_proposals_task_status
                ON subtask_file_write_proposals(task_id, status, id DESC);
            CREATE INDEX IF NOT EXISTS idx_file_write_proposals_subtask
                ON subtask_file_write_proposals(subtask_id, id DESC);

            CREATE TABLE IF NOT EXISTS subtask_write_audit_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                subtask_id INTEGER NOT NULL,
                proposal_id INTEGER NOT NULL,
                action_receipt_id INTEGER REFERENCES action_receipts(id) ON DELETE SET NULL,
                action TEXT NOT NULL,
                proposed_path TEXT NOT NULL,
                resolved_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_write_audit_task ON subtask_write_audit_log(task_id, id DESC);

            CREATE TABLE IF NOT EXISTS action_receipts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                class TEXT NOT NULL,
                surface TEXT NOT NULL,
                level TEXT NOT NULL DEFAULT 'L1',
                description TEXT NOT NULL,
                payload_excerpt TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL,
                failure_reason TEXT,
                undo_ref TEXT,
                outcome_accounted_at TEXT,
                created_at TEXT NOT NULL DEFAULT ({now}),
                resolved_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_action_receipts_task
                ON action_receipts(task_id, id DESC);
            CREATE INDEX IF NOT EXISTS idx_action_receipts_class
                ON action_receipts(class, status, id DESC);

            CREATE TABLE IF NOT EXISTS trust_levels (
                class TEXT PRIMARY KEY,
                level TEXT NOT NULL DEFAULT 'L1',
                approval_streak INTEGER NOT NULL DEFAULT 0,
                graduation_offered_at TEXT,
                sticky_l1 INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE TABLE IF NOT EXISTS jobs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                goal_contract TEXT NOT NULL,
                plan_json TEXT NOT NULL DEFAULT '[]',
                budget_json TEXT NOT NULL DEFAULT '{{}}',
                status TEXT NOT NULL DEFAULT 'pending',
                speculative INTEGER NOT NULL DEFAULT 0,
                deliverable_json TEXT,
                verification_transcript TEXT,
                capability_request_json TEXT,
                error_message TEXT,
                created_at TEXT NOT NULL DEFAULT ({now}),
                updated_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_jobs_task_status
                ON jobs(task_id, status, id DESC);

            CREATE TABLE IF NOT EXISTS job_steps (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                job_id INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
                step_index INTEGER NOT NULL,
                phase TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                title TEXT NOT NULL,
                input_json TEXT NOT NULL DEFAULT '{{}}',
                output_json TEXT,
                error_message TEXT,
                started_at TEXT,
                completed_at TEXT,
                UNIQUE(job_id, step_index)
            );

            CREATE INDEX IF NOT EXISTS idx_job_steps_job
                ON job_steps(job_id, step_index);

            CREATE TABLE IF NOT EXISTS job_artifacts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                job_id INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
                artifact_type TEXT NOT NULL,
                title TEXT NOT NULL,
                content TEXT NOT NULL,
                metadata_json TEXT NOT NULL DEFAULT '{{}}',
                created_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_job_artifacts_job
                ON job_artifacts(job_id, id DESC);

            CREATE TABLE IF NOT EXISTS job_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                job_id INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
                event_type TEXT NOT NULL,
                payload_json TEXT NOT NULL DEFAULT '{{}}',
                created_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_job_events_job
                ON job_events(job_id, id DESC);

            CREATE TABLE IF NOT EXISTS job_checkpoints (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                job_id INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
                step_index INTEGER NOT NULL,
                phase TEXT NOT NULL,
                state_json TEXT NOT NULL DEFAULT '{{}}',
                created_at TEXT NOT NULL DEFAULT ({now}),
                UNIQUE(job_id, step_index)
            );

            CREATE INDEX IF NOT EXISTS idx_job_checkpoints_job
                ON job_checkpoints(job_id, step_index);

            CREATE TABLE IF NOT EXISTS job_steering (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                job_id INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
                message TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                boundary_step_index INTEGER,
                created_at TEXT NOT NULL DEFAULT ({now}),
                applied_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_job_steering_job
                ON job_steering(job_id, status, id ASC);

            CREATE TABLE IF NOT EXISTS standing_jobs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                goal_contract TEXT NOT NULL,
                schedule_spec TEXT NOT NULL,
                trigger_kind TEXT NOT NULL,
                next_run_at TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                critical INTEGER NOT NULL DEFAULT 0,
                last_job_id INTEGER REFERENCES jobs(id) ON DELETE SET NULL,
                created_at TEXT NOT NULL DEFAULT ({now}),
                updated_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_standing_jobs_due
                ON standing_jobs(enabled, trigger_kind, next_run_at, id ASC);
            CREATE INDEX IF NOT EXISTS idx_standing_jobs_task
                ON standing_jobs(task_id, id DESC);

            CREATE TABLE IF NOT EXISTS speculation_cache (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                request_signature TEXT NOT NULL,
                request_text TEXT NOT NULL,
                job_id INTEGER REFERENCES jobs(id) ON DELETE SET NULL,
                artifact_json TEXT,
                status TEXT NOT NULL DEFAULT 'fresh',
                created_at TEXT NOT NULL DEFAULT ({now}),
                invalidated_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_speculation_cache_sig
                ON speculation_cache(request_signature, status, id DESC);
            CREATE INDEX IF NOT EXISTS idx_speculation_cache_task
                ON speculation_cache(task_id, id DESC);

            CREATE TABLE IF NOT EXISTS speculation_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER REFERENCES tasks(id) ON DELETE CASCADE,
                kind TEXT NOT NULL,
                request_signature TEXT,
                created_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_speculation_events_kind
                ON speculation_events(kind, id DESC);

            CREATE TABLE IF NOT EXISTS capability_gaps (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                surface TEXT NOT NULL,
                description TEXT NOT NULL,
                occurrence_count INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL DEFAULT ({now}),
                updated_at TEXT NOT NULL DEFAULT ({now}),
                UNIQUE(surface, description)
            );

            CREATE INDEX IF NOT EXISTS idx_capability_gaps_count
                ON capability_gaps(occurrence_count DESC, id DESC);

            CREATE TABLE IF NOT EXISTS custom_tools (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                kind TEXT NOT NULL,
                purpose TEXT NOT NULL,
                target_allowlist_json TEXT NOT NULL DEFAULT '[]',
                spec_json TEXT NOT NULL,
                code TEXT NOT NULL,
                test_transcript TEXT,
                status TEXT NOT NULL DEFAULT 'staged',
                gap_id INTEGER REFERENCES capability_gaps(id) ON DELETE SET NULL,
                created_at TEXT NOT NULL DEFAULT ({now}),
                updated_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_custom_tools_status
                ON custom_tools(status, id DESC);

            CREATE TABLE IF NOT EXISTS tool_connections (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                transport TEXT NOT NULL,
                endpoint TEXT NOT NULL,
                scopes_json TEXT NOT NULL DEFAULT '[]',
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL DEFAULT ({now}),
                updated_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE TABLE IF NOT EXISTS tool_connection_tools (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                connection_id INTEGER NOT NULL REFERENCES tool_connections(id) ON DELETE CASCADE,
                tool_name TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT ({now}),
                UNIQUE(connection_id, tool_name)
            );

            CREATE TABLE IF NOT EXISTS tool_call_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                connection_id INTEGER REFERENCES tool_connections(id) ON DELETE SET NULL,
                connection_name TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                argument_summary TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_tool_call_log_recent
                ON tool_call_log(id DESC);

            CREATE TABLE IF NOT EXISTS web_query_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                query TEXT NOT NULL,
                tool TEXT NOT NULL,
                result_count INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_web_query_log_recent
                ON web_query_log(id DESC);

            CREATE TABLE IF NOT EXISTS email_reply_watches (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER REFERENCES tasks(id) ON DELETE SET NULL,
                sender TEXT NOT NULL,
                thread_hint TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'watching',
                created_at TEXT NOT NULL DEFAULT ({now}),
                resolved_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_email_reply_watches_active
                ON email_reply_watches(status, id DESC);

            CREATE TABLE IF NOT EXISTS remote_ingested_docs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                title TEXT NOT NULL,
                url TEXT NOT NULL,
                provenance TEXT NOT NULL,
                artifact_id INTEGER REFERENCES artifacts(id) ON DELETE SET NULL,
                created_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_remote_ingested_docs_task
                ON remote_ingested_docs(task_id, id DESC);
            "#,
            now = SQLITE_NOW_EXPR,
            embedding_model = crate::providers::OPENAI_EMBEDDING_MODEL_ID,
        ))
        .context("failed to initialize sqlite schema")?;

        // D1/D8 hardening migrations. These are intentionally additive so an
        // existing user database can be upgraded without rebuilding it.
        let _ = conn.execute_batch(
            "ALTER TABLE action_receipts ADD COLUMN outcome_accounted_at TEXT;",
        );
        let _ = conn.execute_batch(
            "ALTER TABLE subtask_write_audit_log ADD COLUMN action_receipt_id INTEGER REFERENCES action_receipts(id) ON DELETE SET NULL;",
        );
        let _ = conn.execute_batch(
            "ALTER TABLE speculation_events ADD COLUMN task_id INTEGER REFERENCES tasks(id) ON DELETE CASCADE;",
        );

        // idempotent migration for older subtasks schema without phase 16 columns.
        let _ = conn
            .execute_batch("ALTER TABLE subtasks ADD COLUMN max_steps INTEGER NOT NULL DEFAULT 5;");
        let _ = conn.execute_batch(
            "ALTER TABLE subtasks ADD COLUMN current_step INTEGER NOT NULL DEFAULT 0;",
        );

        // phase 23: user profile memory + live edit receipts (idempotent)
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS user_profile (
                key        TEXT PRIMARY KEY,
                value      TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
            );
            CREATE TABLE IF NOT EXISTS live_edit_receipts (
                id             INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id        INTEGER REFERENCES tasks(id) ON DELETE SET NULL,
                action_receipt_id INTEGER REFERENCES action_receipts(id) ON DELETE SET NULL,
                editor_surface TEXT NOT NULL,
                document_title TEXT NOT NULL,
                before_hash    TEXT NOT NULL,
                after_hash     TEXT NOT NULL,
                before_text    TEXT NOT NULL DEFAULT '',
                after_text     TEXT NOT NULL DEFAULT '',
                timestamp      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
                status         TEXT NOT NULL DEFAULT 'pending_approval'
            );",
        )
        .context("failed to create phase 23 tables")?;

        // apex: backfill legacy receipts now that live_edit_receipts exists.
        conn.execute_batch(
            "INSERT INTO action_receipts
                (task_id, class, surface, level, description, payload_excerpt,
                 status, failure_reason, undo_ref, created_at, resolved_at)
             SELECT task_id, 'doc.replace', editor_surface, 'L1',
                    'Migrated live edit receipt #' || id,
                    before_hash || ' -> ' || after_hash,
                    CASE status
                        WHEN 'pending' THEN 'pending_approval'
                        ELSE status
                    END,
                    NULL, NULL, timestamp,
                    CASE WHEN status IN ('applied','rejected','failed','guided')
                         THEN timestamp ELSE NULL END
             FROM live_edit_receipts
             WHERE action_receipt_id IS NULL AND task_id IS NOT NULL;

             UPDATE live_edit_receipts
             SET action_receipt_id = (
                SELECT ar.id FROM action_receipts ar
                WHERE ar.description = 'Migrated live edit receipt #' || live_edit_receipts.id
                ORDER BY ar.id DESC LIMIT 1
             )
             WHERE action_receipt_id IS NULL AND task_id IS NOT NULL;

             INSERT INTO action_receipts
                (task_id, class, surface, level, description, payload_excerpt,
                 status, failure_reason, undo_ref, created_at, resolved_at)
             SELECT task_id, 'file.write', 'legacy_subtask', 'L1',
                    'Migrated write audit #' || id,
                    proposed_path,
                    CASE action
                        WHEN 'approved' THEN 'applied'
                        WHEN 'rejected' THEN 'rejected'
                        WHEN 'apply_failed' THEN 'failed'
                        ELSE 'guided'
                    END,
                    CASE WHEN action = 'apply_failed' THEN 'legacy apply failed' ELSE NULL END,
                    NULL, resolved_at, resolved_at
             FROM subtask_write_audit_log
             WHERE action_receipt_id IS NULL;

             UPDATE subtask_write_audit_log
             SET action_receipt_id = (
                SELECT ar.id FROM action_receipts ar
                WHERE ar.description = 'Migrated write audit #' || subtask_write_audit_log.id
                ORDER BY ar.id DESC LIMIT 1
             )
             WHERE action_receipt_id IS NULL;",
        )
        .context("failed to backfill legacy mutation receipts")?;

        // idempotent migration for databases created by the first phase 23 pass.
        let _ = conn.execute_batch("ALTER TABLE live_edit_receipts ADD COLUMN task_id INTEGER REFERENCES tasks(id) ON DELETE SET NULL;");
        let _ = conn.execute_batch("ALTER TABLE live_edit_receipts ADD COLUMN action_receipt_id INTEGER REFERENCES action_receipts(id) ON DELETE SET NULL;");
        let _ = conn.execute_batch(
            "ALTER TABLE live_edit_receipts ADD COLUMN before_text TEXT NOT NULL DEFAULT '';",
        );
        let _ = conn.execute_batch(
            "ALTER TABLE live_edit_receipts ADD COLUMN after_text TEXT NOT NULL DEFAULT '';",
        );

        // phase 29: opinionated output — alternative revision linking
        let _ = conn
            .execute_batch("ALTER TABLE artifact_revisions ADD COLUMN parent_revision_id INTEGER;");

        // apex a3: embedding model versioning. Existing chunks were created
        // before local embeddings and are treated as OpenAI small embeddings so
        // retrieval can dual-read and lazily re-embed when the active model id
        // changes.
        let _ = conn.execute_batch(&format!(
            "ALTER TABLE artifact_chunks ADD COLUMN embedding_model TEXT NOT NULL DEFAULT '{}';",
            crate::providers::OPENAI_EMBEDDING_MODEL_ID
        ));

        // apex c1: two-stage judgment. the stage 2 decision (speak/hold/drop),
        // chosen channel, and its reason are recorded alongside the stage 1 log.
        let _ = conn.execute_batch("ALTER TABLE synthesis_log ADD COLUMN stage2_decision TEXT;");
        let _ = conn.execute_batch("ALTER TABLE synthesis_log ADD COLUMN stage2_channel TEXT;");
        let _ = conn.execute_batch("ALTER TABLE synthesis_log ADD COLUMN stage2_reason TEXT;");

        // apex c2: interruption ledger. every delivered interjection records the
        // focus it landed in and the user's reaction, so spacing becomes learned.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS interruption_ledger (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER,
                delivered_at TEXT NOT NULL DEFAULT (datetime('now')),
                reason_type TEXT NOT NULL,
                channel TEXT NOT NULL,
                focus_score REAL NOT NULL DEFAULT 0,
                reaction TEXT,
                reaction_at TEXT
             );
             CREATE INDEX IF NOT EXISTS idx_interruption_ledger_task
                ON interruption_ledger(task_id, id DESC);",
        )
        .context("failed to create interruption_ledger table")?;

        // phase 30: relational understanding.
        conn.execute_batch(&format!(
            "
            CREATE TABLE IF NOT EXISTS stated_goals (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                goal_text TEXT NOT NULL,
                stated_at TEXT NOT NULL DEFAULT ({now}),
                status TEXT NOT NULL DEFAULT 'active',
                updated_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_stated_goals_task_status
                ON stated_goals(task_id, status, updated_at DESC);

            CREATE TABLE IF NOT EXISTS struggle_patterns (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                pattern_text TEXT NOT NULL,
                task_ids_json TEXT NOT NULL DEFAULT '[]',
                first_seen TEXT NOT NULL DEFAULT ({now}),
                last_seen TEXT NOT NULL DEFAULT ({now}),
                occurrence_count INTEGER NOT NULL DEFAULT 1
            );

            CREATE INDEX IF NOT EXISTS idx_struggle_patterns_last_seen
                ON struggle_patterns(last_seen DESC);

            CREATE TABLE IF NOT EXISTS collaboration_style_signals (
                key TEXT PRIMARY KEY,
                value REAL NOT NULL,
                updated_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE TABLE IF NOT EXISTS trust_metrics (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                times_accepted_opinion INTEGER NOT NULL DEFAULT 0,
                times_pushed_back INTEGER NOT NULL DEFAULT 0,
                times_asked_for_more INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL DEFAULT ({now})
            );
            ",
            now = SQLITE_NOW_EXPR,
        ))
        .context("failed to create phase 30 relational tables")?;

        conn.execute(
            "INSERT OR IGNORE INTO trust_metrics
             (id, times_accepted_opinion, times_pushed_back, times_asked_for_more)
             VALUES (1, 0, 0, 0)",
            [],
        )
        .context("failed to initialize trust metrics")?;

        for key in [
            "prefers_opinions",
            "wants_explanations",
            "delegation_comfort",
            "interruption_tolerance",
        ] {
            conn.execute(
                "INSERT OR IGNORE INTO collaboration_style_signals (key, value)
                 VALUES (?1, 0.5)",
                params![key],
            )
            .with_context(|| format!("failed to initialize collaboration style signal {key}"))?;
        }

        // apex b3: typed episodic memory. embeddings are stored as a BLOB of
        // little-endian f32 values so candidate search can stay local.
        conn.execute_batch(&format!(
            "
            CREATE TABLE IF NOT EXISTS episodes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                kind TEXT NOT NULL,
                text TEXT NOT NULL,
                embedding BLOB NOT NULL,
                embedding_model TEXT NOT NULL DEFAULT '',
                salience REAL NOT NULL,
                source TEXT NOT NULL,
                consolidated_at TEXT,
                created_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_episodes_task_kind_created
                ON episodes(task_id, kind, created_at DESC, id DESC);
            CREATE INDEX IF NOT EXISTS idx_episodes_unconsolidated
                ON episodes(consolidated_at, created_at DESC);
            ",
            now = SQLITE_NOW_EXPR,
        ))
        .context("failed to create apex b3 episode tables")?;

        // apex b4: durable consolidated memory facts. embeddings are local-only
        // merge aids; prompt surfaces use the plain-language fact text.
        conn.execute_batch(&format!(
            "
            CREATE TABLE IF NOT EXISTS facts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                text TEXT NOT NULL,
                kind TEXT NOT NULL,
                embedding BLOB NOT NULL,
                embedding_model TEXT NOT NULL DEFAULT '',
                confidence REAL NOT NULL,
                evidence_ids_json TEXT NOT NULL,
                salience REAL NOT NULL,
                last_reinforced TEXT NOT NULL DEFAULT ({now}),
                created_at TEXT NOT NULL DEFAULT ({now})
            );

            CREATE INDEX IF NOT EXISTS idx_facts_kind_salience
                ON facts(kind, salience DESC, last_reinforced DESC, id DESC);
            CREATE INDEX IF NOT EXISTS idx_facts_last_reinforced
                ON facts(last_reinforced ASC);
            ",
            now = SQLITE_NOW_EXPR,
        ))
        .context("failed to create apex b4 fact tables")?;

        Ok(())
    }

    // ---------------------------------------------------------------------
    // task core
    // ---------------------------------------------------------------------

    pub fn create_task(&self, title: &str) -> Result<TaskDto> {
        let clean_title = title.trim();
        if clean_title.is_empty() {
            return Err(anyhow!("task title cannot be empty"));
        }

        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("failed to start task creation transaction")?;

        let base_slug = slugify_title(clean_title);
        let slug = Self::next_available_slug(&tx, &base_slug)?;
        let workspace_path = self.paths.workspace_root.join(&slug);
        fs::create_dir_all(&workspace_path).with_context(|| {
            format!(
                "failed to create task workspace {}",
                workspace_path.display()
            )
        })?;

        let has_active: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM tasks WHERE is_active = 1",
                [],
                |row| row.get(0),
            )
            .context("failed to check active task count")?;
        let should_activate = has_active == 0;

        tx.execute(
            &format!(
                "INSERT INTO tasks (title, slug, workspace_path, created_at, updated_at, is_active)
                 VALUES (?1, ?2, ?3, ({now}), ({now}), ?4)",
                now = SQLITE_NOW_EXPR,
            ),
            params![
                clean_title,
                slug,
                workspace_path.to_string_lossy().to_string(),
                if should_activate { 1 } else { 0 },
            ],
        )
        .context("failed to insert task")?;

        let task_id = tx.last_insert_rowid();

        tx.execute(
            &format!(
                "INSERT INTO task_summaries (task_id, summary_text, updated_at)
                 VALUES (?1, ?2, ({now}))",
                now = SQLITE_NOW_EXPR,
            ),
            params![
                task_id,
                format!("Summary placeholder for '{}'.", clean_title)
            ],
        )
        .context("failed to insert task summary")?;

        tx.execute(
            &format!(
                "INSERT INTO clipboard_capture_settings (task_id, enabled, updated_at)
                 VALUES (?1, 0, ({now}))
                 ON CONFLICT(task_id) DO NOTHING",
                now = SQLITE_NOW_EXPR,
            ),
            params![task_id],
        )
        .context("failed to seed clipboard settings")?;

        Self::record_event_tx(
            &tx,
            task_id,
            "task_created",
            &json!({ "task_id": task_id, "title": clean_title, "slug": slug }).to_string(),
        )?;

        if should_activate {
            Self::record_event_tx(
                &tx,
                task_id,
                "task_activated",
                &json!({ "task_id": task_id }).to_string(),
            )?;
        }

        tx.commit()
            .context("failed to commit task creation transaction")?;

        self.get_task_by_id(task_id)?
            .ok_or_else(|| anyhow!("task was created but could not be reloaded"))
    }

    pub fn list_tasks(&self) -> Result<Vec<TaskDto>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, title, slug, workspace_path, created_at, updated_at, is_active
                 FROM tasks
                 ORDER BY is_active DESC, updated_at DESC, id DESC",
            )
            .context("failed to prepare list_tasks query")?;

        let rows = stmt
            .query_map([], task_from_row)
            .context("failed to query tasks")?;

        let mut tasks = Vec::new();
        for row in rows {
            tasks.push(row.context("failed to map task row")?);
        }
        Ok(tasks)
    }

    pub fn get_active_task(&self) -> Result<Option<TaskDto>> {
        let conn = self.connect()?;
        let task = conn
            .query_row(
                "SELECT id, title, slug, workspace_path, created_at, updated_at, is_active
                 FROM tasks
                 WHERE is_active = 1
                 LIMIT 1",
                [],
                task_from_row,
            )
            .optional()
            .context("failed to query active task")?;
        Ok(task)
    }

    pub fn set_active_task(&self, task_id: i64) -> Result<TaskDto> {
        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("failed to start set_active_task transaction")?;

        let exists: Option<i64> = tx
            .query_row(
                "SELECT id FROM tasks WHERE id = ?1",
                params![task_id],
                |row| row.get(0),
            )
            .optional()
            .context("failed to verify task exists")?;
        if exists.is_none() {
            return Err(anyhow!("task id={} not found", task_id));
        }

        tx.execute("UPDATE tasks SET is_active = 0 WHERE is_active = 1", [])
            .context("failed to deactivate current active task")?;

        tx.execute(
            &format!(
                "UPDATE tasks
                 SET is_active = 1, updated_at = ({now})
                 WHERE id = ?1",
                now = SQLITE_NOW_EXPR,
            ),
            params![task_id],
        )
        .context("failed to activate task")?;

        Self::record_event_tx(
            &tx,
            task_id,
            "task_activated",
            &json!({ "task_id": task_id }).to_string(),
        )?;

        tx.commit()
            .context("failed to commit set_active_task transaction")?;

        self.get_task_by_id(task_id)?
            .ok_or_else(|| anyhow!("activated task could not be reloaded"))
    }

    pub fn get_task_workspace(&self, task_id: i64) -> Result<WorkspaceInfoDto> {
        let task = self
            .get_task_by_id(task_id)?
            .ok_or_else(|| anyhow!("task id={} not found", task_id))?;

        Ok(WorkspaceInfoDto {
            task_id: task.id,
            slug: task.slug,
            workspace_path: task.workspace_path.clone(),
            exists_on_disk: Path::new(&task.workspace_path).exists(),
        })
    }

    pub fn get_task_workspace_path(&self, task_id: i64) -> Result<PathBuf> {
        let workspace = self.get_task_workspace(task_id)?;
        Ok(PathBuf::from(workspace.workspace_path))
    }

    pub fn get_task_summary(&self, task_id: i64) -> Result<TaskSummaryDto> {
        let conn = self.connect()?;

        if let Some(summary) = conn
            .query_row(
                "SELECT task_id, summary_text, updated_at FROM task_summaries WHERE task_id = ?1",
                params![task_id],
                task_summary_from_row,
            )
            .optional()
            .context("failed to query task summary")?
        {
            return Ok(summary);
        }

        let task = self
            .get_task_by_id(task_id)?
            .ok_or_else(|| anyhow!("task id={} not found", task_id))?;

        conn.execute(
            &format!(
                "INSERT INTO task_summaries (task_id, summary_text, updated_at)
                 VALUES (?1, ?2, ({now}))",
                now = SQLITE_NOW_EXPR,
            ),
            params![
                task_id,
                format!("Summary placeholder for '{}'.", task.title)
            ],
        )
        .context("failed to auto-seed task summary")?;

        conn.query_row(
            "SELECT task_id, summary_text, updated_at FROM task_summaries WHERE task_id = ?1",
            params![task_id],
            task_summary_from_row,
        )
        .context("failed to reload seeded task summary")
    }

    pub fn list_open_resources(&self, task_id: i64) -> Result<Vec<OpenResourceDto>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, task_id, resource_type, resource_path_or_url, label, position_index
                 FROM open_resources
                 WHERE task_id = ?1
                 ORDER BY position_index ASC, id ASC",
            )
            .context("failed to prepare list_open_resources query")?;

        let rows = stmt
            .query_map(params![task_id], open_resource_from_row)
            .context("failed to query open resources")?;

        let mut resources = Vec::new();
        for row in rows {
            resources.push(row.context("failed to map open resource row")?);
        }
        Ok(resources)
    }

    // ---------------------------------------------------------------------
    // artifacts + retrieval support
    // ---------------------------------------------------------------------

    pub fn insert_artifact_with_chunks(
        &self,
        task_id: i64,
        file_name: &str,
        file_extension: &str,
        original_path: &str,
        stored_path: &str,
        chunks: &[ChunkEmbeddingInput],
    ) -> Result<ArtifactDto> {
        let clean_file_name = file_name.trim();
        if clean_file_name.is_empty() {
            return Err(anyhow!("file_name cannot be empty"));
        }

        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("failed to start insert_artifact_with_chunks transaction")?;

        let _task_exists: i64 = tx
            .query_row(
                "SELECT id FROM tasks WHERE id = ?1",
                params![task_id],
                |row| row.get(0),
            )
            .optional()
            .context("failed to verify task exists for artifact insert")?
            .ok_or_else(|| anyhow!("task id={} not found", task_id))?;

        tx.execute(
            &format!(
                "INSERT INTO artifacts
                 (task_id, file_name, file_extension, original_path, stored_path, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ({now}), ({now}))",
                now = SQLITE_NOW_EXPR,
            ),
            params![
                task_id,
                clean_file_name,
                file_extension.trim(),
                original_path.trim(),
                stored_path.trim(),
            ],
        )
        .context("failed to insert artifact row")?;

        let artifact_id = tx.last_insert_rowid();

        for chunk in chunks {
            let embedding_json = serde_json::to_string(&chunk.embedding)
                .context("failed to serialize chunk embedding")?;
            tx.execute(
                "INSERT INTO artifact_chunks (task_id, artifact_id, chunk_text, position_index, embedding_json, embedding_model)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    task_id,
                    artifact_id,
                    chunk.chunk_text,
                    chunk.position_index,
                    embedding_json,
                    chunk.embedding_model,
                ],
            )
            .context("failed to insert artifact chunk")?;
        }

        Self::record_event_tx(
            &tx,
            task_id,
            "artifact_imported",
            &json!({
                "artifact_id": artifact_id,
                "file_name": clean_file_name,
                "chunk_count": chunks.len(),
            })
            .to_string(),
        )?;

        tx.commit()
            .context("failed to commit insert_artifact_with_chunks transaction")?;

        self.get_artifact_by_id(artifact_id)?
            .ok_or_else(|| anyhow!("artifact was inserted but could not be reloaded"))
    }

    pub fn list_artifacts(&self, task_id: i64) -> Result<Vec<ArtifactDto>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT a.id,
                        a.task_id,
                        a.file_name,
                        a.file_extension,
                        a.original_path,
                        a.stored_path,
                        a.created_at,
                        a.updated_at,
                        COUNT(c.id) AS chunk_count
                 FROM artifacts a
                 LEFT JOIN artifact_chunks c ON c.artifact_id = a.id
                 WHERE a.task_id = ?1
                 GROUP BY a.id
                 ORDER BY a.id DESC",
            )
            .context("failed to prepare list_artifacts query")?;

        let rows = stmt
            .query_map(params![task_id], artifact_from_row)
            .context("failed to query artifacts")?;

        let mut artifacts = Vec::new();
        for row in rows {
            artifacts.push(row.context("failed to map artifact row")?);
        }
        Ok(artifacts)
    }

    pub fn fetch_chunk_embeddings_for_task(
        &self,
        task_id: i64,
    ) -> Result<Vec<StoredChunkEmbedding>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT c.id,
                        c.task_id,
                        c.artifact_id,
                        a.file_name,
                        a.stored_path,
                        c.chunk_text,
                        c.position_index,
                        c.embedding_json,
                        c.embedding_model
                 FROM artifact_chunks c
                 JOIN artifacts a ON a.id = c.artifact_id
                 WHERE c.task_id = ?1
                 ORDER BY c.artifact_id ASC, c.position_index ASC, c.id ASC",
            )
            .context("failed to prepare fetch_chunk_embeddings_for_task query")?;

        let rows = stmt
            .query_map(params![task_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                ))
            })
            .context("failed to query chunk embeddings")?;

        let mut chunks = Vec::new();
        for row in rows {
            let (
                chunk_id,
                row_task_id,
                artifact_id,
                artifact_file_name,
                artifact_stored_path,
                chunk_text,
                position_index,
                embedding_json,
                embedding_model,
            ) = row.context("failed to decode stored chunk row")?;

            let embedding: Vec<f32> = serde_json::from_str(&embedding_json)
                .context("failed to deserialize stored embedding json")?;

            chunks.push(StoredChunkEmbedding {
                chunk_id,
                task_id: row_task_id,
                artifact_id,
                artifact_file_name,
                artifact_stored_path,
                chunk_text,
                position_index,
                embedding,
                embedding_model,
            });
        }

        Ok(chunks)
    }

    pub fn update_chunk_embedding(
        &self,
        chunk_id: i64,
        embedding: &[f32],
        embedding_model: &str,
    ) -> Result<()> {
        let conn = self.connect()?;
        let embedding_json = serde_json::to_string(embedding)
            .context("failed to serialize updated chunk embedding")?;
        let changed = conn
            .execute(
                "UPDATE artifact_chunks
                 SET embedding_json = ?1, embedding_model = ?2
                 WHERE id = ?3",
                params![embedding_json, embedding_model.trim(), chunk_id],
            )
            .context("failed to update chunk embedding")?;
        if changed == 0 {
            return Err(anyhow!("chunk id={} not found", chunk_id));
        }
        Ok(())
    }

    pub fn replace_artifact_chunks(
        &self,
        task_id: i64,
        artifact_id: i64,
        chunks: &[ChunkEmbeddingInput],
    ) -> Result<()> {
        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("failed to start replace_artifact_chunks transaction")?;

        let owner_task_id: Option<i64> = tx
            .query_row(
                "SELECT task_id FROM artifacts WHERE id = ?1",
                params![artifact_id],
                |row| row.get(0),
            )
            .optional()
            .context("failed to verify artifact owner task")?;

        match owner_task_id {
            Some(id) if id == task_id => {}
            Some(id) => {
                return Err(anyhow!(
                    "artifact id={} belongs to task id={}, not task id={}",
                    artifact_id,
                    id,
                    task_id
                ));
            }
            None => return Err(anyhow!("artifact id={} not found", artifact_id)),
        }

        tx.execute(
            "DELETE FROM artifact_chunks WHERE artifact_id = ?1",
            params![artifact_id],
        )
        .context("failed to delete previous artifact chunks")?;

        for chunk in chunks {
            let embedding_json = serde_json::to_string(&chunk.embedding)
                .context("failed to serialize replacement embedding")?;
            tx.execute(
                "INSERT INTO artifact_chunks (task_id, artifact_id, chunk_text, position_index, embedding_json, embedding_model)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    task_id,
                    artifact_id,
                    chunk.chunk_text,
                    chunk.position_index,
                    embedding_json,
                    chunk.embedding_model,
                ],
            )
            .context("failed to insert replacement artifact chunk")?;
        }

        tx.execute(
            &format!(
                "UPDATE artifacts SET updated_at = ({now}) WHERE id = ?1",
                now = SQLITE_NOW_EXPR,
            ),
            params![artifact_id],
        )
        .context("failed to touch artifact updated_at during reindex")?;

        Self::record_event_tx(
            &tx,
            task_id,
            "artifact_reindexed",
            &json!({ "artifact_id": artifact_id, "chunk_count": chunks.len() }).to_string(),
        )?;

        tx.commit()
            .context("failed to commit replace_artifact_chunks transaction")?;

        Ok(())
    }

    pub fn get_artifact_content(&self, artifact_id: i64) -> Result<ArtifactContentDto> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, task_id, file_name, file_extension, stored_path
                 FROM artifacts
                 WHERE id = ?1",
            )
            .context("failed to prepare get_artifact_content query")?;

        let artifact_row: Option<(i64, i64, String, String, String)> = stmt
            .query_row(params![artifact_id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .optional()
            .context("failed to query artifact content metadata")?;

        let (id, task_id, file_name, file_extension, stored_path) =
            artifact_row.ok_or_else(|| anyhow!("artifact id={} not found", artifact_id))?;

        let editable = matches!(file_extension.to_ascii_lowercase().as_str(), "md" | "txt");
        let content = fs::read_to_string(&stored_path).unwrap_or_default();

        Ok(ArtifactContentDto {
            artifact_id: id,
            task_id,
            file_name,
            file_extension,
            stored_path,
            content,
            is_editable: editable,
        })
    }

    pub fn touch_artifact_updated_at(&self, artifact_id: i64) -> Result<()> {
        let conn = self.connect()?;
        let changed = conn
            .execute(
                &format!(
                    "UPDATE artifacts SET updated_at = ({now}) WHERE id = ?1",
                    now = SQLITE_NOW_EXPR,
                ),
                params![artifact_id],
            )
            .context("failed to touch artifact updated_at")?;

        if changed == 0 {
            return Err(anyhow!("artifact id={} not found", artifact_id));
        }

        Ok(())
    }

    // ---------------------------------------------------------------------
    // chat messages + streaming placeholders
    // ---------------------------------------------------------------------

    pub fn append_chat_message(
        &self,
        task_id: i64,
        role: &str,
        message_source: &str,
        message_kind: MessageKind,
        content: &str,
    ) -> Result<ChatMessageDto> {
        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("failed to start append_chat_message transaction")?;

        tx.execute(
            &format!(
                "INSERT INTO chat_messages
                 (task_id, session_id, role, message_source, message_kind, content, created_at)
                 VALUES (?1, NULL, ?2, ?3, ?4, ?5, ({now}))",
                now = SQLITE_NOW_EXPR,
            ),
            params![
                task_id,
                role.trim(),
                message_source.trim(),
                message_kind.as_str(),
                content
            ],
        )
        .context("failed to insert chat message")?;

        let message_id = tx.last_insert_rowid();

        Self::record_event_tx(
            &tx,
            task_id,
            "message_appended",
            &json!({
                "message_id": message_id,
                "role": role,
                "message_kind": message_kind.as_str(),
            })
            .to_string(),
        )?;

        tx.commit()
            .context("failed to commit append_chat_message transaction")?;

        self.get_chat_message_by_id(message_id)?
            .ok_or_else(|| anyhow!("chat message id={} missing after insert", message_id))
    }

    pub fn list_chat_messages(&self, task_id: i64) -> Result<Vec<ChatMessageDto>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, task_id, session_id, role, message_source, message_kind, content, created_at
                 FROM chat_messages
                 WHERE task_id = ?1
                 ORDER BY id ASC",
            )
            .context("failed to prepare list_chat_messages query")?;

        let rows = stmt
            .query_map(params![task_id], chat_message_from_row)
            .context("failed to query chat messages")?;

        let mut messages = Vec::new();
        for row in rows {
            messages.push(row.context("failed to map chat message row")?);
        }
        Ok(messages)
    }

    pub fn list_recent_chat_messages(
        &self,
        task_id: i64,
        limit: usize,
    ) -> Result<Vec<ChatMessageDto>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, task_id, session_id, role, message_source, message_kind, content, created_at
                 FROM chat_messages
                 WHERE task_id = ?1
                 ORDER BY id DESC
                 LIMIT ?2",
            )
            .context("failed to prepare list_recent_chat_messages query")?;

        let rows = stmt
            .query_map(params![task_id, limit as i64], chat_message_from_row)
            .context("failed to query recent chat messages")?;

        let mut messages = Vec::new();
        for row in rows {
            messages.push(row.context("failed to map recent chat message row")?);
        }
        messages.reverse();
        Ok(messages)
    }

    pub fn insert_streaming_placeholder(&self, task_id: i64) -> Result<i64> {
        let conn = self.connect()?;
        conn.execute(
            &format!(
                "INSERT INTO chat_messages
                 (task_id, session_id, role, message_source, message_kind, content, created_at)
                 VALUES (?1, NULL, 'assistant', 'assistant', 'assistant_partial', '', ({now}))",
                now = SQLITE_NOW_EXPR,
            ),
            params![task_id],
        )
        .context("failed to insert streaming placeholder")?;
        Ok(conn.last_insert_rowid())
    }

    pub fn finalize_streaming_message(
        &self,
        message_id: i64,
        content: &str,
        final_kind: MessageKind,
    ) -> Result<()> {
        let final_content = if matches!(final_kind, MessageKind::AssistantInterrupted)
            && content.trim().is_empty()
        {
            "(interrupted)".to_string()
        } else {
            content.to_string()
        };

        let conn = self.connect()?;
        let changed = conn
            .execute(
                "UPDATE chat_messages
                 SET content = ?1, message_kind = ?2, message_source = 'assistant'
                 WHERE id = ?3",
                params![final_content, final_kind.as_str(), message_id],
            )
            .context("failed to finalize streaming placeholder")?;
        if changed == 0 {
            return Err(anyhow!("streaming placeholder id={} not found", message_id));
        }
        Ok(())
    }

    // ---------------------------------------------------------------------
    // revisions + versions
    // ---------------------------------------------------------------------

    pub fn create_revision_proposal(
        &self,
        input: &NewRevisionProposalInput,
    ) -> Result<RevisionProposalDto> {
        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("failed to start create_revision_proposal transaction")?;

        tx.execute(
            &format!(
                "INSERT INTO artifact_revisions
                 (
                   task_id,
                   artifact_id,
                   target_start_offset,
                   target_end_offset,
                   target_description,
                   original_text,
                   proposed_text,
                   instruction_text,
                   instruction_source,
                   rationale,
                   grounding_notes,
                   retrieval_confidence,
                   parent_revision_id,
                   status,
                   created_at,
                   updated_at
                 )
                 VALUES
                 (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 'pending', ({now}), ({now}))",
                now = SQLITE_NOW_EXPR,
            ),
            params![
                input.task_id,
                input.artifact_id,
                input.target_start_offset,
                input.target_end_offset,
                input.target_description,
                input.original_text,
                input.proposed_text,
                input.instruction_text,
                input.instruction_source,
                input.rationale,
                input.grounding_notes,
                input.retrieval_confidence,
                input.parent_revision_id,
            ],
        )
        .context("failed to insert revision proposal")?;

        let revision_id = tx.last_insert_rowid();

        Self::record_event_tx(
            &tx,
            input.task_id,
            "revision_proposed",
            &json!({ "revision_id": revision_id, "artifact_id": input.artifact_id }).to_string(),
        )?;

        tx.commit()
            .context("failed to commit create_revision_proposal transaction")?;

        self.get_revision_by_id(revision_id)?
            .ok_or_else(|| anyhow!("revision id={} missing after insert", revision_id))
    }

    pub fn list_pending_revisions(
        &self,
        task_id: i64,
        artifact_id: i64,
    ) -> Result<Vec<RevisionProposalDto>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT revision_id,
                        task_id,
                        artifact_id,
                        target_start_offset,
                        target_end_offset,
                        target_description,
                        original_text,
                        proposed_text,
                        instruction_text,
                        instruction_source,
                        rationale,
                        grounding_notes,
                        retrieval_confidence,
                        status,
                        created_at,
                        updated_at,
                        parent_revision_id
                 FROM artifact_revisions
                 WHERE task_id = ?1
                   AND artifact_id = ?2
                   AND status = 'pending'
                 ORDER BY revision_id DESC",
            )
            .context("failed to prepare list_pending_revisions query")?;

        let rows = stmt
            .query_map(params![task_id, artifact_id], revision_from_row)
            .context("failed to query pending revisions")?;

        let mut revisions = Vec::new();
        for row in rows {
            revisions.push(row.context("failed to map pending revision row")?);
        }
        Ok(revisions)
    }

    pub fn list_pending_revisions_for_task(
        &self,
        task_id: i64,
    ) -> Result<Vec<RevisionProposalDto>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT revision_id,
                        task_id,
                        artifact_id,
                        target_start_offset,
                        target_end_offset,
                        target_description,
                        original_text,
                        proposed_text,
                        instruction_text,
                        instruction_source,
                        rationale,
                        grounding_notes,
                        retrieval_confidence,
                        status,
                        created_at,
                        updated_at,
                        parent_revision_id
                 FROM artifact_revisions
                 WHERE task_id = ?1
                   AND status = 'pending'
                 ORDER BY revision_id DESC",
            )
            .context("failed to prepare list_pending_revisions_for_task query")?;

        let rows = stmt
            .query_map(params![task_id], revision_from_row)
            .context("failed to query pending revisions for task")?;

        let mut revisions = Vec::new();
        for row in rows {
            revisions.push(row.context("failed to map task pending revision row")?);
        }
        Ok(revisions)
    }

    pub fn get_revision_by_id(&self, revision_id: i64) -> Result<Option<RevisionProposalDto>> {
        let conn = self.connect()?;
        let revision = conn
            .query_row(
                "SELECT revision_id,
                        task_id,
                        artifact_id,
                        target_start_offset,
                        target_end_offset,
                        target_description,
                        original_text,
                        proposed_text,
                        instruction_text,
                        instruction_source,
                        rationale,
                        grounding_notes,
                        retrieval_confidence,
                        status,
                        created_at,
                        updated_at,
                        parent_revision_id
                 FROM artifact_revisions
                 WHERE revision_id = ?1",
                params![revision_id],
                revision_from_row,
            )
            .optional()
            .context("failed to query revision by id")?;
        Ok(revision)
    }

    pub fn list_alternative_revisions(
        &self,
        parent_revision_id: i64,
    ) -> Result<Vec<RevisionProposalDto>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT revision_id,
                        task_id,
                        artifact_id,
                        target_start_offset,
                        target_end_offset,
                        target_description,
                        original_text,
                        proposed_text,
                        instruction_text,
                        instruction_source,
                        rationale,
                        grounding_notes,
                        retrieval_confidence,
                        status,
                        created_at,
                        updated_at,
                        parent_revision_id
                 FROM artifact_revisions
                 WHERE parent_revision_id = ?1
                 ORDER BY revision_id ASC",
            )
            .context("failed to prepare list_alternative_revisions query")?;

        let rows = stmt
            .query_map(params![parent_revision_id], revision_from_row)
            .context("failed to query alternative revisions")?;

        let mut revisions = Vec::new();
        for row in rows {
            revisions.push(row.context("failed to map alternative revision row")?);
        }
        Ok(revisions)
    }

    pub fn set_revision_status(
        &self,
        revision_id: i64,
        status: &str,
    ) -> Result<RevisionProposalDto> {
        let conn = self.connect()?;
        let changed = conn
            .execute(
                &format!(
                    "UPDATE artifact_revisions
                     SET status = ?1, updated_at = ({now})
                     WHERE revision_id = ?2",
                    now = SQLITE_NOW_EXPR,
                ),
                params![status.trim(), revision_id],
            )
            .context("failed to update revision status")?;

        if changed == 0 {
            return Err(anyhow!("revision id={} not found", revision_id));
        }

        self.get_revision_by_id(revision_id)?
            .ok_or_else(|| anyhow!("revision disappeared after status update"))
    }

    pub fn create_artifact_version(
        &self,
        input: &NewArtifactVersionInput,
    ) -> Result<ArtifactVersionDto> {
        let preview = compact_preview(&input.content_snapshot, 240);
        let content_length = input.content_snapshot.chars().count() as i64;

        let conn = self.connect()?;
        conn.execute(
            &format!(
                "INSERT INTO artifact_versions
                 (
                    task_id,
                    artifact_id,
                    revision_id,
                    version_reason,
                    content_snapshot,
                    stored_path,
                    content_preview,
                    content_length,
                    created_at
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ({now}))",
                now = SQLITE_NOW_EXPR,
            ),
            params![
                input.task_id,
                input.artifact_id,
                input.revision_id,
                input.version_reason,
                input.content_snapshot,
                input.stored_path,
                preview,
                content_length,
            ],
        )
        .context("failed to insert artifact version")?;

        let version_id = conn.last_insert_rowid();
        self.get_artifact_version_by_id(version_id)?
            .ok_or_else(|| anyhow!("artifact version id={} missing after insert", version_id))
    }

    pub fn list_artifact_versions(&self, artifact_id: i64) -> Result<Vec<ArtifactVersionDto>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT version_id,
                        task_id,
                        artifact_id,
                        revision_id,
                        version_reason,
                        content_preview,
                        content_length,
                        created_at
                 FROM artifact_versions
                 WHERE artifact_id = ?1
                 ORDER BY version_id DESC",
            )
            .context("failed to prepare list_artifact_versions query")?;

        let rows = stmt
            .query_map(params![artifact_id], artifact_version_from_row)
            .context("failed to query artifact versions")?;

        let mut versions = Vec::new();
        for row in rows {
            versions.push(row.context("failed to map artifact version row")?);
        }
        Ok(versions)
    }

    pub fn get_artifact_version_snapshot(
        &self,
        version_id: i64,
    ) -> Result<Option<ArtifactVersionSnapshot>> {
        let conn = self.connect()?;
        let row = conn
            .query_row(
                "SELECT version_id,
                        task_id,
                        artifact_id,
                        revision_id,
                        version_reason,
                        content_snapshot,
                        stored_path,
                        content_preview,
                        content_length,
                        created_at
                 FROM artifact_versions
                 WHERE version_id = ?1",
                params![version_id],
                |row| {
                    Ok((
                        ArtifactVersionDto {
                            version_id: row.get(0)?,
                            task_id: row.get(1)?,
                            artifact_id: row.get(2)?,
                            revision_id: row.get(3)?,
                            version_reason: row.get(4)?,
                            content_preview: row.get(7)?,
                            content_length: row.get(8)?,
                            created_at: row.get(9)?,
                        },
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()
            .context("failed to query artifact version snapshot")?;

        Ok(row.map(
            |(dto, content_snapshot, stored_path)| ArtifactVersionSnapshot {
                dto,
                content_snapshot,
                stored_path,
            },
        ))
    }

    // ---------------------------------------------------------------------
    // subtasks + chain steps + file proposals
    // ---------------------------------------------------------------------

    pub fn create_subtask(&self, input: &NewSubTaskInput) -> Result<SubTaskDto> {
        let clean_title = input.title.trim();
        let clean_description = input.description.trim();
        if clean_title.is_empty() {
            return Err(anyhow!("subtask title cannot be empty"));
        }
        if clean_description.is_empty() {
            return Err(anyhow!("subtask description cannot be empty"));
        }

        let conn = self.connect()?;
        conn.execute(
            &format!(
                "INSERT INTO subtasks
                 (
                    task_id,
                    title,
                    description,
                    execution_type,
                    status,
                    result_review_status,
                    created_at,
                    updated_at,
                    result_summary,
                    result_payload,
                    instruction_source,
                    parent_context_snapshot,
                    error_message,
                    max_steps,
                    current_step
                 )
                 VALUES
                 (?1, ?2, ?3, ?4, 'pending', 'unreviewed', ({now}), ({now}), NULL, NULL, ?5, ?6, NULL, 5, 0)",
                now = SQLITE_NOW_EXPR,
            ),
            params![
                input.task_id,
                clean_title,
                clean_description,
                input.execution_type.trim(),
                input.instruction_source.trim(),
                input.parent_context_snapshot,
            ],
        )
        .context("failed to insert subtask")?;

        let subtask_id = conn.last_insert_rowid();
        self.get_subtask_by_id(subtask_id)?
            .ok_or_else(|| anyhow!("subtask id={} missing after insert", subtask_id))
    }

    pub fn list_subtasks(&self, task_id: i64) -> Result<Vec<SubTaskDto>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT subtask_id,
                        task_id,
                        title,
                        description,
                        execution_type,
                        status,
                        result_review_status,
                        created_at,
                        updated_at,
                        result_summary,
                        result_payload,
                        instruction_source,
                        parent_context_snapshot,
                        error_message
                 FROM subtasks
                 WHERE task_id = ?1
                 ORDER BY subtask_id DESC",
            )
            .context("failed to prepare list_subtasks query")?;

        let rows = stmt
            .query_map(params![task_id], subtask_from_row)
            .context("failed to query subtasks")?;

        let mut subtasks = Vec::new();
        for row in rows {
            subtasks.push(row.context("failed to map subtask row")?);
        }
        Ok(subtasks)
    }

    pub fn get_subtask_by_id(&self, subtask_id: i64) -> Result<Option<SubTaskDto>> {
        let conn = self.connect()?;
        let subtask = conn
            .query_row(
                "SELECT subtask_id,
                        task_id,
                        title,
                        description,
                        execution_type,
                        status,
                        result_review_status,
                        created_at,
                        updated_at,
                        result_summary,
                        result_payload,
                        instruction_source,
                        parent_context_snapshot,
                        error_message
                 FROM subtasks
                 WHERE subtask_id = ?1",
                params![subtask_id],
                subtask_from_row,
            )
            .optional()
            .context("failed to query subtask by id")?;

        Ok(subtask)
    }

    pub fn transition_subtask_status(
        &self,
        subtask_id: i64,
        status: &str,
        result_summary: Option<&str>,
        result_payload: Option<&str>,
        error_message: Option<&str>,
    ) -> Result<SubTaskDto> {
        let existing = self
            .get_subtask_by_id(subtask_id)?
            .ok_or_else(|| anyhow!("subtask id={} not found", subtask_id))?;

        let next_summary = result_summary
            .map(|value| value.to_string())
            .or(existing.result_summary.clone());
        let next_payload = result_payload
            .map(|value| value.to_string())
            .or(existing.result_payload.clone());
        let next_error = error_message
            .map(|value| value.to_string())
            .or(existing.error_message.clone());

        let conn = self.connect()?;
        conn.execute(
            &format!(
                "UPDATE subtasks
                 SET status = ?1,
                     result_summary = ?2,
                     result_payload = ?3,
                     error_message = ?4,
                     updated_at = ({now})
                 WHERE subtask_id = ?5",
                now = SQLITE_NOW_EXPR,
            ),
            params![
                status.trim(),
                next_summary,
                next_payload,
                next_error,
                subtask_id
            ],
        )
        .context("failed to transition subtask status")?;

        self.get_subtask_by_id(subtask_id)?
            .ok_or_else(|| anyhow!("subtask disappeared after status transition"))
    }

    pub fn set_subtask_result_review_status(
        &self,
        subtask_id: i64,
        status: &str,
    ) -> Result<SubTaskDto> {
        let conn = self.connect()?;
        let changed = conn
            .execute(
                &format!(
                    "UPDATE subtasks
                     SET result_review_status = ?1,
                         updated_at = ({now})
                     WHERE subtask_id = ?2",
                    now = SQLITE_NOW_EXPR,
                ),
                params![status.trim(), subtask_id],
            )
            .context("failed to set subtask review status")?;

        if changed == 0 {
            return Err(anyhow!("subtask id={} not found", subtask_id));
        }

        self.get_subtask_by_id(subtask_id)?
            .ok_or_else(|| anyhow!("subtask disappeared after review status update"))
    }

    pub fn update_subtask_current_step(&self, subtask_id: i64, current_step: i64) -> Result<()> {
        let conn = self.connect()?;
        let changed = conn
            .execute(
                &format!(
                    "UPDATE subtasks
                     SET current_step = ?1,
                         updated_at = ({now})
                     WHERE subtask_id = ?2",
                    now = SQLITE_NOW_EXPR,
                ),
                params![current_step, subtask_id],
            )
            .context("failed to update subtask current_step")?;

        if changed == 0 {
            return Err(anyhow!("subtask id={} not found", subtask_id));
        }

        Ok(())
    }

    pub fn create_subtask_step(
        &self,
        subtask_id: i64,
        step_index: i64,
        step_type: &str,
        description: &str,
    ) -> Result<SubTaskStepDto> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO subtask_steps
             (subtask_id, step_index, step_type, status, description)
             VALUES (?1, ?2, ?3, 'pending', ?4)",
            params![subtask_id, step_index, step_type.trim(), description],
        )
        .context("failed to insert subtask step")?;

        let step_id = conn.last_insert_rowid();
        self.get_subtask_step(step_id)?
            .ok_or_else(|| anyhow!("step id={} missing after insert", step_id))
    }

    pub fn update_subtask_step_status(
        &self,
        step_id: i64,
        status: &str,
        result_summary: Option<&str>,
        result_payload: Option<&str>,
        error_message: Option<&str>,
    ) -> Result<SubTaskStepDto> {
        let existing = self
            .get_subtask_step(step_id)?
            .ok_or_else(|| anyhow!("subtask step id={} not found", step_id))?;

        let clean_status = status.trim();
        let started_at = if clean_status == "running" && existing.started_at.is_none() {
            Some(now_string()?)
        } else {
            existing.started_at.clone()
        };

        let completed_at = if matches!(clean_status, "completed" | "failed" | "skipped") {
            Some(now_string()?)
        } else {
            existing.completed_at.clone()
        };

        let next_summary = result_summary
            .map(|value| value.to_string())
            .or(existing.result_summary.clone());
        let next_payload = result_payload
            .map(|value| value.to_string())
            .or(existing.result_payload.clone());
        let next_error = error_message
            .map(|value| value.to_string())
            .or(existing.error_message.clone());

        let conn = self.connect()?;
        conn.execute(
            "UPDATE subtask_steps
             SET status = ?1,
                 result_summary = ?2,
                 result_payload = ?3,
                 error_message = ?4,
                 started_at = ?5,
                 completed_at = ?6
             WHERE id = ?7",
            params![
                clean_status,
                next_summary,
                next_payload,
                next_error,
                started_at,
                completed_at,
                step_id,
            ],
        )
        .context("failed to update subtask step status")?;

        self.get_subtask_step(step_id)?
            .ok_or_else(|| anyhow!("subtask step disappeared after update"))
    }

    pub fn list_subtask_steps(&self, subtask_id: i64) -> Result<Vec<SubTaskStepDto>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT id,
                        subtask_id,
                        step_index,
                        step_type,
                        status,
                        description,
                        result_summary,
                        result_payload,
                        error_message,
                        started_at,
                        completed_at
                 FROM subtask_steps
                 WHERE subtask_id = ?1
                 ORDER BY step_index ASC, id ASC",
            )
            .context("failed to prepare list_subtask_steps query")?;

        let rows = stmt
            .query_map(params![subtask_id], subtask_step_from_row)
            .context("failed to query subtask steps")?;

        let mut steps = Vec::new();
        for row in rows {
            steps.push(row.context("failed to map subtask step row")?);
        }
        Ok(steps)
    }

    pub fn get_subtask_step(&self, step_id: i64) -> Result<Option<SubTaskStepDto>> {
        let conn = self.connect()?;
        let step = conn
            .query_row(
                "SELECT id,
                        subtask_id,
                        step_index,
                        step_type,
                        status,
                        description,
                        result_summary,
                        result_payload,
                        error_message,
                        started_at,
                        completed_at
                 FROM subtask_steps
                 WHERE id = ?1",
                params![step_id],
                subtask_step_from_row,
            )
            .optional()
            .context("failed to query subtask step by id")?;

        Ok(step)
    }

    pub fn create_file_write_proposal(
        &self,
        subtask_id: i64,
        step_id: Option<i64>,
        task_id: i64,
        proposed_path: &str,
        proposed_content: &str,
    ) -> Result<FileWriteProposalDto> {
        let conn = self.connect()?;
        conn.execute(
            &format!(
                "INSERT INTO subtask_file_write_proposals
                 (subtask_id, step_id, task_id, proposed_path, proposed_content, status, proposed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'pending_approval', ({now}))",
                now = SQLITE_NOW_EXPR,
            ),
            params![
                subtask_id,
                step_id,
                task_id,
                proposed_path.trim(),
                proposed_content,
            ],
        )
        .context("failed to insert file write proposal")?;

        let proposal_id = conn.last_insert_rowid();
        self.get_file_write_proposal_by_id(proposal_id)?
            .ok_or_else(|| {
                anyhow!(
                    "file write proposal id={} missing after insert",
                    proposal_id
                )
            })
    }

    pub fn transition_file_write_proposal_status(
        &self,
        proposal_id: i64,
        expected_status: &str,
        next_status: &str,
        mark_resolved: bool,
    ) -> Result<FileWriteProposalDto> {
        let clean_expected = expected_status.trim();
        let clean_next = next_status.trim();
        if clean_expected.is_empty() || clean_next.is_empty() {
            return Err(anyhow!(
                "expected_status and next_status must be non-empty for proposal transition"
            ));
        }

        let conn = self.connect()?;
        let changed = if mark_resolved {
            conn.execute(
                &format!(
                    "UPDATE subtask_file_write_proposals
                     SET status = ?1,
                         resolved_at = ({now})
                     WHERE id = ?2
                       AND status = ?3",
                    now = SQLITE_NOW_EXPR,
                ),
                params![clean_next, proposal_id, clean_expected],
            )
            .context("failed to transition file write proposal status with resolved_at")?
        } else {
            conn.execute(
                "UPDATE subtask_file_write_proposals
                 SET status = ?1,
                     resolved_at = NULL
                 WHERE id = ?2
                   AND status = ?3",
                params![clean_next, proposal_id, clean_expected],
            )
            .context("failed to transition file write proposal status without resolved_at")?
        };

        if changed == 0 {
            let current_status = self
                .get_file_write_proposal_by_id(proposal_id)?
                .map(|proposal| proposal.status)
                .unwrap_or_else(|| "missing".to_string());
            return Err(anyhow!(
                "proposal id={} transition rejected: expected status '{}' but current status is '{}'",
                proposal_id,
                clean_expected,
                current_status
            ));
        }

        self.get_file_write_proposal_by_id(proposal_id)?
            .ok_or_else(|| anyhow!("proposal disappeared after transition"))
    }

    pub fn begin_file_write_proposal_apply(
        &self,
        proposal_id: i64,
    ) -> Result<FileWriteProposalDto> {
        self.transition_file_write_proposal_status(
            proposal_id,
            "pending_approval",
            "applying",
            false,
        )
    }

    pub fn complete_file_write_proposal_apply(
        &self,
        proposal_id: i64,
    ) -> Result<FileWriteProposalDto> {
        self.transition_file_write_proposal_status(proposal_id, "applying", "approved", true)
    }

    pub fn rollback_file_write_proposal_apply(
        &self,
        proposal_id: i64,
    ) -> Result<FileWriteProposalDto> {
        self.transition_file_write_proposal_status(
            proposal_id,
            "applying",
            "pending_approval",
            false,
        )
    }

    pub fn resolve_file_write_proposal(
        &self,
        proposal_id: i64,
        action: &str,
    ) -> Result<FileWriteProposalDto> {
        let normalized = action.trim().to_ascii_lowercase();
        if normalized != "approved" && normalized != "rejected" {
            return Err(anyhow!(
                "invalid proposal resolution action '{}' (expected approved or rejected)",
                action
            ));
        }

        let conn = self.connect()?;
        let changed = conn
            .execute(
                &format!(
                    "UPDATE subtask_file_write_proposals
                     SET status = ?1,
                         resolved_at = ({now})
                     WHERE id = ?2",
                    now = SQLITE_NOW_EXPR,
                ),
                params![normalized, proposal_id],
            )
            .context("failed to resolve file write proposal")?;

        if changed == 0 {
            return Err(anyhow!("file write proposal id={} not found", proposal_id));
        }

        self.get_file_write_proposal_by_id(proposal_id)?
            .ok_or_else(|| anyhow!("proposal disappeared after resolution"))
    }

    pub fn list_pending_file_write_proposals(
        &self,
        task_id: i64,
    ) -> Result<Vec<FileWriteProposalDto>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT id,
                        subtask_id,
                        step_id,
                        task_id,
                        proposed_path,
                        proposed_content,
                        status,
                        proposed_at,
                        resolved_at
                 FROM subtask_file_write_proposals
                 WHERE task_id = ?1
                   AND status = 'pending_approval'
                 ORDER BY id ASC",
            )
            .context("failed to prepare list_pending_file_write_proposals query")?;

        let rows = stmt
            .query_map(params![task_id], file_write_proposal_from_row)
            .context("failed to query pending file write proposals")?;

        let mut proposals = Vec::new();
        for row in rows {
            proposals.push(row.context("failed to map pending file write proposal row")?);
        }
        Ok(proposals)
    }

    pub fn list_file_write_proposals_for_subtask(
        &self,
        subtask_id: i64,
    ) -> Result<Vec<FileWriteProposalDto>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT id,
                        subtask_id,
                        step_id,
                        task_id,
                        proposed_path,
                        proposed_content,
                        status,
                        proposed_at,
                        resolved_at
                 FROM subtask_file_write_proposals
                 WHERE subtask_id = ?1
                 ORDER BY id ASC",
            )
            .context("failed to prepare list_file_write_proposals_for_subtask query")?;

        let rows = stmt
            .query_map(params![subtask_id], file_write_proposal_from_row)
            .context("failed to query file write proposals for subtask")?;

        let mut proposals = Vec::new();
        for row in rows {
            proposals.push(row.context("failed to map subtask file write proposal row")?);
        }
        Ok(proposals)
    }

    pub fn get_file_write_proposal_by_id(
        &self,
        proposal_id: i64,
    ) -> Result<Option<FileWriteProposalDto>> {
        let conn = self.connect()?;
        let proposal = conn
            .query_row(
                "SELECT id,
                        subtask_id,
                        step_id,
                        task_id,
                        proposed_path,
                        proposed_content,
                        status,
                        proposed_at,
                        resolved_at
                 FROM subtask_file_write_proposals
                 WHERE id = ?1",
                params![proposal_id],
                file_write_proposal_from_row,
            )
            .optional()
            .context("failed to query file write proposal by id")?;

        Ok(proposal)
    }

    pub fn append_write_audit_entry(
        &self,
        task_id: i64,
        subtask_id: i64,
        proposal_id: i64,
        action: &str,
        path: &str,
    ) -> Result<WriteAuditEntryDto> {
        let conn = self.connect()?;
        conn.execute(
            &format!(
                "INSERT INTO subtask_write_audit_log
                 (task_id, subtask_id, proposal_id, action, proposed_path, resolved_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ({now}))",
                now = SQLITE_NOW_EXPR,
            ),
            params![task_id, subtask_id, proposal_id, action.trim(), path],
        )
        .context("failed to append write audit entry")?;

        let audit_id = conn.last_insert_rowid();
        self.get_write_audit_entry_by_id(audit_id)?
            .ok_or_else(|| anyhow!("write audit entry id={} missing after insert", audit_id))
    }

    pub fn list_write_audit_log(
        &self,
        task_id: i64,
        limit: usize,
    ) -> Result<Vec<WriteAuditEntryDto>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT id,
                        task_id,
                        subtask_id,
                        proposal_id,
                        action,
                        proposed_path,
                        resolved_at
                 FROM subtask_write_audit_log
                 WHERE task_id = ?1
                 ORDER BY id DESC
                 LIMIT ?2",
            )
            .context("failed to prepare list_write_audit_log query")?;

        let rows = stmt
            .query_map(params![task_id, limit as i64], write_audit_from_row)
            .context("failed to query write audit log")?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.context("failed to map write audit row")?);
        }
        Ok(entries)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_action_receipt(
        &self,
        task_id: i64,
        class: &str,
        surface: &str,
        level: &str,
        description: &str,
        payload_excerpt: &str,
        status: &str,
        failure_reason: Option<&str>,
        undo_ref: Option<&str>,
    ) -> Result<ActionReceiptDto> {
        let class = class.trim();
        let parsed_class = crate::action_bus::ActionClass::parse(class)
            .ok_or_else(|| anyhow!("unknown action class: {class}"))?;
        if parsed_class.as_str() != class {
            return Err(anyhow!(
                "action class must be canonical (received {class}, canonical {})",
                parsed_class.as_str()
            ));
        }
        crate::trust::assert_runtime_level_allowed(class, level.trim())?;
        if surface.trim().is_empty() || description.trim().is_empty() {
            return Err(anyhow!("action receipt surface and description are required"));
        }
        let conn = self.connect()?;
        let mark_resolved = matches!(
            status,
            "applied" | "rejected" | "reverted" | "failed" | "guided"
        );
        let resolved_expr = if mark_resolved {
            format!("({SQLITE_NOW_EXPR})")
        } else {
            "NULL".to_string()
        };
        conn.execute(
            &format!(
                "INSERT INTO action_receipts
                 (task_id, class, surface, level, description, payload_excerpt, status, failure_reason, undo_ref, created_at, resolved_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ({now}), {resolved})",
                now = SQLITE_NOW_EXPR,
                resolved = resolved_expr,
            ),
            params![
                task_id,
                class,
                surface.trim(),
                level.trim(),
                description.trim(),
                payload_excerpt,
                status.trim(),
                failure_reason,
                undo_ref,
            ],
        )
        .context("failed to insert action receipt")?;

        let receipt_id = conn.last_insert_rowid();
        self.get_action_receipt(receipt_id)?
            .ok_or_else(|| anyhow!("action receipt id={} missing after insert", receipt_id))
    }

    pub fn update_action_receipt_status(
        &self,
        receipt_id: i64,
        status: &str,
        failure_reason: Option<&str>,
        undo_ref: Option<&str>,
    ) -> Result<ActionReceiptDto> {
        let status = status.trim();
        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("failed to start action receipt transition")?;
        let current: Option<String> = tx
            .query_row(
                "SELECT status FROM action_receipts WHERE id = ?1",
                params![receipt_id],
                |row| row.get(0),
            )
            .optional()
            .context("failed to read action receipt transition state")?;
        let current = current.ok_or_else(|| anyhow!("action receipt id={} not found", receipt_id))?;
        if !valid_action_status_transition(&current, status) {
            return Err(anyhow!(
                "invalid action receipt transition for id={receipt_id}: {current} -> {status}"
            ));
        }
        let resolved_at = if is_terminal_action_status(status) {
            format!("COALESCE(resolved_at, ({SQLITE_NOW_EXPR}))")
        } else {
            "NULL".to_string()
        };
        tx.execute(
            &format!(
                "UPDATE action_receipts
                 SET status = ?1,
                     failure_reason = ?2,
                     undo_ref = COALESCE(?3, undo_ref),
                     resolved_at = {resolved_at}
                 WHERE id = ?4 AND status = ?5"
            ),
            params![status, failure_reason, undo_ref, receipt_id, current],
        )
        .context("failed to update action receipt")?;
        tx.commit()
            .context("failed to commit action receipt transition")?;
        self.get_action_receipt(receipt_id)?
            .ok_or_else(|| anyhow!("action receipt disappeared after update"))
    }

    pub fn get_action_receipt(&self, receipt_id: i64) -> Result<Option<ActionReceiptDto>> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT id, task_id, class, surface, level, description, payload_excerpt,
                    status, failure_reason, undo_ref, created_at, resolved_at
             FROM action_receipts
             WHERE id = ?1",
            params![receipt_id],
            action_receipt_from_row,
        )
        .optional()
        .context("failed to query action receipt")
    }

    pub fn list_action_receipts(
        &self,
        task_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<ActionReceiptDto>> {
        let conn = self.connect()?;
        let max = limit.min(500) as i64;
        let mut receipts = Vec::new();
        if let Some(task_id) = task_id {
            let mut stmt = conn
                .prepare(
                    "SELECT id, task_id, class, surface, level, description, payload_excerpt,
                            status, failure_reason, undo_ref, created_at, resolved_at
                     FROM action_receipts
                     WHERE task_id = ?1
                     ORDER BY id DESC
                     LIMIT ?2",
                )
                .context("failed to prepare task action receipt query")?;
            let rows = stmt
                .query_map(params![task_id, max], action_receipt_from_row)
                .context("failed to query task action receipts")?;
            for row in rows {
                receipts.push(row.context("failed to map action receipt row")?);
            }
        } else {
            let mut stmt = conn
                .prepare(
                    "SELECT id, task_id, class, surface, level, description, payload_excerpt,
                            status, failure_reason, undo_ref, created_at, resolved_at
                     FROM action_receipts
                     ORDER BY id DESC
                     LIMIT ?1",
                )
                .context("failed to prepare action receipt query")?;
            let rows = stmt
                .query_map(params![max], action_receipt_from_row)
                .context("failed to query action receipts")?;
            for row in rows {
                receipts.push(row.context("failed to map action receipt row")?);
            }
        }
        Ok(receipts)
    }

    // ---------------------------------------------------------------------
    // flow/session mode/suggestions
    // ---------------------------------------------------------------------

    pub fn upsert_session_mode_state(
        &self,
        input: &SessionModeUpdateInput,
    ) -> Result<SessionModeStateDto> {
        let conn = self.connect()?;
        conn.execute(
            &format!(
                "INSERT INTO session_mode_state
                 (task_id, current_mode, mode_reason, waiting_on_user_decision, last_engine_decision, active_artifact_id, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ({now}))
                 ON CONFLICT(task_id) DO UPDATE SET
                    current_mode = excluded.current_mode,
                    mode_reason = excluded.mode_reason,
                    waiting_on_user_decision = excluded.waiting_on_user_decision,
                    last_engine_decision = excluded.last_engine_decision,
                    active_artifact_id = excluded.active_artifact_id,
                    updated_at = ({now})",
                now = SQLITE_NOW_EXPR,
            ),
            params![
                input.task_id,
                input.current_mode.trim(),
                input.mode_reason,
                if input.waiting_on_user_decision { 1 } else { 0 },
                input.last_engine_decision,
                input.active_artifact_id,
            ],
        )
        .context("failed to upsert session_mode_state")?;

        self.get_session_mode_state(input.task_id)?
            .ok_or_else(|| anyhow!("session mode state missing after upsert"))
    }

    pub fn get_session_mode_state(&self, task_id: i64) -> Result<Option<SessionModeStateDto>> {
        let conn = self.connect()?;
        let state = conn
            .query_row(
                "SELECT task_id,
                        current_mode,
                        mode_reason,
                        waiting_on_user_decision,
                        last_engine_decision,
                        active_artifact_id,
                        updated_at
                 FROM session_mode_state
                 WHERE task_id = ?1",
                params![task_id],
                session_mode_from_row,
            )
            .optional()
            .context("failed to query session mode state")?;

        Ok(state)
    }

    pub fn create_suggestion(&self, input: &NewSuggestionInput) -> Result<SuggestionDto> {
        let conn = self.connect()?;
        conn.execute(
            &format!(
                "INSERT INTO suggestions
                 (
                    task_id,
                    title,
                    description,
                    suggestion_type,
                    source_reason,
                    status,
                    suggestion_key,
                    linked_context,
                    linked_subtask_type,
                    linked_revision_intent,
                    created_at,
                    updated_at
                 )
                 VALUES
                 (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7, ?8, ?9, ({now}), ({now}))",
                now = SQLITE_NOW_EXPR,
            ),
            params![
                input.task_id,
                input.title.trim(),
                input.description,
                input.suggestion_type.trim(),
                input.source_reason,
                input.suggestion_key.trim(),
                input.linked_context,
                input.linked_subtask_type,
                input.linked_revision_intent,
            ],
        )
        .context("failed to insert suggestion")?;

        let suggestion_id = conn.last_insert_rowid();
        self.get_suggestion_by_id(suggestion_id)?
            .ok_or_else(|| anyhow!("suggestion id={} missing after insert", suggestion_id))
    }

    pub fn list_suggestions(
        &self,
        task_id: i64,
        include_resolved: bool,
    ) -> Result<Vec<SuggestionDto>> {
        let conn = self.connect()?;
        let sql = if include_resolved {
            "SELECT suggestion_id,
                    task_id,
                    title,
                    description,
                    suggestion_type,
                    source_reason,
                    status,
                    suggestion_key,
                    linked_context,
                    linked_subtask_type,
                    linked_revision_intent,
                    created_at,
                    updated_at
             FROM suggestions
             WHERE task_id = ?1
             ORDER BY suggestion_id DESC"
        } else {
            "SELECT suggestion_id,
                    task_id,
                    title,
                    description,
                    suggestion_type,
                    source_reason,
                    status,
                    suggestion_key,
                    linked_context,
                    linked_subtask_type,
                    linked_revision_intent,
                    created_at,
                    updated_at
             FROM suggestions
             WHERE task_id = ?1
               AND status = 'pending'
             ORDER BY suggestion_id DESC"
        };

        let mut stmt = conn
            .prepare(sql)
            .context("failed to prepare list_suggestions query")?;
        let rows = stmt
            .query_map(params![task_id], suggestion_from_row)
            .context("failed to query suggestions")?;

        let mut suggestions = Vec::new();
        for row in rows {
            suggestions.push(row.context("failed to map suggestion row")?);
        }
        Ok(suggestions)
    }

    pub fn get_suggestion_by_id(&self, suggestion_id: i64) -> Result<Option<SuggestionDto>> {
        let conn = self.connect()?;
        let suggestion = conn
            .query_row(
                "SELECT suggestion_id,
                        task_id,
                        title,
                        description,
                        suggestion_type,
                        source_reason,
                        status,
                        suggestion_key,
                        linked_context,
                        linked_subtask_type,
                        linked_revision_intent,
                        created_at,
                        updated_at
                 FROM suggestions
                 WHERE suggestion_id = ?1",
                params![suggestion_id],
                suggestion_from_row,
            )
            .optional()
            .context("failed to query suggestion by id")?;

        Ok(suggestion)
    }

    pub fn set_suggestion_status(&self, suggestion_id: i64, status: &str) -> Result<SuggestionDto> {
        let conn = self.connect()?;
        let changed = conn
            .execute(
                &format!(
                    "UPDATE suggestions
                     SET status = ?1,
                         updated_at = ({now})
                     WHERE suggestion_id = ?2",
                    now = SQLITE_NOW_EXPR,
                ),
                params![status.trim(), suggestion_id],
            )
            .context("failed to set suggestion status")?;

        if changed == 0 {
            return Err(anyhow!("suggestion id={} not found", suggestion_id));
        }

        self.get_suggestion_by_id(suggestion_id)?
            .ok_or_else(|| anyhow!("suggestion disappeared after status update"))
    }

    pub fn has_recent_suggestion_key(
        &self,
        task_id: i64,
        suggestion_key: &str,
        cooldown_seconds: i64,
    ) -> Result<bool> {
        let conn = self.connect()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*)
                 FROM suggestions
                 WHERE task_id = ?1
                   AND suggestion_key = ?2
                   AND (strftime('%s','now') - strftime('%s', created_at)) <= ?3",
                params![task_id, suggestion_key.trim(), cooldown_seconds.max(0)],
                |row| row.get(0),
            )
            .context("failed to query suggestion key cooldown")?;
        Ok(count > 0)
    }

    pub fn was_suggestion_key_dismissed_recently(
        &self,
        task_id: i64,
        suggestion_key: &str,
        suppression_seconds: i64,
    ) -> Result<bool> {
        let conn = self.connect()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*)
                 FROM suggestions
                 WHERE task_id = ?1
                   AND suggestion_key = ?2
                   AND status = 'dismissed'
                   AND (strftime('%s','now') - strftime('%s', updated_at)) <= ?3",
                params![task_id, suggestion_key.trim(), suppression_seconds.max(0)],
                |row| row.get(0),
            )
            .context("failed to query dismissed suggestion suppression window")?;
        Ok(count > 0)
    }

    pub fn count_recent_revision_activity(&self, task_id: i64, window_seconds: i64) -> Result<i64> {
        let conn = self.connect()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*)
                 FROM artifact_revisions
                 WHERE task_id = ?1
                   AND (strftime('%s','now') - strftime('%s', updated_at)) <= ?2",
                params![task_id, window_seconds.max(0)],
                |row| row.get(0),
            )
            .context("failed to count recent revision activity")?;
        Ok(count)
    }

    // ---------------------------------------------------------------------
    // app settings
    // ---------------------------------------------------------------------

    pub fn get_app_setting(&self, key: &str) -> Result<Option<String>> {
        let conn = self.connect()?;
        let value = conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = ?1",
                params![key.trim()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("failed to query app setting")?;
        Ok(value)
    }

    pub fn set_app_setting(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            &format!(
                "INSERT INTO app_settings (key, value, updated_at)
                 VALUES (?1, ?2, ({now}))
                 ON CONFLICT(key) DO UPDATE SET
                    value = excluded.value,
                    updated_at = ({now})",
                now = SQLITE_NOW_EXPR,
            ),
            params![key.trim(), value],
        )
        .context("failed to upsert app setting")?;
        Ok(())
    }

    pub fn delete_app_setting(&self, key: &str) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "DELETE FROM app_settings WHERE key = ?1",
            params![key.trim()],
        )
        .context("failed to delete app setting")?;
        Ok(())
    }

    pub fn get_app_setting_bool(&self, key: &str) -> Result<Option<bool>> {
        let raw = self.get_app_setting(key)?;
        Ok(raw.map(|value| {
            let lowered = value.trim().to_ascii_lowercase();
            matches!(lowered.as_str(), "1" | "true" | "yes" | "on")
        }))
    }

    pub fn append_llm_usage_log(&self, input: &LlmUsageLogInput) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            &format!(
                "INSERT INTO llm_usage_log
                 (tier, model, purpose, input_tokens, output_tokens, cached_tokens, est_cost_usd, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ({now}))",
                now = SQLITE_NOW_EXPR,
            ),
            params![
                input.tier.trim(),
                input.model.trim(),
                input.purpose.trim(),
                input.input_tokens as i64,
                input.output_tokens as i64,
                input.cached_tokens as i64,
                input.est_cost_usd,
            ],
        )
        .context("failed to append llm usage log")?;
        Ok(())
    }

    pub fn sum_llm_usage_today(&self, tier: Option<&str>) -> Result<f64> {
        let conn = self.connect()?;
        let sql = match tier {
            Some(_) => {
                "SELECT COALESCE(SUM(est_cost_usd), 0.0)
                 FROM llm_usage_log
                 WHERE tier = ?1 AND date(created_at) = date('now')"
            }
            None => {
                "SELECT COALESCE(SUM(est_cost_usd), 0.0)
                 FROM llm_usage_log
                 WHERE date(created_at) = date('now')"
            }
        };
        let total = match tier {
            Some(tier) => conn.query_row(sql, params![tier], |row| row.get::<_, f64>(0)),
            None => conn.query_row(sql, [], |row| row.get::<_, f64>(0)),
        }
        .context("failed to sum today's llm usage")?;
        Ok(total)
    }

    pub fn sum_llm_usage_today_by_tier(&self) -> Result<Vec<LlmSpendByTier>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT tier, COALESCE(SUM(est_cost_usd), 0.0)
                 FROM llm_usage_log
                 WHERE date(created_at) = date('now')
                 GROUP BY tier
                 ORDER BY tier ASC",
            )
            .context("failed to prepare llm usage by tier query")?;
        let rows = stmt
            .query_map([], |row| {
                Ok(LlmSpendByTier {
                    tier: row.get(0)?,
                    est_cost_usd: row.get(1)?,
                })
            })
            .context("failed to query llm usage by tier")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to decode llm usage by tier")
    }

    pub fn llm_usage_history(&self, days: usize) -> Result<Vec<LlmSpendHistoryRow>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT date(created_at) AS usage_date, COALESCE(SUM(est_cost_usd), 0.0)
                 FROM llm_usage_log
                 WHERE date(created_at) >= date('now', ?1)
                 GROUP BY usage_date
                 ORDER BY usage_date ASC",
            )
            .context("failed to prepare llm usage history query")?;
        let since = format!("-{} days", days.saturating_sub(1));
        let rows = stmt
            .query_map(params![since], |row| {
                Ok(LlmSpendHistoryRow {
                    date: row.get(0)?,
                    est_cost_usd: row.get(1)?,
                })
            })
            .context("failed to query llm usage history")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to decode llm usage history")
    }

    pub fn get_onboarding_complete(&self) -> Result<bool> {
        Ok(self
            .get_app_setting_bool(APP_SETTING_ONBOARDING_COMPLETE)?
            .unwrap_or(false))
    }

    pub fn set_onboarding_complete(&self, complete: bool) -> Result<()> {
        self.set_app_setting(
            APP_SETTING_ONBOARDING_COMPLETE,
            if complete { "1" } else { "0" },
        )?;

        if complete {
            self.set_app_setting(APP_SETTING_ONBOARDING_LAST_COMPLETED_AT, "1")?;
        }

        Ok(())
    }

    pub fn get_preferred_workspace_folder(&self) -> Result<Option<String>> {
        Ok(self
            .get_app_setting(APP_SETTING_PREFERRED_WORKSPACE_FOLDER)?
            .map(|path| path.trim().to_string())
            .filter(|path| !path.is_empty()))
    }

    pub fn set_preferred_workspace_folder(&self, folder: Option<&str>) -> Result<()> {
        let Some(folder) = folder.map(str::trim).filter(|value| !value.is_empty()) else {
            self.delete_app_setting(APP_SETTING_PREFERRED_WORKSPACE_FOLDER)?;
            return Ok(());
        };

        self.set_app_setting(APP_SETTING_PREFERRED_WORKSPACE_FOLDER, folder)
    }

    pub fn get_workspace_prompt_dismissed(&self) -> Result<bool> {
        Ok(self
            .get_app_setting_bool(APP_SETTING_WORKSPACE_PROMPT_DISMISSED)?
            .unwrap_or(false))
    }

    pub fn set_workspace_prompt_dismissed(&self, dismissed: bool) -> Result<()> {
        self.set_app_setting(
            APP_SETTING_WORKSPACE_PROMPT_DISMISSED,
            if dismissed { "1" } else { "0" },
        )
    }

    // phase 19: session persistence -------------------------------------------

    pub fn get_launch_at_login(&self) -> Result<bool> {
        Ok(self
            .get_app_setting_bool(APP_SETTING_LAUNCH_AT_LOGIN)?
            .unwrap_or(false))
    }

    pub fn set_launch_at_login(&self, enabled: bool) -> Result<()> {
        self.set_app_setting(
            APP_SETTING_LAUNCH_AT_LOGIN,
            if enabled { "true" } else { "false" },
        )
    }

    pub fn get_quiet_mode(&self) -> Result<bool> {
        Ok(self
            .get_app_setting_bool(APP_SETTING_QUIET_MODE)?
            .unwrap_or(false))
    }

    pub fn set_quiet_mode(&self, quiet: bool) -> Result<()> {
        self.set_app_setting(APP_SETTING_QUIET_MODE, if quiet { "true" } else { "false" })
    }

    // stores overlay mode as the string "expanded" or "collapsed".
    pub fn get_overlay_expanded(&self) -> Result<bool> {
        Ok(self
            .get_app_setting(APP_SETTING_OVERLAY_MODE)?
            .map(|v| v.trim() == "expanded")
            .unwrap_or(false))
    }

    pub fn set_overlay_expanded(&self, expanded: bool) -> Result<()> {
        self.set_app_setting(
            APP_SETTING_OVERLAY_MODE,
            if expanded { "expanded" } else { "collapsed" },
        )
    }

    // phase 21: privacy center settings --------------------------------------

    pub fn get_privacy_workspace_watcher_enabled(&self) -> Result<bool> {
        Ok(self
            .get_app_setting_bool(APP_SETTING_PRIVACY_WORKSPACE_WATCHER_ENABLED)?
            .unwrap_or(true))
    }

    pub fn set_privacy_workspace_watcher_enabled(&self, enabled: bool) -> Result<()> {
        self.set_app_setting(
            APP_SETTING_PRIVACY_WORKSPACE_WATCHER_ENABLED,
            if enabled { "true" } else { "false" },
        )
    }

    pub fn get_privacy_clipboard_capture_enabled(&self) -> Result<bool> {
        Ok(self
            .get_app_setting_bool(APP_SETTING_PRIVACY_CLIPBOARD_CAPTURE_ENABLED)?
            .unwrap_or(true))
    }

    pub fn set_privacy_clipboard_capture_enabled(&self, enabled: bool) -> Result<()> {
        self.set_app_setting(
            APP_SETTING_PRIVACY_CLIPBOARD_CAPTURE_ENABLED,
            if enabled { "true" } else { "false" },
        )
    }

    pub fn get_privacy_active_window_context_enabled(&self) -> Result<bool> {
        Ok(self
            .get_app_setting_bool(APP_SETTING_PRIVACY_ACTIVE_WINDOW_CONTEXT_ENABLED)?
            .unwrap_or(true))
    }

    pub fn set_privacy_active_window_context_enabled(&self, enabled: bool) -> Result<()> {
        self.set_app_setting(
            APP_SETTING_PRIVACY_ACTIVE_WINDOW_CONTEXT_ENABLED,
            if enabled { "true" } else { "false" },
        )
    }

    pub fn get_privacy_proactive_triggers_enabled(&self) -> Result<bool> {
        Ok(self
            .get_app_setting_bool(APP_SETTING_PRIVACY_PROACTIVE_TRIGGERS_ENABLED)?
            .unwrap_or_else(|| !self.get_quiet_mode().unwrap_or(false)))
    }

    pub fn set_privacy_proactive_triggers_enabled(&self, enabled: bool) -> Result<()> {
        self.set_app_setting(
            APP_SETTING_PRIVACY_PROACTIVE_TRIGGERS_ENABLED,
            if enabled { "true" } else { "false" },
        )
    }

    pub fn get_privacy_user_profile_memory_enabled(&self) -> Result<bool> {
        Ok(self
            .get_app_setting_bool(APP_SETTING_PRIVACY_USER_PROFILE_MEMORY_ENABLED)?
            .unwrap_or(false))
    }

    pub fn set_privacy_user_profile_memory_enabled(&self, enabled: bool) -> Result<()> {
        self.set_app_setting(
            APP_SETTING_PRIVACY_USER_PROFILE_MEMORY_ENABLED,
            if enabled { "true" } else { "false" },
        )
    }

    pub fn get_privacy_calendar_context_enabled(&self) -> Result<bool> {
        Ok(self
            .get_app_setting_bool(APP_SETTING_PRIVACY_CALENDAR_CONTEXT_ENABLED)?
            .unwrap_or(false))
    }

    pub fn set_privacy_calendar_context_enabled(&self, enabled: bool) -> Result<()> {
        self.set_app_setting(
            APP_SETTING_PRIVACY_CALENDAR_CONTEXT_ENABLED,
            if enabled { "true" } else { "false" },
        )
    }

    pub fn get_privacy_selection_capture_enabled(&self) -> Result<bool> {
        Ok(self
            .get_app_setting_bool(APP_SETTING_PRIVACY_SELECTION_CAPTURE_ENABLED)?
            .unwrap_or(true))
    }

    pub fn set_privacy_selection_capture_enabled(&self, enabled: bool) -> Result<()> {
        self.set_app_setting(
            APP_SETTING_PRIVACY_SELECTION_CAPTURE_ENABLED,
            if enabled { "true" } else { "false" },
        )
    }

    pub fn get_privacy_typing_activity_enabled(&self) -> Result<bool> {
        Ok(self
            .get_app_setting_bool(APP_SETTING_PRIVACY_TYPING_ACTIVITY_ENABLED)?
            .unwrap_or(true))
    }

    pub fn set_privacy_typing_activity_enabled(&self, enabled: bool) -> Result<()> {
        self.set_app_setting(
            APP_SETTING_PRIVACY_TYPING_ACTIVITY_ENABLED,
            if enabled { "true" } else { "false" },
        )
    }

    // per-task content observation toggle (off by default).
    // key format: privacy_content_observation_task_[task_id]
    pub fn get_content_observation_enabled(&self, task_id: i64) -> Result<bool> {
        let key = format!("privacy_content_observation_task_{task_id}");
        Ok(self.get_app_setting_bool(&key)?.unwrap_or(false))
    }

    pub fn set_content_observation_enabled(&self, task_id: i64, enabled: bool) -> Result<()> {
        let key = format!("privacy_content_observation_task_{task_id}");
        self.set_app_setting(&key, if enabled { "true" } else { "false" })
    }

    pub fn get_tts_voice(&self) -> Result<String> {
        let configured = self
            .get_app_setting(APP_SETTING_TTS_VOICE)?
            .unwrap_or_else(|| crate::voice_naturalness::DEFAULT_TTS_VOICE.to_string());
        Ok(crate::voice_naturalness::normalize_tts_voice(&configured))
    }

    pub fn set_tts_voice(&self, voice: &str) -> Result<String> {
        let normalized = crate::voice_naturalness::normalize_tts_voice(voice);
        self.set_app_setting(APP_SETTING_TTS_VOICE, &normalized)?;
        Ok(normalized)
    }

    // last reorientation summary — stored when a notification fires so the
    // frontend can retrieve it on notification click even after cooldown resets.
    pub fn get_last_reorientation_summary(&self, task_id: i64) -> Result<Option<String>> {
        let key = format!("{}{}", LAST_REORIENTATION_SUMMARY_PREFIX, task_id);
        self.get_app_setting(&key)
    }

    pub fn set_last_reorientation_summary(&self, task_id: i64, summary: &str) -> Result<()> {
        let key = format!("{}{}", LAST_REORIENTATION_SUMMARY_PREFIX, task_id);
        self.set_app_setting(&key, summary)
    }

    // returns true if this is NOT the first ever session (session_restored_at is set).
    pub fn get_session_restored_at(&self) -> Result<bool> {
        Ok(self
            .get_app_setting(APP_SETTING_SESSION_RESTORED_AT)?
            .is_some())
    }

    pub fn mark_session_restored(&self) -> Result<()> {
        self.set_app_setting(APP_SETTING_SESSION_RESTORED_AT, "1")
    }

    // ---------------------------------------------------------------------
    // event log
    // ---------------------------------------------------------------------

    pub fn list_recent_events(&self, task_id: i64, limit: usize) -> Result<Vec<EventLogEntryDto>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, task_id, event_type, payload_json, created_at
                 FROM event_log
                 WHERE task_id = ?1
                 ORDER BY id DESC
                 LIMIT ?2",
            )
            .context("failed to prepare list_recent_events query")?;

        let rows = stmt
            .query_map(params![task_id, limit as i64], event_log_from_row)
            .context("failed to query event log entries")?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.context("failed to map event log row")?);
        }
        Ok(entries)
    }

    pub fn record_event(&self, task_id: i64, event_type: &str, payload_json: &str) -> Result<()> {
        let mut conn = self.connect()?;
        let tx = conn
            .transaction()
            .context("failed to start record_event tx")?;
        Self::record_event_tx(&tx, task_id, event_type, payload_json)?;
        tx.commit().context("failed to commit record_event tx")?;
        Ok(())
    }

    // ---------------------------------------------------------------------
    // phase 13 workspace awareness
    // ---------------------------------------------------------------------

    pub fn set_watched_folder(&self, task_id: i64, folder_path: &str) -> Result<WatchedFolderDto> {
        let conn = self.connect()?;
        conn.execute(
            &format!(
                "INSERT INTO watched_folders
                 (task_id, folder_path, is_active, ignore_rules_json, created_at, updated_at)
                 VALUES (?1, ?2, 1, '[]', ({now}), ({now}))
                 ON CONFLICT(task_id) DO UPDATE SET
                    folder_path = excluded.folder_path,
                    is_active = 1,
                    updated_at = ({now})",
                now = SQLITE_NOW_EXPR,
            ),
            params![task_id, folder_path.trim()],
        )
        .context("failed to set watched folder")?;

        self.get_watched_folder(task_id)?
            .ok_or_else(|| anyhow!("watched folder missing after upsert"))
    }

    pub fn clear_watched_folder(&self, task_id: i64) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            &format!(
                "UPDATE watched_folders
                 SET is_active = 0,
                     updated_at = ({now})
                 WHERE task_id = ?1",
                now = SQLITE_NOW_EXPR,
            ),
            params![task_id],
        )
        .context("failed to clear watched folder")?;
        Ok(())
    }

    pub fn get_watched_folder(&self, task_id: i64) -> Result<Option<WatchedFolderDto>> {
        let conn = self.connect()?;
        let folder = conn
            .query_row(
                "SELECT task_id, folder_path, is_active, ignore_rules_json, created_at, updated_at
                 FROM watched_folders
                 WHERE task_id = ?1 AND is_active = 1",
                params![task_id],
                watched_folder_from_row,
            )
            .optional()
            .context("failed to query watched folder")?;

        Ok(folder)
    }

    pub fn upsert_file_registry_entry(
        &self,
        task_id: i64,
        canonical_path: &str,
        artifact_id: Option<i64>,
        version_tag: &str,
    ) -> Result<WatchedFileRegistryEntry> {
        let conn = self.connect()?;
        conn.execute(
            &format!(
                "INSERT INTO watched_file_registry
                 (task_id, canonical_path, artifact_id, last_modified_at, ingested_at)
                 VALUES (?1, ?2, ?3, ?4, ({now}))
                 ON CONFLICT(task_id, canonical_path) DO UPDATE SET
                    artifact_id = excluded.artifact_id,
                    last_modified_at = excluded.last_modified_at,
                    ingested_at = ({now})",
                now = SQLITE_NOW_EXPR,
            ),
            params![task_id, canonical_path.trim(), artifact_id, version_tag],
        )
        .context("failed to upsert watched file registry entry")?;

        self.get_file_registry_entry(task_id, canonical_path)?
            .ok_or_else(|| anyhow!("watched file registry entry missing after upsert"))
    }

    pub fn get_file_registry_entry(
        &self,
        task_id: i64,
        canonical_path: &str,
    ) -> Result<Option<WatchedFileRegistryEntry>> {
        let conn = self.connect()?;
        let entry = conn
            .query_row(
                "SELECT id, task_id, canonical_path, artifact_id, last_modified_at, ingested_at
                 FROM watched_file_registry
                 WHERE task_id = ?1 AND canonical_path = ?2",
                params![task_id, canonical_path.trim()],
                watched_file_registry_from_row,
            )
            .optional()
            .context("failed to query watched file registry entry")?;

        Ok(entry)
    }

    pub fn remove_file_registry_entry(&self, task_id: i64, canonical_path: &str) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "DELETE FROM watched_file_registry WHERE task_id = ?1 AND canonical_path = ?2",
            params![task_id, canonical_path.trim()],
        )
        .context("failed to remove watched file registry entry")?;
        Ok(())
    }

    pub fn count_watched_files(&self, task_id: i64) -> Result<i64> {
        let conn = self.connect()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM watched_file_registry WHERE task_id = ?1",
                params![task_id],
                |row| row.get(0),
            )
            .context("failed to count watched files")?;
        Ok(count)
    }

    pub fn set_clipboard_capture(&self, task_id: i64, enabled: bool) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            &format!(
                "INSERT INTO clipboard_capture_settings (task_id, enabled, updated_at)
                 VALUES (?1, ?2, ({now}))
                 ON CONFLICT(task_id) DO UPDATE SET
                    enabled = excluded.enabled,
                    updated_at = ({now})",
                now = SQLITE_NOW_EXPR,
            ),
            params![task_id, if enabled { 1 } else { 0 }],
        )
        .context("failed to upsert clipboard capture setting")?;
        Ok(())
    }

    pub fn get_clipboard_capture(&self, task_id: i64) -> Result<bool> {
        let conn = self.connect()?;
        let enabled = conn
            .query_row(
                "SELECT enabled FROM clipboard_capture_settings WHERE task_id = ?1",
                params![task_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .context("failed to query clipboard capture setting")?;

        Ok(enabled.unwrap_or(0) != 0)
    }

    pub fn append_recently_learned(
        &self,
        task_id: i64,
        source: &str,
        label: &str,
        preview: &str,
    ) -> Result<RecentlyLearnedItemDto> {
        let conn = self.connect()?;
        conn.execute(
            &format!(
                "INSERT INTO recently_learned_log
                 (task_id, source, display_label, preview_text, ingested_at)
                 VALUES (?1, ?2, ?3, ?4, ({now}))",
                now = SQLITE_NOW_EXPR,
            ),
            params![task_id, source.trim(), label.trim(), preview],
        )
        .context("failed to append recently learned item")?;

        let id = conn.last_insert_rowid();
        self.get_recently_learned_by_id(id)?
            .ok_or_else(|| anyhow!("recently learned id={} missing after insert", id))
    }

    pub fn list_recently_learned(
        &self,
        task_id: i64,
        limit: usize,
    ) -> Result<Vec<RecentlyLearnedItemDto>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, task_id, source, display_label, preview_text, ingested_at
                 FROM recently_learned_log
                 WHERE task_id = ?1
                 ORDER BY id DESC
                 LIMIT ?2",
            )
            .context("failed to prepare list_recently_learned query")?;

        let rows = stmt
            .query_map(params![task_id, limit as i64], recently_learned_from_row)
            .context("failed to query recently learned items")?;

        let mut items = Vec::new();
        for row in rows {
            items.push(row.context("failed to map recently learned row")?);
        }
        Ok(items)
    }

    // ---------------------------------------------------------------------
    // phase 15 proactive initiation
    // ---------------------------------------------------------------------

    pub fn record_task_focus(&self, task_id: i64) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            &format!(
                "INSERT INTO task_focus_log (task_id, focused_at)
                 VALUES (?1, ({now}))",
                now = SQLITE_NOW_EXPR,
            ),
            params![task_id],
        )
        .context("failed to record task focus")?;
        Ok(())
    }

    pub fn get_last_task_focus(&self, task_id: i64) -> Result<Option<String>> {
        let conn = self.connect()?;
        let value = conn
            .query_row(
                "SELECT focused_at
                 FROM task_focus_log
                 WHERE task_id = ?1
                 ORDER BY id DESC
                 LIMIT 1",
                params![task_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("failed to query last task focus")?;
        Ok(value)
    }

    // phase 23: workload helpers

    /// count pending items for a task: pending file write proposals + running subtasks.
    pub fn count_pending_items_for_task(&self, task_id: i64) -> Result<i64> {
        let conn = self.connect()?;
        let proposals: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM subtask_file_write_proposals
                 WHERE task_id = ?1 AND status = 'pending_approval'",
                params![task_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let running: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM subtasks
                 WHERE task_id = ?1 AND status IN ('pending', 'running')",
                params![task_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(proposals + running)
    }

    /// returns true if any speculative subtask (instruction_source='system')
    /// has result_review_status = 'unreviewed' for the given task.
    pub fn task_has_unreviewed_speculative_results(&self, task_id: i64) -> Result<bool> {
        let conn = self.connect()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM subtasks
                 WHERE task_id = ?1
                   AND instruction_source = 'system'
                   AND result_review_status = 'unreviewed'
                   AND status = 'completed'",
                params![task_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(count > 0)
    }

    // phase 23: returns (task_title, instruction_text) pairs for completed subtasks
    // from tasks other than the given task_id, within the last `days` days.
    // used for cross-task collision detection.
    pub fn get_recent_cross_task_subtasks(
        &self,
        exclude_task_id: i64,
        days: i64,
    ) -> Result<Vec<(String, String)>> {
        let conn = self.connect()?;
        let cutoff = format!("-{days} days");
        let mut stmt = conn.prepare(
            "SELECT t.title, s.title || char(10) || s.description
             FROM subtasks s
             JOIN tasks t ON t.id = s.task_id
             WHERE s.task_id != ?1
               AND s.status = 'completed'
               AND s.created_at > datetime('now', ?2)
             ORDER BY s.created_at DESC
             LIMIT 50",
        )?;
        let rows = stmt
            .query_map(params![exclude_task_id, cutoff], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    pub fn record_proactive_trigger(
        &self,
        task_id: i64,
        trigger_type: &str,
        suppressed: bool,
    ) -> Result<i64> {
        let conn = self.connect()?;
        conn.execute(
            &format!(
                "INSERT INTO proactive_trigger_log
                 (task_id, trigger_type, fired_at, suppressed)
                 VALUES (?1, ?2, ({now}), ?3)",
                now = SQLITE_NOW_EXPR,
            ),
            params![task_id, trigger_type.trim(), if suppressed { 1 } else { 0 }],
        )
        .context("failed to record proactive trigger")?;
        Ok(conn.last_insert_rowid())
    }

    pub fn get_last_proactive_trigger(
        &self,
        task_id: i64,
        trigger_type: &str,
    ) -> Result<Option<String>> {
        let conn = self.connect()?;
        // cooldown should only consider real fired triggers, not suppressed attempts.
        let value = conn
            .query_row(
                "SELECT fired_at
                 FROM proactive_trigger_log
                 WHERE task_id = ?1
                   AND trigger_type = ?2
                   AND suppressed = 0
                 ORDER BY id DESC
                 LIMIT 1",
                params![task_id, trigger_type.trim()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("failed to query last proactive trigger")?;
        Ok(value)
    }

    pub fn list_proactive_trigger_audit_log(
        &self,
        task_id: i64,
        limit: usize,
    ) -> Result<Vec<ProactiveAuditEntryDto>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, task_id, trigger_type, fired_at, suppressed
                 FROM proactive_trigger_log
                 WHERE task_id = ?1
                 ORDER BY id DESC
                 LIMIT ?2",
            )
            .context("failed to prepare proactive trigger audit query")?;

        let rows = stmt
            .query_map(params![task_id, limit as i64], proactive_audit_from_row)
            .context("failed to query proactive trigger audit log")?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.context("failed to map proactive audit row")?);
        }
        Ok(entries)
    }

    pub fn log_synthesis_decision(
        &self,
        task_id: Option<i64>,
        reason_type: &str,
        reason_detail: Option<&str>,
        snapshot_confidence: f32,
        snapshot_attention_state: &str,
        message: Option<&str>,
        delivered: bool,
    ) -> Result<i64> {
        let conn = self.connect()?;
        conn.execute(
            &format!(
                "INSERT INTO synthesis_log
                 (task_id, reason_type, reason_detail, snapshot_confidence,
                  snapshot_attention_state, message, delivered, delivered_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7,
                         CASE WHEN ?7 = 1 THEN ({now}) ELSE NULL END,
                         ({now}))",
                now = SQLITE_NOW_EXPR,
            ),
            params![
                task_id,
                reason_type.trim(),
                reason_detail,
                snapshot_confidence,
                snapshot_attention_state.trim(),
                message,
                if delivered { 1 } else { 0 },
            ],
        )
        .context("failed to log synthesis decision")?;
        Ok(conn.last_insert_rowid())
    }

    // apex c1: log a decision that carries the stage 2 verdict, channel, and
    // reason. speak/hold/drop are all recorded; only "speak" that reaches the
    // user sets delivered = 1.
    #[allow(clippy::too_many_arguments)]
    pub fn log_synthesis_decision_staged(
        &self,
        task_id: Option<i64>,
        reason_type: &str,
        reason_detail: Option<&str>,
        snapshot_confidence: f32,
        snapshot_attention_state: &str,
        message: Option<&str>,
        delivered: bool,
        stage2_decision: Option<&str>,
        stage2_channel: Option<&str>,
        stage2_reason: Option<&str>,
    ) -> Result<i64> {
        let conn = self.connect()?;
        conn.execute(
            &format!(
                "INSERT INTO synthesis_log
                 (task_id, reason_type, reason_detail, snapshot_confidence,
                  snapshot_attention_state, message, delivered, delivered_at, created_at,
                  stage2_decision, stage2_channel, stage2_reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7,
                         CASE WHEN ?7 = 1 THEN ({now}) ELSE NULL END,
                         ({now}), ?8, ?9, ?10)",
                now = SQLITE_NOW_EXPR,
            ),
            params![
                task_id,
                reason_type.trim(),
                reason_detail,
                snapshot_confidence,
                snapshot_attention_state.trim(),
                message,
                if delivered { 1 } else { 0 },
                stage2_decision,
                stage2_channel,
                stage2_reason,
            ],
        )
        .context("failed to log staged synthesis decision")?;
        Ok(conn.last_insert_rowid())
    }

    // apex c2: record a delivered interjection into the ledger (reaction pending).
    pub fn record_interruption(
        &self,
        task_id: Option<i64>,
        reason_type: &str,
        channel: &str,
        focus_score: f32,
    ) -> Result<i64> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO interruption_ledger (task_id, reason_type, channel, focus_score)
             VALUES (?1, ?2, ?3, ?4)",
            params![task_id, reason_type.trim(), channel.trim(), focus_score],
        )
        .context("failed to record interruption")?;
        Ok(conn.last_insert_rowid())
    }

    // record the user's reaction to the most recent still-pending interruption
    // for a task delivered within the window. returns the affected row id.
    pub fn record_interruption_reaction_within(
        &self,
        task_id: i64,
        window_seconds: i64,
        reaction: &str,
    ) -> Result<Option<i64>> {
        let conn = self.connect()?;
        let id = conn
            .query_row(
                "SELECT id FROM interruption_ledger
                 WHERE task_id = ?1 AND reaction IS NULL
                   AND CAST(strftime('%s','now') AS INTEGER)
                       - CAST(strftime('%s', delivered_at) AS INTEGER) <= ?2
                 ORDER BY id DESC LIMIT 1",
                params![task_id, window_seconds],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .context("failed to find pending interruption")?;
        if let Some(id) = id {
            conn.execute(
                "UPDATE interruption_ledger
                 SET reaction = ?1, reaction_at = datetime('now') WHERE id = ?2",
                params![reaction.trim(), id],
            )
            .context("failed to record interruption reaction")?;
        }
        Ok(id)
    }

    pub fn list_recent_interruptions(
        &self,
        task_id: i64,
        limit: usize,
    ) -> Result<Vec<InterruptionLedgerRow>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT reason_type, focus_score, reaction,
                        CAST(strftime('%s', delivered_at) AS INTEGER)
                 FROM interruption_ledger
                 WHERE task_id = ?1
                 ORDER BY id DESC LIMIT ?2",
            )
            .context("failed to prepare interruption query")?;
        let rows = stmt
            .query_map(params![task_id, limit as i64], |row| {
                Ok(InterruptionLedgerRow {
                    reason_type: row.get(0)?,
                    focus_score: row.get::<_, f64>(1)? as f32,
                    reaction: row.get(2)?,
                    delivered_at_unix: row.get(3)?,
                })
            })
            .context("failed to query interruptions")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to collect interruptions")
    }

    // weekly self-audit: (delivered, engaged) over the last `days` days.
    pub fn interruption_audit(&self, days: i64) -> Result<(i64, i64)> {
        let conn = self.connect()?;
        let delivered: i64 = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM interruption_ledger
                     WHERE delivered_at >= datetime('now', '-{days} days')"
                ),
                [],
                |row| row.get(0),
            )
            .context("failed to count delivered interruptions")?;
        let engaged: i64 = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM interruption_ledger
                     WHERE reaction = 'engaged'
                       AND delivered_at >= datetime('now', '-{days} days')"
                ),
                [],
                |row| row.get(0),
            )
            .context("failed to count engaged interruptions")?;
        Ok((delivered, engaged))
    }

    pub fn get_last_synthesis_at(&self, task_id: i64) -> Result<Option<i64>> {
        let conn = self.connect()?;
        let value = conn
            .query_row(
                "SELECT CAST(strftime('%s', delivered_at) AS INTEGER)
                 FROM synthesis_log
                 WHERE task_id = ?1
                   AND delivered = 1
                   AND delivered_at IS NOT NULL
                 ORDER BY delivered_at DESC, id DESC
                 LIMIT 1",
                params![task_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .context("failed to query last synthesis timestamp")?;
        Ok(value)
    }

    pub fn list_synthesis_log(
        &self,
        task_id: i64,
        limit: usize,
    ) -> Result<Vec<SynthesisLogEntryDto>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, task_id, reason_type, reason_detail, snapshot_confidence,
                        snapshot_attention_state, message, delivered, delivered_at, created_at
                 FROM synthesis_log
                 WHERE task_id = ?1
                 ORDER BY id DESC
                 LIMIT ?2",
            )
            .context("failed to prepare synthesis log query")?;

        let rows = stmt
            .query_map(params![task_id, limit as i64], synthesis_log_from_row)
            .context("failed to query synthesis log")?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.context("failed to map synthesis log row")?);
        }
        Ok(entries)
    }

    // ---------------------------------------------------------------------
    // phase 23: user profile
    // ---------------------------------------------------------------------

    pub fn get_profile_value(&self, key: &str) -> Result<Option<String>> {
        let conn = self.connect()?;
        let val = conn
            .query_row(
                "SELECT value FROM user_profile WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .context("failed to get profile value")?;
        Ok(val)
    }

    pub fn set_profile_value(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO user_profile (key, value, updated_at)
             VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
             ON CONFLICT(key) DO UPDATE SET
               value = excluded.value,
               updated_at = excluded.updated_at",
            params![key, value],
        )
        .context("failed to upsert profile value")?;
        Ok(())
    }

    pub fn get_all_profile_signals(&self) -> Result<Vec<(String, String, String)>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare("SELECT key, value, updated_at FROM user_profile ORDER BY updated_at DESC")
            .context("failed to prepare profile query")?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .context("failed to query user profile")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to collect profile rows")?;
        Ok(rows)
    }

    pub fn delete_profile_signal(&self, key: &str) -> Result<()> {
        let conn = self.connect()?;
        conn.execute("DELETE FROM user_profile WHERE key = ?1", params![key])
            .context("failed to delete profile signal")?;
        Ok(())
    }

    // ---------------------------------------------------------------------
    // phase 23: live edit receipts
    // ---------------------------------------------------------------------

    pub fn create_live_edit_receipt(
        &self,
        task_id: Option<i64>,
        editor_surface: &str,
        document_title: &str,
        before_hash: &str,
        after_hash: &str,
        before_text: &str,
        after_text: &str,
    ) -> Result<i64> {
        let task_id = task_id.ok_or_else(|| {
            anyhow!("live edit requests require an active task for audit ownership")
        })?;
        let action_receipt_id = self
            .create_action_receipt(
                task_id,
                "doc.replace",
                editor_surface,
                "L1",
                &format!("Live edit request for {}", document_title.trim()),
                &format!("{} -> {}", before_hash, after_hash),
                "pending_approval",
                None,
                None,
            )?
            .id;
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO live_edit_receipts
             (task_id, action_receipt_id, editor_surface, document_title, before_hash, after_hash, before_text, after_text)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                task_id,
                action_receipt_id,
                editor_surface,
                document_title,
                before_hash,
                after_hash,
                before_text,
                after_text
            ],
        )
        .context("failed to create live edit receipt")?;
        Ok(conn.last_insert_rowid())
    }

    pub fn create_guided_live_edit_receipt(
        &self,
        task_id: Option<i64>,
        action_receipt_id: i64,
        editor_surface: &str,
        document_title: &str,
        fallback_reason: &str,
        before_text: &str,
        after_text: &str,
    ) -> Result<i64> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO live_edit_receipts
             (task_id, action_receipt_id, editor_surface, document_title, before_hash, after_hash, before_text, after_text, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'fallback')",
            params![
                task_id,
                action_receipt_id,
                editor_surface,
                document_title,
                fallback_reason,
                "guided",
                before_text,
                after_text
            ],
        )
        .context("failed to create guided live edit receipt")?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_live_edit_status(
        &self,
        receipt_id: i64,
        status: &str,
    ) -> Result<crate::models::LiveEditReceiptDto> {
        if !matches!(
            status,
            "pending_approval" | "approved" | "rejected" | "applied" | "failed" | "fallback"
        ) {
            return Err(anyhow!("invalid live edit status: {status}"));
        }
        let conn = self.connect()?;
        let action_receipt_id: Option<i64> = conn
            .query_row(
                "SELECT action_receipt_id FROM live_edit_receipts WHERE id = ?1",
                params![receipt_id],
                |row| row.get(0),
            )
            .optional()
            .context("failed to query live edit action receipt id")?
            .flatten();
        let changed = conn
            .execute(
                "UPDATE live_edit_receipts SET status = ?1 WHERE id = ?2",
                params![status, receipt_id],
            )
            .context("failed to update live edit receipt status")?;
        if changed == 0 {
            return Err(anyhow!("live edit receipt id={receipt_id} not found"));
        }
        if let Some(action_receipt_id) = action_receipt_id {
            let mapped = match status {
                "fallback" => "guided",
                other => other,
            };
            let _ = self.update_action_receipt_status(action_receipt_id, mapped, None, None);
        }
        let receipt = conn
            .query_row(
                "SELECT id, task_id, editor_surface, document_title, before_hash, after_hash, timestamp, status
                 FROM live_edit_receipts WHERE id = ?1",
                params![receipt_id],
                |row| {
                    Ok(crate::models::LiveEditReceiptDto {
                        id: row.get(0)?,
                        task_id: row.get(1)?,
                        editor_surface: row.get(2)?,
                        document_title: row.get(3)?,
                        before_hash: row.get(4)?,
                        after_hash: row.get(5)?,
                        timestamp: row.get(6)?,
                        status: row.get(7)?,
                    })
                },
            )
            .context("failed to read updated live edit receipt")?;
        Ok(receipt)
    }

    pub fn list_live_edit_receipts(
        &self,
        task_id: Option<i64>,
    ) -> Result<Vec<crate::models::LiveEditReceiptDto>> {
        let conn = self.connect()?;
        let sql = if task_id.is_some() {
            "SELECT id, task_id, editor_surface, document_title, before_hash, after_hash, timestamp, status
             FROM live_edit_receipts WHERE task_id = ?1 ORDER BY timestamp DESC LIMIT 100"
        } else {
            "SELECT id, task_id, editor_surface, document_title, before_hash, after_hash, timestamp, status
             FROM live_edit_receipts ORDER BY timestamp DESC LIMIT 100"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(task_id), |row| {
                Ok(crate::models::LiveEditReceiptDto {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    editor_surface: row.get(2)?,
                    document_title: row.get(3)?,
                    before_hash: row.get(4)?,
                    after_hash: row.get(5)?,
                    timestamp: row.get(6)?,
                    status: row.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to collect live edit receipts")?;
        Ok(rows)
    }

    pub fn get_unresolved_live_edits(&self) -> Result<Vec<crate::models::PendingLiveEditDto>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, task_id, editor_surface, document_title, before_text, after_text, timestamp, status
             FROM live_edit_receipts
             WHERE status IN ('pending_approval', 'fallback', 'failed')
             ORDER BY timestamp ASC",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(crate::models::PendingLiveEditDto {
                    receipt_id: row.get(0)?,
                    task_id: row.get(1)?,
                    editor_surface: row.get(2)?,
                    document_title: row.get(3)?,
                    before_text: row.get(4)?,
                    after_text: row.get(5)?,
                    timestamp: row.get(6)?,
                    status: row.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to collect unresolved live edits")?;
        Ok(rows)
    }

    // ---------------------------------------------------------------------
    // phase 21 privacy/data controls
    // ---------------------------------------------------------------------

    pub fn count_user_profile_signals(&self) -> Result<i64> {
        let conn = self.connect()?;
        if !Self::table_exists(&conn, "user_profile")? {
            return Ok(0);
        }
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM user_profile", [], |row| row.get(0))
            .context("failed to count user profile signals")?;
        Ok(count)
    }

    pub fn clear_user_profile(&self) -> Result<()> {
        let conn = self.connect()?;
        if Self::table_exists(&conn, "user_profile")? {
            conn.execute("DELETE FROM user_profile", [])
                .context("failed to clear user profile")?;
        }
        Ok(())
    }

    pub fn clear_task_data(&self, task_id: i64) -> Result<()> {
        let task = self
            .get_task_by_id(task_id)?
            .ok_or_else(|| anyhow!("task id={} not found", task_id))?;

        // Capture only server-owned undo paths before deleting their receipts.
        // They are removed after the database transaction commits so a failed
        // clear never leaves the DB claiming data was removed when it was not.
        let undo_refs = {
            let conn = self.connect()?;
            let mut stmt = conn.prepare(
                "SELECT id FROM action_receipts WHERE task_id = ?1",
            )?;
            let receipt_ids = stmt
                .query_map(params![task_id], |row| row.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
                ;
            receipt_ids
                .into_iter()
                .map(|receipt_id| {
                    self.action_undo_root()
                        .join(receipt_id.to_string())
                        .display()
                        .to_string()
                })
                .collect::<Vec<_>>()
        };

        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("failed to start clear_task_data transaction")?;

        tx.execute(
            "DELETE FROM live_edit_receipts
             WHERE task_id = ?1
                OR action_receipt_id IN (SELECT id FROM action_receipts WHERE task_id = ?1)",
            params![task_id],
        )
        .context("failed to clear live edit receipts")?;
        tx.execute(
            "DELETE FROM interruption_ledger WHERE task_id = ?1",
            params![task_id],
        )
        .context("failed to clear interruption ledger")?;
        if Self::table_exists_tx(&tx, "speculation_events")? {
            tx.execute(
                "DELETE FROM speculation_events WHERE task_id = ?1",
                params![task_id],
            )
            .context("failed to clear speculation events")?;
        }
        if Self::table_exists_tx(&tx, "speculation_cache")? {
            tx.execute(
                "DELETE FROM speculation_cache WHERE task_id = ?1",
                params![task_id],
            )
            .context("failed to clear speculation cache")?;
        }
        tx.execute(
            "DELETE FROM subtask_write_audit_log WHERE task_id = ?1",
            params![task_id],
        )
        .context("failed to clear subtask write audit")?;
        tx.execute(
            "DELETE FROM action_receipts WHERE task_id = ?1",
            params![task_id],
        )
        .context("failed to clear action receipts")?;
        tx.execute(
            "DELETE FROM subtask_file_write_proposals WHERE task_id = ?1",
            params![task_id],
        )
        .context("failed to clear file write proposals")?;
        tx.execute(
            "DELETE FROM subtask_steps
             WHERE subtask_id IN (SELECT subtask_id FROM subtasks WHERE task_id = ?1)",
            params![task_id],
        )
        .context("failed to clear subtask steps")?;
        tx.execute("DELETE FROM subtasks WHERE task_id = ?1", params![task_id])
            .context("failed to clear subtasks")?;
        if Self::table_exists_tx(&tx, "jobs")? {
            if Self::table_exists_tx(&tx, "standing_jobs")? {
                tx.execute("DELETE FROM standing_jobs WHERE task_id = ?1", params![task_id])
                    .context("failed to clear standing jobs")?;
            }
            if Self::table_exists_tx(&tx, "job_steering")? {
                tx.execute(
                    "DELETE FROM job_steering
                     WHERE job_id IN (SELECT id FROM jobs WHERE task_id = ?1)",
                    params![task_id],
                )
                .context("failed to clear job steering")?;
            }
            if Self::table_exists_tx(&tx, "job_checkpoints")? {
                tx.execute(
                    "DELETE FROM job_checkpoints
                     WHERE job_id IN (SELECT id FROM jobs WHERE task_id = ?1)",
                    params![task_id],
                )
                .context("failed to clear job checkpoints")?;
            }
            tx.execute(
                "DELETE FROM job_events
                 WHERE job_id IN (SELECT id FROM jobs WHERE task_id = ?1)",
                params![task_id],
            )
            .context("failed to clear job events")?;
            tx.execute(
                "DELETE FROM job_artifacts
                 WHERE job_id IN (SELECT id FROM jobs WHERE task_id = ?1)",
                params![task_id],
            )
            .context("failed to clear job artifacts")?;
            tx.execute(
                "DELETE FROM job_steps
                 WHERE job_id IN (SELECT id FROM jobs WHERE task_id = ?1)",
                params![task_id],
            )
            .context("failed to clear job steps")?;
            tx.execute("DELETE FROM jobs WHERE task_id = ?1", params![task_id])
                .context("failed to clear jobs")?;
        }
        tx.execute(
            "DELETE FROM artifact_versions WHERE task_id = ?1",
            params![task_id],
        )
        .context("failed to clear artifact versions")?;
        tx.execute(
            "DELETE FROM artifact_revisions WHERE task_id = ?1",
            params![task_id],
        )
        .context("failed to clear artifact revisions")?;
        tx.execute(
            "DELETE FROM artifact_chunks WHERE task_id = ?1",
            params![task_id],
        )
        .context("failed to clear artifact chunks")?;
        tx.execute("DELETE FROM artifacts WHERE task_id = ?1", params![task_id])
            .context("failed to clear artifacts")?;
        tx.execute(
            "DELETE FROM chat_messages WHERE task_id = ?1",
            params![task_id],
        )
        .context("failed to clear chat messages")?;
        tx.execute("DELETE FROM sessions WHERE task_id = ?1", params![task_id])
            .context("failed to clear sessions")?;
        tx.execute(
            "DELETE FROM task_summaries WHERE task_id = ?1",
            params![task_id],
        )
        .context("failed to clear task summary")?;
        tx.execute(
            "DELETE FROM open_resources WHERE task_id = ?1",
            params![task_id],
        )
        .context("failed to clear open resources")?;
        tx.execute("DELETE FROM event_log WHERE task_id = ?1", params![task_id])
            .context("failed to clear event log")?;
        tx.execute(
            "DELETE FROM watched_file_registry WHERE task_id = ?1",
            params![task_id],
        )
        .context("failed to clear watched file registry")?;
        tx.execute(
            "DELETE FROM watched_folders WHERE task_id = ?1",
            params![task_id],
        )
        .context("failed to clear watched folders")?;
        tx.execute(
            "DELETE FROM clipboard_capture_settings WHERE task_id = ?1",
            params![task_id],
        )
        .context("failed to clear clipboard capture setting")?;
        tx.execute(
            "DELETE FROM recently_learned_log WHERE task_id = ?1",
            params![task_id],
        )
        .context("failed to clear recently learned log")?;
        tx.execute(
            "DELETE FROM suggestions WHERE task_id = ?1",
            params![task_id],
        )
        .context("failed to clear suggestions")?;
        tx.execute(
            "DELETE FROM session_mode_state WHERE task_id = ?1",
            params![task_id],
        )
        .context("failed to clear session mode state")?;
        tx.execute(
            "DELETE FROM proactive_trigger_log WHERE task_id = ?1",
            params![task_id],
        )
        .context("failed to clear proactive trigger log")?;
        tx.execute(
            "DELETE FROM synthesis_log WHERE task_id = ?1",
            params![task_id],
        )
        .context("failed to clear synthesis log")?;
        tx.execute(
            "DELETE FROM task_focus_log WHERE task_id = ?1",
            params![task_id],
        )
        .context("failed to clear task focus log")?;
        if Self::table_exists_tx(&tx, "stated_goals")? {
            tx.execute(
                "DELETE FROM stated_goals WHERE task_id = ?1",
                params![task_id],
            )
            .context("failed to clear stated goals")?;
        }
        if Self::table_exists_tx(&tx, "episodes")? {
            tx.execute("DELETE FROM episodes WHERE task_id = ?1", params![task_id])
                .context("failed to clear episodes")?;
        }
        tx.execute(
            "DELETE FROM app_settings WHERE key = ?1",
            params![format!("active_artifact_task_{task_id}")],
        )
        .context("failed to clear active artifact selection")?;

        tx.commit()
            .context("failed to commit clear_task_data transaction")?;

        for undo_ref in undo_refs {
            self.remove_owned_undo_path(&undo_ref)?;
        }
        self.remove_internal_task_workspace_contents(&task.workspace_path)?;
        Ok(())
    }

    pub fn clear_all_data(&self) -> Result<()> {
        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("failed to start clear_all_data transaction")?;

        // Tables with nullable/no task foreign keys must be cleared before the
        // task cascade or their sensitive payloads would survive CLEAR JEFF.
        for table in [
            "live_edit_receipts",
            "interruption_ledger",
            "speculation_events",
            "custom_tools",
            "capability_gaps",
            "llm_usage_log",
        ] {
            if Self::table_exists_tx(&tx, table)? {
                tx.execute(&format!("DELETE FROM {table}"), [])
                    .with_context(|| format!("failed to clear {table}"))?;
            }
        }
        tx.execute("DELETE FROM tasks", [])
            .context("failed to clear tasks")?;
        tx.execute("DELETE FROM app_settings", [])
            .context("failed to clear app settings")?;
        if Self::table_exists_tx(&tx, "user_profile")? {
            tx.execute("DELETE FROM user_profile", [])
                .context("failed to clear user profile")?;
        }
        if Self::table_exists_tx(&tx, "stated_goals")? {
            tx.execute("DELETE FROM stated_goals", [])
                .context("failed to clear stated goals")?;
        }
        if Self::table_exists_tx(&tx, "struggle_patterns")? {
            tx.execute("DELETE FROM struggle_patterns", [])
                .context("failed to clear struggle patterns")?;
        }
        if Self::table_exists_tx(&tx, "collaboration_style_signals")? {
            tx.execute("DELETE FROM collaboration_style_signals", [])
                .context("failed to clear collaboration style")?;
        }
        if Self::table_exists_tx(&tx, "trust_metrics")? {
            tx.execute("DELETE FROM trust_metrics", [])
                .context("failed to clear trust metrics")?;
        }
        if Self::table_exists_tx(&tx, "trust_levels")? {
            tx.execute("DELETE FROM trust_levels", [])
                .context("failed to clear trust levels")?;
        }
        if Self::table_exists_tx(&tx, "episodes")? {
            tx.execute("DELETE FROM episodes", [])
                .context("failed to clear episodes")?;
        }
        if Self::table_exists_tx(&tx, "facts")? {
            tx.execute("DELETE FROM facts", [])
                .context("failed to clear facts")?;
        }
        let _ = tx.execute("DELETE FROM sqlite_sequence", []);

        tx.commit()
            .context("failed to commit clear_all_data transaction")?;

        if self.paths.workspace_root.exists() {
            fs::remove_dir_all(&self.paths.workspace_root).with_context(|| {
                format!(
                    "failed to remove workspace root {}",
                    self.paths.workspace_root.display()
                )
            })?;
        }
        fs::create_dir_all(&self.paths.workspace_root).with_context(|| {
            format!(
                "failed to recreate workspace root {}",
                self.paths.workspace_root.display()
            )
        })?;
        for owned_root in [self.action_undo_root(), self.custom_tools_root()] {
            if owned_root.exists() {
                fs::remove_dir_all(&owned_root).with_context(|| {
                    format!("failed to remove sensitive root {}", owned_root.display())
                })?;
            }
        }
        Ok(())
    }

    // ---------------------------------------------------------------------
    // shared helpers
    // ---------------------------------------------------------------------

    fn remove_owned_undo_path(&self, undo_ref: &str) -> Result<()> {
        let root = self.action_undo_root();
        let candidate = PathBuf::from(undo_ref);
        let owned = candidate.parent() == Some(root.as_path())
            && candidate
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| !name.is_empty() && name.chars().all(|ch| ch.is_ascii_digit()));
        if !owned {
            return Err(anyhow!(
                "refusing to remove unowned undo path {}",
                candidate.display()
            ));
        }
        let Ok(metadata) = fs::symlink_metadata(&candidate) else {
            return Ok(());
        };
        if metadata.file_type().is_symlink() || metadata.is_file() {
            fs::remove_file(&candidate)
                .with_context(|| format!("failed to remove undo path {}", candidate.display()))?;
        } else if metadata.is_dir() {
            fs::remove_dir_all(&candidate)
                .with_context(|| format!("failed to remove undo path {}", candidate.display()))?;
        }
        Ok(())
    }

    fn get_task_by_id(&self, task_id: i64) -> Result<Option<TaskDto>> {
        let conn = self.connect()?;
        let task = conn
            .query_row(
                "SELECT id, title, slug, workspace_path, created_at, updated_at, is_active
                 FROM tasks
                 WHERE id = ?1",
                params![task_id],
                task_from_row,
            )
            .optional()
            .context("failed to query task by id")?;
        Ok(task)
    }

    fn get_artifact_by_id(&self, artifact_id: i64) -> Result<Option<ArtifactDto>> {
        let conn = self.connect()?;
        let artifact = conn
            .query_row(
                "SELECT a.id,
                        a.task_id,
                        a.file_name,
                        a.file_extension,
                        a.original_path,
                        a.stored_path,
                        a.created_at,
                        a.updated_at,
                        COUNT(c.id) AS chunk_count
                 FROM artifacts a
                 LEFT JOIN artifact_chunks c ON c.artifact_id = a.id
                 WHERE a.id = ?1
                 GROUP BY a.id",
                params![artifact_id],
                artifact_from_row,
            )
            .optional()
            .context("failed to query artifact by id")?;
        Ok(artifact)
    }

    fn get_chat_message_by_id(&self, message_id: i64) -> Result<Option<ChatMessageDto>> {
        let conn = self.connect()?;
        let message = conn
            .query_row(
                "SELECT id, task_id, session_id, role, message_source, message_kind, content, created_at
                 FROM chat_messages
                 WHERE id = ?1",
                params![message_id],
                chat_message_from_row,
            )
            .optional()
            .context("failed to query chat message by id")?;
        Ok(message)
    }

    fn get_artifact_version_by_id(&self, version_id: i64) -> Result<Option<ArtifactVersionDto>> {
        let conn = self.connect()?;
        let version = conn
            .query_row(
                "SELECT version_id,
                        task_id,
                        artifact_id,
                        revision_id,
                        version_reason,
                        content_preview,
                        content_length,
                        created_at
                 FROM artifact_versions
                 WHERE version_id = ?1",
                params![version_id],
                artifact_version_from_row,
            )
            .optional()
            .context("failed to query artifact version by id")?;
        Ok(version)
    }

    fn get_recently_learned_by_id(&self, id: i64) -> Result<Option<RecentlyLearnedItemDto>> {
        let conn = self.connect()?;
        let item = conn
            .query_row(
                "SELECT id, task_id, source, display_label, preview_text, ingested_at
                 FROM recently_learned_log
                 WHERE id = ?1",
                params![id],
                recently_learned_from_row,
            )
            .optional()
            .context("failed to query recently learned by id")?;
        Ok(item)
    }

    fn get_write_audit_entry_by_id(&self, id: i64) -> Result<Option<WriteAuditEntryDto>> {
        let conn = self.connect()?;
        let entry = conn
            .query_row(
                "SELECT id, task_id, subtask_id, proposal_id, action, proposed_path, resolved_at
                 FROM subtask_write_audit_log
                 WHERE id = ?1",
                params![id],
                write_audit_from_row,
            )
            .optional()
            .context("failed to query write audit entry by id")?;
        Ok(entry)
    }

    fn remove_internal_task_workspace_contents(&self, workspace_path: &str) -> Result<()> {
        let workspace = PathBuf::from(workspace_path);
        if workspace.as_os_str().is_empty() || !workspace.starts_with(&self.paths.workspace_root) {
            return Ok(());
        }

        if workspace.exists() {
            for entry in fs::read_dir(&workspace)
                .with_context(|| format!("failed to read task workspace {}", workspace.display()))?
            {
                let path = entry
                    .with_context(|| {
                        format!(
                            "failed to read entry in task workspace {}",
                            workspace.display()
                        )
                    })?
                    .path();
                if path.is_dir() {
                    fs::remove_dir_all(&path)
                        .with_context(|| format!("failed to remove {}", path.display()))?;
                } else {
                    fs::remove_file(&path)
                        .with_context(|| format!("failed to remove {}", path.display()))?;
                }
            }
        } else {
            fs::create_dir_all(&workspace).with_context(|| {
                format!("failed to recreate task workspace {}", workspace.display())
            })?;
        }

        Ok(())
    }

    fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*)
                 FROM sqlite_master
                 WHERE type = 'table' AND name = ?1",
                params![table],
                |row| row.get(0),
            )
            .with_context(|| format!("failed to check whether table '{}' exists", table))?;
        Ok(exists > 0)
    }

    fn table_exists_tx(tx: &Transaction<'_>, table: &str) -> Result<bool> {
        let exists: i64 = tx
            .query_row(
                "SELECT COUNT(*)
                 FROM sqlite_master
                 WHERE type = 'table' AND name = ?1",
                params![table],
                |row| row.get(0),
            )
            .with_context(|| format!("failed to check whether table '{}' exists", table))?;
        Ok(exists > 0)
    }

    fn next_available_slug(tx: &Transaction<'_>, base_slug: &str) -> Result<String> {
        let mut candidate = base_slug.to_string();
        let mut suffix: i64 = 2;

        loop {
            let exists: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM tasks WHERE slug = ?1",
                    params![candidate],
                    |row| row.get(0),
                )
                .context("failed to check slug uniqueness")?;

            if exists == 0 {
                return Ok(candidate);
            }

            candidate = format!("{}-{}", base_slug, suffix);
            suffix += 1;
        }
    }

    fn record_event_tx(
        tx: &Transaction<'_>,
        task_id: i64,
        event_type: &str,
        payload_json: &str,
    ) -> Result<()> {
        tx.execute(
            &format!(
                "INSERT INTO event_log (task_id, event_type, payload_json, created_at)
                 VALUES (?1, ?2, ?3, ({now}))",
                now = SQLITE_NOW_EXPR,
            ),
            params![task_id, event_type.trim(), payload_json],
        )
        .with_context(|| format!("failed to record event '{}'", event_type))?;
        Ok(())
    }
}

// -------------------------------------------------------------------------
// row mappers
// -------------------------------------------------------------------------

fn task_from_row(row: &Row<'_>) -> rusqlite::Result<TaskDto> {
    Ok(TaskDto {
        id: row.get(0)?,
        title: row.get(1)?,
        slug: row.get(2)?,
        workspace_path: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        is_active: row.get::<_, i64>(6)? != 0,
    })
}

fn task_summary_from_row(row: &Row<'_>) -> rusqlite::Result<TaskSummaryDto> {
    Ok(TaskSummaryDto {
        task_id: row.get(0)?,
        summary_text: row.get(1)?,
        updated_at: row.get(2)?,
    })
}

fn open_resource_from_row(row: &Row<'_>) -> rusqlite::Result<OpenResourceDto> {
    Ok(OpenResourceDto {
        id: row.get(0)?,
        task_id: row.get(1)?,
        resource_type: row.get(2)?,
        resource_path_or_url: row.get(3)?,
        label: row.get(4)?,
        position_index: row.get(5)?,
    })
}

fn artifact_from_row(row: &Row<'_>) -> rusqlite::Result<ArtifactDto> {
    Ok(ArtifactDto {
        id: row.get(0)?,
        task_id: row.get(1)?,
        file_name: row.get(2)?,
        file_extension: row.get(3)?,
        original_path: row.get(4)?,
        stored_path: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        chunk_count: row.get(8)?,
    })
}

fn chat_message_from_row(row: &Row<'_>) -> rusqlite::Result<ChatMessageDto> {
    let stored_message_kind: String = row.get(5)?;
    let message_kind = if stored_message_kind == "assistant_proactive" {
        "proactive_reorientation".to_string()
    } else {
        stored_message_kind
    };

    Ok(ChatMessageDto {
        id: row.get(0)?,
        task_id: row.get(1)?,
        session_id: row.get(2)?,
        role: row.get(3)?,
        message_source: row.get(4)?,
        message_kind,
        content: row.get(6)?,
        created_at: row.get(7)?,
    })
}

fn revision_from_row(row: &Row<'_>) -> rusqlite::Result<RevisionProposalDto> {
    Ok(RevisionProposalDto {
        revision_id: row.get(0)?,
        task_id: row.get(1)?,
        artifact_id: row.get(2)?,
        target_start_offset: row.get(3)?,
        target_end_offset: row.get(4)?,
        target_description: row.get(5)?,
        original_text: row.get(6)?,
        proposed_text: row.get(7)?,
        instruction_text: row.get(8)?,
        instruction_source: row.get(9)?,
        rationale: row.get(10)?,
        grounding_notes: row.get(11)?,
        retrieval_confidence: row.get(12)?,
        status: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
        parent_revision_id: row.get(16)?,
    })
}

fn artifact_version_from_row(row: &Row<'_>) -> rusqlite::Result<ArtifactVersionDto> {
    Ok(ArtifactVersionDto {
        version_id: row.get(0)?,
        task_id: row.get(1)?,
        artifact_id: row.get(2)?,
        revision_id: row.get(3)?,
        version_reason: row.get(4)?,
        content_preview: row.get(5)?,
        content_length: row.get(6)?,
        created_at: row.get(7)?,
    })
}

fn subtask_from_row(row: &Row<'_>) -> rusqlite::Result<SubTaskDto> {
    Ok(SubTaskDto {
        subtask_id: row.get(0)?,
        task_id: row.get(1)?,
        title: row.get(2)?,
        description: row.get(3)?,
        execution_type: row.get(4)?,
        status: row.get(5)?,
        result_review_status: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        result_summary: row.get(9)?,
        result_payload: row.get(10)?,
        instruction_source: row.get(11)?,
        parent_context_snapshot: row.get(12)?,
        error_message: row.get(13)?,
    })
}

fn session_mode_from_row(row: &Row<'_>) -> rusqlite::Result<SessionModeStateDto> {
    Ok(SessionModeStateDto {
        task_id: row.get(0)?,
        current_mode: row.get(1)?,
        mode_reason: row.get(2)?,
        waiting_on_user_decision: row.get::<_, i64>(3)? != 0,
        last_engine_decision: row.get(4)?,
        active_artifact_id: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn suggestion_from_row(row: &Row<'_>) -> rusqlite::Result<SuggestionDto> {
    Ok(SuggestionDto {
        suggestion_id: row.get(0)?,
        task_id: row.get(1)?,
        title: row.get(2)?,
        description: row.get(3)?,
        suggestion_type: row.get(4)?,
        source_reason: row.get(5)?,
        status: row.get(6)?,
        suggestion_key: row.get(7)?,
        linked_context: row.get(8)?,
        linked_subtask_type: row.get(9)?,
        linked_revision_intent: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

fn event_log_from_row(row: &Row<'_>) -> rusqlite::Result<EventLogEntryDto> {
    Ok(EventLogEntryDto {
        id: row.get(0)?,
        task_id: row.get(1)?,
        event_type: row.get(2)?,
        payload_json: row.get(3)?,
        created_at: row.get(4)?,
    })
}

fn watched_folder_from_row(row: &Row<'_>) -> rusqlite::Result<WatchedFolderDto> {
    Ok(WatchedFolderDto {
        task_id: row.get(0)?,
        folder_path: row.get(1)?,
        is_active: row.get::<_, i64>(2)? != 0,
        ignore_rules_json: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn watched_file_registry_from_row(row: &Row<'_>) -> rusqlite::Result<WatchedFileRegistryEntry> {
    Ok(WatchedFileRegistryEntry {
        id: row.get(0)?,
        task_id: row.get(1)?,
        canonical_path: row.get(2)?,
        artifact_id: row.get(3)?,
        last_modified_at: row.get(4)?,
        ingested_at: row.get(5)?,
    })
}

fn recently_learned_from_row(row: &Row<'_>) -> rusqlite::Result<RecentlyLearnedItemDto> {
    Ok(RecentlyLearnedItemDto {
        id: row.get(0)?,
        task_id: row.get(1)?,
        source: row.get(2)?,
        display_label: row.get(3)?,
        preview_text: row.get(4)?,
        ingested_at: row.get(5)?,
    })
}

fn subtask_step_from_row(row: &Row<'_>) -> rusqlite::Result<SubTaskStepDto> {
    Ok(SubTaskStepDto {
        id: row.get(0)?,
        subtask_id: row.get(1)?,
        step_index: row.get(2)?,
        step_type: row.get(3)?,
        status: row.get(4)?,
        description: row.get(5)?,
        result_summary: row.get(6)?,
        result_payload: row.get(7)?,
        error_message: row.get(8)?,
        started_at: row.get(9)?,
        completed_at: row.get(10)?,
    })
}

fn file_write_proposal_from_row(row: &Row<'_>) -> rusqlite::Result<FileWriteProposalDto> {
    Ok(FileWriteProposalDto {
        id: row.get(0)?,
        subtask_id: row.get(1)?,
        step_id: row.get(2)?,
        task_id: row.get(3)?,
        proposed_path: row.get(4)?,
        proposed_content: row.get(5)?,
        status: row.get(6)?,
        proposed_at: row.get(7)?,
        resolved_at: row.get(8)?,
    })
}

fn write_audit_from_row(row: &Row<'_>) -> rusqlite::Result<WriteAuditEntryDto> {
    Ok(WriteAuditEntryDto {
        id: row.get(0)?,
        task_id: row.get(1)?,
        subtask_id: row.get(2)?,
        proposal_id: row.get(3)?,
        action: row.get(4)?,
        proposed_path: row.get(5)?,
        resolved_at: row.get(6)?,
        resolved_path: None,
        action_receipt_id: None,
    })
}

fn action_receipt_from_row(row: &Row<'_>) -> rusqlite::Result<ActionReceiptDto> {
    Ok(ActionReceiptDto {
        id: row.get(0)?,
        task_id: row.get(1)?,
        class: row.get(2)?,
        surface: row.get(3)?,
        level: row.get(4)?,
        description: row.get(5)?,
        payload_excerpt: row.get(6)?,
        status: row.get(7)?,
        failure_reason: row.get(8)?,
        undo_ref: row.get(9)?,
        created_at: row.get(10)?,
        resolved_at: row.get(11)?,
    })
}

fn is_terminal_action_status(status: &str) -> bool {
    matches!(status, "applied" | "rejected" | "reverted" | "failed" | "guided")
}

fn valid_action_status_transition(current: &str, next: &str) -> bool {
    if current == next {
        return true;
    }
    match current {
        "pending_approval" => matches!(
            next,
            "approved" | "rejected" | "guided" | "failed" | "applying"
        ),
        "running" => matches!(next, "applying" | "applied" | "failed"),
        "approved" => matches!(next, "applying" | "applied" | "rejected" | "failed"),
        "applying" => matches!(next, "applied" | "failed"),
        "applied" => next == "reverted",
        "rejected" | "reverted" | "failed" | "guided" => false,
        _ => false,
    }
}

fn proactive_audit_from_row(row: &Row<'_>) -> rusqlite::Result<ProactiveAuditEntryDto> {
    Ok(ProactiveAuditEntryDto {
        id: row.get(0)?,
        task_id: row.get(1)?,
        trigger_type: row.get(2)?,
        fired_at: row.get(3)?,
        suppressed: row.get::<_, i64>(4)? != 0,
    })
}

fn synthesis_log_from_row(row: &Row<'_>) -> rusqlite::Result<SynthesisLogEntryDto> {
    Ok(SynthesisLogEntryDto {
        id: row.get(0)?,
        task_id: row.get(1)?,
        reason_type: row.get(2)?,
        reason_detail: row.get(3)?,
        snapshot_confidence: row.get(4)?,
        snapshot_attention_state: row.get(5)?,
        message: row.get(6)?,
        delivered: row.get::<_, i64>(7)? != 0,
        delivered_at: row.get(8)?,
        created_at: row.get(9)?,
    })
}

fn compact_preview(content: &str, max_chars: usize) -> String {
    let compact = content.split_whitespace().collect::<Vec<&str>>().join(" ");

    if compact.chars().count() <= max_chars {
        return compact;
    }

    let mut trimmed: String = compact.chars().take(max_chars).collect();
    while trimmed.ends_with(' ') {
        trimmed.pop();
    }
    format!("{trimmed}...")
}

fn now_string() -> Result<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock is before unix epoch")?
        .as_secs() as i64;

    let (year, month, day, hour, minute, second) = unix_to_ymd_hms(now);
    Ok(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, minute, second
    ))
}

fn unix_to_ymd_hms(secs: i64) -> (i64, i64, i64, i64, i64, i64) {
    let hour = (secs / 3600) % 24;
    let minute = (secs / 60) % 60;
    let second = secs % 60;
    let mut days = secs / 86400;

    let leap = |year: i64| -> bool { (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 };

    let mut year = 1970i64;
    loop {
        let days_in_year = if leap(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }

    let month_days = [
        31i64,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];

    let mut month = 1i64;
    for day_count in month_days {
        if days < day_count {
            break;
        }
        days -= day_count;
        month += 1;
    }

    (year, month, days + 1, hour, minute, second)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn new_test_store() -> (TempDir, TaskStore) {
        let dir = TempDir::new().expect("failed to create temp dir");
        let store = TaskStore::initialize(dir.path()).expect("failed to initialize store");
        (dir, store)
    }

    #[test]
    fn legacy_assistant_proactive_rows_normalize_to_phase_28_kind() {
        let (_dir, store) = new_test_store();
        let task = store.create_task("legacy proactive").unwrap();
        let conn = store.connect().unwrap();
        conn.execute(
            "INSERT INTO chat_messages
             (task_id, session_id, role, message_source, message_kind, content, created_at)
             VALUES (?1, NULL, 'assistant', 'assistant', 'assistant_proactive', 'legacy', '2026-04-29T00:00:00Z')",
            rusqlite::params![task.id],
        )
        .unwrap();

        let messages = store.list_chat_messages(task.id).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].message_kind, "proactive_reorientation");
    }

    #[test]
    fn schema_table_count_matches_phase_16_expectation() {
        let (_dir, store) = new_test_store();
        let conn = store.connect().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*)
                 FROM sqlite_master
                 WHERE type = 'table'
                   AND name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(
            count, 53,
            "expected 53 application tables after apex e5 (added remote_ingested_docs)"
        );
    }

    #[test]
    fn task_creation_and_activation_round_trip() {
        let (_dir, store) = new_test_store();
        let first = store.create_task("History StoryMap").unwrap();
        let second = store.create_task("History StoryMap").unwrap();

        assert_eq!(first.slug, "history-storymap");
        assert_eq!(second.slug, "history-storymap-2");
        assert!(PathBuf::from(first.workspace_path).exists());
        assert!(PathBuf::from(second.workspace_path).exists());

        let active = store.get_active_task().unwrap().unwrap();
        assert_eq!(active.id, first.id);

        let switched = store.set_active_task(second.id).unwrap();
        assert_eq!(switched.id, second.id);
        assert!(switched.is_active);

        let active_after = store.get_active_task().unwrap().unwrap();
        assert_eq!(active_after.id, second.id);
    }

    #[test]
    fn onboarding_settings_round_trip() {
        let (_dir, store) = new_test_store();

        assert!(!store.get_onboarding_complete().unwrap());
        assert!(store.get_preferred_workspace_folder().unwrap().is_none());

        store.set_onboarding_complete(true).unwrap();
        assert!(store.get_onboarding_complete().unwrap());

        store
            .set_preferred_workspace_folder(Some("/tmp/jeff/workspace"))
            .unwrap();
        assert_eq!(
            store.get_preferred_workspace_folder().unwrap().as_deref(),
            Some("/tmp/jeff/workspace")
        );

        store.set_preferred_workspace_folder(None).unwrap();
        assert!(store.get_preferred_workspace_folder().unwrap().is_none());
    }

    #[test]
    fn watched_folder_round_trips_and_clear_works() {
        let (_dir, store) = new_test_store();
        let task = store.create_task("Watcher").unwrap();

        let set = store
            .set_watched_folder(task.id, "/tmp/watch-root")
            .unwrap();
        assert_eq!(set.task_id, task.id);
        assert!(set.is_active);

        let fetched = store.get_watched_folder(task.id).unwrap().unwrap();
        assert_eq!(fetched.folder_path, "/tmp/watch-root");

        store.clear_watched_folder(task.id).unwrap();
        assert!(store.get_watched_folder(task.id).unwrap().is_none());
    }

    #[test]
    fn file_registry_upsert_and_remove_round_trip() {
        let (_dir, store) = new_test_store();
        let task = store.create_task("Registry").unwrap();

        let first = store
            .upsert_file_registry_entry(task.id, "/tmp/a.md", Some(42), "v1")
            .unwrap();
        assert_eq!(first.canonical_path, "/tmp/a.md");
        assert_eq!(first.artifact_id, Some(42));
        assert_eq!(first.last_modified_at, "v1");

        let second = store
            .upsert_file_registry_entry(task.id, "/tmp/a.md", Some(43), "v2")
            .unwrap();
        assert_eq!(second.artifact_id, Some(43));
        assert_eq!(second.last_modified_at, "v2");

        store
            .remove_file_registry_entry(task.id, "/tmp/a.md")
            .unwrap();
        assert!(store
            .get_file_registry_entry(task.id, "/tmp/a.md")
            .unwrap()
            .is_none());
    }

    #[test]
    fn clipboard_defaults_off_and_can_toggle() {
        let (_dir, store) = new_test_store();
        let task = store.create_task("Clipboard").unwrap();

        assert!(!store.get_clipboard_capture(task.id).unwrap());
        store.set_clipboard_capture(task.id, true).unwrap();
        assert!(store.get_clipboard_capture(task.id).unwrap());
        store.set_clipboard_capture(task.id, false).unwrap();
        assert!(!store.get_clipboard_capture(task.id).unwrap());
    }

    #[test]
    fn recently_learned_round_trip_ordering() {
        let (_dir, store) = new_test_store();
        let task = store.create_task("Learned").unwrap();

        store
            .append_recently_learned(task.id, "file", "notes.md", "first")
            .unwrap();
        store
            .append_recently_learned(task.id, "clipboard", "snippet", "second")
            .unwrap();

        let rows = store.list_recently_learned(task.id, 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].source, "clipboard");
        assert_eq!(rows[1].source, "file");
    }

    #[test]
    fn proactive_trigger_cooldown_ignores_suppressed_entries() {
        let (_dir, store) = new_test_store();
        let task = store.create_task("Proactive").unwrap();

        store
            .record_proactive_trigger(task.id, "resume", true)
            .unwrap();
        assert!(store
            .get_last_proactive_trigger(task.id, "resume")
            .unwrap()
            .is_none());

        store
            .record_proactive_trigger(task.id, "resume", false)
            .unwrap();
        assert!(store
            .get_last_proactive_trigger(task.id, "resume")
            .unwrap()
            .is_some());
    }

    #[test]
    fn synthesis_log_round_trips_suppressed_and_delivered_entries() {
        let (_dir, store) = new_test_store();
        let task = store.create_task("Synthesis").unwrap();

        let suppressed_id = store
            .log_synthesis_decision(
                Some(task.id),
                "suppressed",
                Some("no_reason"),
                0.2,
                "idle",
                None,
                false,
            )
            .unwrap();
        assert!(suppressed_id > 0);
        assert!(store.get_last_synthesis_at(task.id).unwrap().is_none());

        let delivered_id = store
            .log_synthesis_decision(
                Some(task.id),
                "task_return",
                Some("idle_minutes=8"),
                0.8,
                "returning",
                Some("You have been away for a bit; the launch memo is still open."),
                true,
            )
            .unwrap();
        assert!(delivered_id > suppressed_id);

        let entries = store.list_synthesis_log(task.id, 10).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].reason_type, "task_return");
        assert!(entries[0].delivered);
        assert!(entries[0].delivered_at.is_some());
        assert_eq!(entries[1].reason_type, "suppressed");
        assert!(!entries[1].delivered);
        assert!(entries[1].delivered_at.is_none());
        assert!(store.get_last_synthesis_at(task.id).unwrap().is_some());
    }

    #[test]
    fn synthesis_log_records_quiet_mode_suppression() {
        let (_dir, store) = new_test_store();
        let task = store.create_task("Quiet Synthesis").unwrap();

        store
            .log_synthesis_decision(
                Some(task.id),
                "task_return",
                Some("idle_minutes=8; suppressed=quiet_mode"),
                0.8,
                "returning",
                None,
                false,
            )
            .unwrap();

        let entries = store.list_synthesis_log(task.id, 10).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].reason_detail.as_deref(),
            Some("idle_minutes=8; suppressed=quiet_mode")
        );
        assert!(!entries[0].delivered);
        assert!(entries[0].message.is_none());
    }

    #[test]
    fn subtask_steps_create_and_update_round_trip() {
        let (_dir, store) = new_test_store();
        let task = store.create_task("Subtask Steps").unwrap();

        let subtask = store
            .create_subtask(&NewSubTaskInput {
                task_id: task.id,
                title: "Chain task".to_string(),
                description: "Run steps".to_string(),
                execution_type: "synthesis".to_string(),
                instruction_source: "text".to_string(),
                parent_context_snapshot: "{}".to_string(),
            })
            .unwrap();

        let step = store
            .create_subtask_step(subtask.subtask_id, 0, "llm_call", "draft section")
            .unwrap();
        assert_eq!(step.status, "pending");

        let running = store
            .update_subtask_step_status(step.id, "running", None, None, None)
            .unwrap();
        assert_eq!(running.status, "running");
        assert!(running.started_at.is_some());

        let done = store
            .update_subtask_step_status(step.id, "completed", Some("ok"), Some("payload"), None)
            .unwrap();
        assert_eq!(done.status, "completed");
        assert_eq!(done.result_summary.as_deref(), Some("ok"));
        assert_eq!(done.result_payload.as_deref(), Some("payload"));
        assert!(done.completed_at.is_some());

        let listed = store.list_subtask_steps(subtask.subtask_id).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, step.id);
    }

    #[test]
    fn file_write_proposal_resolution_and_audit_round_trip() {
        let (_dir, store) = new_test_store();
        let task = store.create_task("File proposals").unwrap();

        let subtask = store
            .create_subtask(&NewSubTaskInput {
                task_id: task.id,
                title: "writer".to_string(),
                description: "propose file".to_string(),
                execution_type: "draft_generation".to_string(),
                instruction_source: "system".to_string(),
                parent_context_snapshot: "{}".to_string(),
            })
            .unwrap();

        let step = store
            .create_subtask_step(
                subtask.subtask_id,
                0,
                "file_write_proposal",
                "path:out.md|intent:test",
            )
            .unwrap();

        let proposal = store
            .create_file_write_proposal(
                subtask.subtask_id,
                Some(step.id),
                task.id,
                "drafts/out.md",
                "# hello",
            )
            .unwrap();

        assert_eq!(proposal.status, "pending_approval");
        let pending = store.list_pending_file_write_proposals(task.id).unwrap();
        assert_eq!(pending.len(), 1);

        let resolved = store
            .resolve_file_write_proposal(proposal.id, "approved")
            .unwrap();
        assert_eq!(resolved.status, "approved");
        assert!(resolved.resolved_at.is_some());

        let audit = store
            .append_write_audit_entry(
                task.id,
                subtask.subtask_id,
                proposal.id,
                "approved",
                "drafts/out.md",
            )
            .unwrap();
        assert_eq!(audit.action, "approved");

        let entries = store.list_write_audit_log(task.id, 10).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, audit.id);
    }

    #[test]
    fn file_write_proposal_apply_state_machine_round_trip() {
        let (_dir, store) = new_test_store();
        let task = store.create_task("Apply state machine").unwrap();

        let subtask = store
            .create_subtask(&NewSubTaskInput {
                task_id: task.id,
                title: "writer".to_string(),
                description: "propose file".to_string(),
                execution_type: "draft_generation".to_string(),
                instruction_source: "system".to_string(),
                parent_context_snapshot: "{}".to_string(),
            })
            .unwrap();

        let step = store
            .create_subtask_step(
                subtask.subtask_id,
                0,
                "file_write_proposal",
                "path:out.md|intent:test",
            )
            .unwrap();

        let proposal = store
            .create_file_write_proposal(
                subtask.subtask_id,
                Some(step.id),
                task.id,
                "drafts/out.md",
                "# hello",
            )
            .unwrap();
        assert_eq!(proposal.status, "pending_approval");

        let applying = store.begin_file_write_proposal_apply(proposal.id).unwrap();
        assert_eq!(applying.status, "applying");
        assert!(applying.resolved_at.is_none());

        let pending_after_begin = store.list_pending_file_write_proposals(task.id).unwrap();
        assert_eq!(
            pending_after_begin.len(),
            0,
            "applying proposals must not remain in pending queue"
        );

        let approved = store
            .complete_file_write_proposal_apply(proposal.id)
            .unwrap();
        assert_eq!(approved.status, "approved");
        assert!(approved.resolved_at.is_some());
    }

    #[test]
    fn file_write_proposal_apply_rollback_restores_pending_state() {
        let (_dir, store) = new_test_store();
        let task = store.create_task("Apply rollback").unwrap();

        let subtask = store
            .create_subtask(&NewSubTaskInput {
                task_id: task.id,
                title: "writer".to_string(),
                description: "propose file".to_string(),
                execution_type: "draft_generation".to_string(),
                instruction_source: "system".to_string(),
                parent_context_snapshot: "{}".to_string(),
            })
            .unwrap();

        let step = store
            .create_subtask_step(
                subtask.subtask_id,
                0,
                "file_write_proposal",
                "path:out.md|intent:test",
            )
            .unwrap();

        let proposal = store
            .create_file_write_proposal(
                subtask.subtask_id,
                Some(step.id),
                task.id,
                "drafts/out.md",
                "# hello",
            )
            .unwrap();

        store.begin_file_write_proposal_apply(proposal.id).unwrap();
        let rolled_back = store
            .rollback_file_write_proposal_apply(proposal.id)
            .unwrap();
        assert_eq!(rolled_back.status, "pending_approval");
        assert!(rolled_back.resolved_at.is_none());

        let pending = store.list_pending_file_write_proposals(task.id).unwrap();
        assert_eq!(
            pending.len(),
            1,
            "rolled-back proposal should return to pending queue"
        );
    }

    #[test]
    fn file_write_proposal_apply_requires_expected_status() {
        let (_dir, store) = new_test_store();
        let task = store.create_task("Apply status guard").unwrap();

        let subtask = store
            .create_subtask(&NewSubTaskInput {
                task_id: task.id,
                title: "writer".to_string(),
                description: "propose file".to_string(),
                execution_type: "draft_generation".to_string(),
                instruction_source: "system".to_string(),
                parent_context_snapshot: "{}".to_string(),
            })
            .unwrap();

        let step = store
            .create_subtask_step(
                subtask.subtask_id,
                0,
                "file_write_proposal",
                "path:out.md|intent:test",
            )
            .unwrap();

        let proposal = store
            .create_file_write_proposal(
                subtask.subtask_id,
                Some(step.id),
                task.id,
                "drafts/out.md",
                "# hello",
            )
            .unwrap();

        let err = store
            .complete_file_write_proposal_apply(proposal.id)
            .expect_err("completing apply should fail before begin");
        assert!(
            err.to_string().contains("expected status 'applying'"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn store_cold_open_is_fast() {
        use crate::latency::STARTUP_BUDGET_MS;
        use std::time::Instant;
        use tempfile::TempDir;

        let dir = TempDir::new().expect("failed to create temp dir");
        let started = Instant::now();
        let _store = TaskStore::initialize(dir.path()).expect("failed to initialize store");
        let elapsed_ms = started.elapsed().as_millis() as u64;

        assert!(
            elapsed_ms < STARTUP_BUDGET_MS,
            "store cold-open exceeded startup budget: {}ms >= {}ms",
            elapsed_ms,
            STARTUP_BUDGET_MS
        );
    }

    #[test]
    fn session_settings_round_trip() {
        let (_dir, store) = new_test_store();

        // all default to false / not set
        assert!(!store.get_launch_at_login().unwrap());
        assert!(!store.get_quiet_mode().unwrap());
        assert!(!store.get_overlay_expanded().unwrap());
        assert!(!store.get_session_restored_at().unwrap());

        // launch_at_login round-trip
        store.set_launch_at_login(true).unwrap();
        assert!(store.get_launch_at_login().unwrap());
        store.set_launch_at_login(false).unwrap();
        assert!(!store.get_launch_at_login().unwrap());

        // quiet_mode round-trip
        store.set_quiet_mode(true).unwrap();
        assert!(store.get_quiet_mode().unwrap());
        store.set_quiet_mode(false).unwrap();
        assert!(!store.get_quiet_mode().unwrap());

        // overlay_expanded round-trip
        store.set_overlay_expanded(true).unwrap();
        assert!(store.get_overlay_expanded().unwrap());
        store.set_overlay_expanded(false).unwrap();
        assert!(!store.get_overlay_expanded().unwrap());

        // session_restored_at: absent = false, mark = true
        store.mark_session_restored().unwrap();
        assert!(store.get_session_restored_at().unwrap());
    }

    #[test]
    fn privacy_settings_round_trip() {
        let (_dir, store) = new_test_store();

        assert!(store.get_privacy_workspace_watcher_enabled().unwrap());
        assert!(store.get_privacy_clipboard_capture_enabled().unwrap());
        assert!(store.get_privacy_active_window_context_enabled().unwrap());
        assert!(store.get_privacy_proactive_triggers_enabled().unwrap());
        assert!(!store.get_privacy_user_profile_memory_enabled().unwrap());
        assert!(!store.get_privacy_calendar_context_enabled().unwrap());
        assert!(store.get_privacy_selection_capture_enabled().unwrap());
        assert!(store.get_privacy_typing_activity_enabled().unwrap());
        assert_eq!(
            store.get_tts_voice().unwrap(),
            crate::voice_naturalness::DEFAULT_TTS_VOICE
        );

        store.set_privacy_workspace_watcher_enabled(false).unwrap();
        store.set_privacy_clipboard_capture_enabled(false).unwrap();
        store
            .set_privacy_active_window_context_enabled(false)
            .unwrap();
        store.set_privacy_proactive_triggers_enabled(false).unwrap();
        store.set_privacy_user_profile_memory_enabled(true).unwrap();
        store.set_privacy_calendar_context_enabled(true).unwrap();
        store.set_privacy_selection_capture_enabled(false).unwrap();
        store.set_privacy_typing_activity_enabled(false).unwrap();
        assert_eq!(store.set_tts_voice("nova").unwrap(), "nova");

        assert!(!store.get_privacy_workspace_watcher_enabled().unwrap());
        assert!(!store.get_privacy_clipboard_capture_enabled().unwrap());
        assert!(!store.get_privacy_active_window_context_enabled().unwrap());
        assert!(!store.get_privacy_proactive_triggers_enabled().unwrap());
        assert!(store.get_privacy_user_profile_memory_enabled().unwrap());
        assert!(store.get_privacy_calendar_context_enabled().unwrap());
        assert!(!store.get_privacy_selection_capture_enabled().unwrap());
        assert!(!store.get_privacy_typing_activity_enabled().unwrap());
        assert_eq!(store.get_tts_voice().unwrap(), "nova");
        assert_eq!(
            store.set_tts_voice("invalid-voice").unwrap(),
            crate::voice_naturalness::DEFAULT_TTS_VOICE
        );
    }

    #[test]
    fn clear_task_data_keeps_task_and_removes_task_content() {
        let (_dir, store) = new_test_store();
        let task = store.create_task("Privacy Clear").unwrap();

        store
            .append_chat_message(
                task.id,
                "user",
                "text",
                MessageKind::UserStatement,
                "keep this private",
            )
            .unwrap();
        store
            .set_watched_folder(task.id, "/tmp/privacy-clear")
            .unwrap();
        store
            .upsert_file_registry_entry(task.id, "/tmp/privacy-clear/a.md", None, "v1")
            .unwrap();
        store.set_clipboard_capture(task.id, true).unwrap();
        store
            .append_recently_learned(task.id, "file", "a.md", "preview")
            .unwrap();
        store.record_task_focus(task.id).unwrap();
        store
            .record_proactive_trigger(task.id, "resume", false)
            .unwrap();
        store
            .log_synthesis_decision(
                Some(task.id),
                "suppressed",
                Some("no_reason"),
                0.1,
                "idle",
                None,
                false,
            )
            .unwrap();

        let task_workspace = PathBuf::from(&task.workspace_path);
        fs::write(task_workspace.join("scratch.txt"), "private").unwrap();

        store.clear_task_data(task.id).unwrap();

        assert_eq!(store.list_tasks().unwrap().len(), 1);
        assert!(store.get_active_task().unwrap().is_some());
        assert!(store.list_chat_messages(task.id).unwrap().is_empty());
        assert!(store.get_watched_folder(task.id).unwrap().is_none());
        assert_eq!(store.count_watched_files(task.id).unwrap(), 0);
        assert!(!store.get_clipboard_capture(task.id).unwrap());
        assert!(store.list_recently_learned(task.id, 10).unwrap().is_empty());
        assert!(store
            .list_proactive_trigger_audit_log(task.id, 10)
            .unwrap()
            .is_empty());
        assert!(store.list_synthesis_log(task.id, 10).unwrap().is_empty());
        assert!(!task_workspace.join("scratch.txt").exists());
        assert!(task_workspace.exists());
    }

    #[test]
    fn clear_all_data_resets_database_and_workspace_root() {
        let (_dir, store) = new_test_store();
        let task = store.create_task("Reset All").unwrap();
        store.set_onboarding_complete(true).unwrap();
        store.set_privacy_workspace_watcher_enabled(false).unwrap();
        fs::write(
            PathBuf::from(&task.workspace_path).join("scratch.txt"),
            "private",
        )
        .unwrap();

        store.clear_all_data().unwrap();

        assert!(store.list_tasks().unwrap().is_empty());
        assert!(!store.get_onboarding_complete().unwrap());
        assert!(store.get_privacy_workspace_watcher_enabled().unwrap());
        assert!(store.paths.workspace_root.exists());
        assert!(!PathBuf::from(task.workspace_path).exists());
    }
}
