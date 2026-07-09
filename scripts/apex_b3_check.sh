#!/usr/bin/env bash
# apex b3 check: typed episodic memory.
# Verifies the local episodic store, asynchronous writers, lull/session capture,
# proposal-outcome capture, debug commands, privacy clear paths, and deterministic
# scripted-session behavior. No external API calls or model downloads required.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
MEMORY_RS="$SRC/memory.rs"
STORE_RS="$SRC/store.rs"
MODELS_RS="$SRC/models.rs"
COMMANDS_RS="$SRC/commands.rs"
MAIN_RS="$SRC/main.rs"
LIB_RS="$SRC/lib.rs"
TAURI_CLIENT_TS="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex b3 typed episodic memory check ---"

# 1. module + typed episode kinds.
test -f "$MEMORY_RS" || fail "memory.rs missing"
grep -q "pub const KIND_SESSION_SUMMARY" "$MEMORY_RS" || fail "session_summary kind missing"
grep -q "pub const KIND_DECISION" "$MEMORY_RS" || fail "decision kind missing"
grep -q "pub const KIND_PROPOSAL_OUTCOME" "$MEMORY_RS" || fail "proposal_outcome kind missing"
grep -q "pub const KIND_WORK_UNDERSTANDING" "$MEMORY_RS" || fail "work_understanding kind missing"
grep -q "pub const KIND_DEADLINE_MENTION" "$MEMORY_RS" || fail "deadline_mention kind missing"
grep -q "pub const KIND_USER_FACT" "$MEMORY_RS" || fail "user_fact kind missing"
grep -q "pub struct NewEpisode" "$MEMORY_RS" || fail "NewEpisode input type missing"
grep -q "pub fn record_episode" "$MEMORY_RS" || fail "record_episode missing"
grep -q "pub fn search_episodes" "$MEMORY_RS" || fail "search_episodes missing"
grep -q "cosine_similarity" "$MEMORY_RS" || fail "episode search does not use cosine similarity"
grep -q "encode_embedding" "$MEMORY_RS" || fail "embedding blob encoder missing"
grep -q "decode_embedding" "$MEMORY_RS" || fail "embedding blob decoder missing"
pass "typed episodic memory module and all required kinds present"

# 2. schema + privacy clear paths.
grep -q "CREATE TABLE IF NOT EXISTS episodes" "$STORE_RS" || fail "episodes table missing"
grep -q "embedding BLOB NOT NULL" "$STORE_RS" || fail "episode embedding blob missing"
grep -q "salience REAL NOT NULL" "$STORE_RS" || fail "episode salience missing"
grep -q "consolidated_at TEXT" "$STORE_RS" || fail "episode consolidated_at marker missing"
grep -q "idx_episodes_task_kind_created" "$STORE_RS" || fail "episode task/kind index missing"
grep -q "DELETE FROM episodes WHERE task_id" "$STORE_RS" || fail "per-task clear does not delete episodes"
grep -q "DELETE FROM episodes" "$STORE_RS" || fail "global clear does not delete episodes"
grep -q "pub struct EpisodeDto" "$MODELS_RS" || fail "EpisodeDto missing"
grep -q "pub struct EpisodeSearchResultDto" "$MODELS_RS" || fail "EpisodeSearchResultDto missing"
pass "episodes schema, DTOs, and clear paths present"

# 3. background capture is privacy-gated and off the response path.
grep -q "mod memory;" "$MAIN_RS" || fail "memory module not registered in binary"
grep -q "pub mod memory;" "$LIB_RS" || fail "memory module not exported for tests"
grep -q "record_episode_async" "$MEMORY_RS" || fail "async episode writer missing"
grep -q "thread::spawn" "$MEMORY_RS" || fail "episode writer is not backgrounded"
grep -q "spawn_goal_extraction_poll" "$MAIN_RS" || fail "lull poll missing"
grep -q "extract_memory_tags_with_fallback" "$MAIN_RS" || fail "memory tag capture not wired to lull poll"
grep -q "record_memory_tags_for_turn" "$MAIN_RS" || fail "memory tag writer not wired"
grep -q "spawn_memory_session_summary_poll" "$MAIN_RS" || fail "idle session summary poll missing"
grep -q "record_idle_session_summary_if_due" "$MAIN_RS" || fail "idle session summary writer not wired"
grep -q "get_privacy_user_profile_memory_enabled" "$MAIN_RS" || fail "memory background work missing privacy gate"
grep -q "spawn_blocking" "$MAIN_RS" || fail "blocking memory work is not offloaded"
pass "decision/deadline/fact and idle summary capture are privacy-gated and off-path"

# 4. proposal outcome writers across user decision surfaces.
grep -q "record_proposal_memory_outcome" "$COMMANDS_RS" || fail "proposal outcome helper missing"
grep -q "revision:.*accepted" "$COMMANDS_RS" || fail "revision acceptance outcome missing"
grep -q "revision:.*rejected" "$COMMANDS_RS" || fail "revision rejection outcome missing"
grep -q "subtask:.*accepted" "$COMMANDS_RS" || fail "subtask acceptance outcome missing"
grep -q "subtask:.*rejected" "$COMMANDS_RS" || fail "subtask rejection outcome missing"
grep -q "suggestion:.*accepted" "$COMMANDS_RS" || fail "suggestion acceptance outcome missing"
grep -q "suggestion:.*dismissed" "$COMMANDS_RS" || fail "suggestion dismissal outcome missing"
grep -q "file_write:.*approved" "$COMMANDS_RS" || fail "file write approval outcome missing"
grep -q "file_write:.*rejected" "$COMMANDS_RS" || fail "file write rejection outcome missing"
grep -q "clear_all_episodes" "$COMMANDS_RS" || fail "profile-memory clear does not clear episodes"
pass "proposal outcomes recorded for revisions, suggestions, subtasks, and file writes"

# 5. debug/API surface for inspection.
grep -q "pub fn list_episodes" "$COMMANDS_RS" || fail "list_episodes command missing"
grep -q "pub fn search_episodes" "$COMMANDS_RS" || fail "search_episodes command missing"
grep -q "commands::list_episodes" "$MAIN_RS" || fail "list_episodes command not registered"
grep -q "commands::search_episodes" "$MAIN_RS" || fail "search_episodes command not registered"
grep -q "export interface EpisodeDto" "$TAURI_CLIENT_TS" || fail "EpisodeDto frontend type missing"
grep -q "export async function listEpisodes" "$TAURI_CLIENT_TS" || fail "listEpisodes frontend binding missing"
grep -q "export async function searchEpisodes" "$TAURI_CLIENT_TS" || fail "searchEpisodes frontend binding missing"
pass "debug list/search commands and frontend bindings wired"

# 6. behavioral: clean compile, b3 tests, frontend typecheck.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

B3_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test b3_ --quiet 2>&1)
echo "$B3_TEST_OUT" | grep -q "test result: ok" || { echo "$B3_TEST_OUT"; fail "b3 tests failed"; }
echo "$B3_TEST_OUT" | grep -q "FAILED" && { echo "$B3_TEST_OUT"; fail "b3 tests failed"; }
grep -q "b3_scripted_working_session_records_typed_episodes" "$MEMORY_RS" \
  || fail "scripted-session test missing"
pass "b3 scripted-session, search, blob, and heuristic tests pass"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend TypeScript check did not run"; }
pass "frontend TypeScript check passes"

echo "--- apex b3 check passed ---"
