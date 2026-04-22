#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
WATCHER_RS="$ROOT_DIR/desktop/src-tauri/src/watcher.rs"
RETRIEVAL_RS="$ROOT_DIR/desktop/src-tauri/src/retrieval.rs"
STORE_RS="$ROOT_DIR/desktop/src-tauri/src/store.rs"
MODELS_RS="$ROOT_DIR/desktop/src-tauri/src/models.rs"
COMMANDS_RS="$ROOT_DIR/desktop/src-tauri/src/commands.rs"
MAIN_RS="$ROOT_DIR/desktop/src-tauri/src/main.rs"
STATE_RS="$ROOT_DIR/desktop/src-tauri/src/state.rs"
TAURI_CLIENT_TS="$ROOT_DIR/desktop/src/tauriClient.ts"
APP_TSX="$ROOT_DIR/desktop/src/App.tsx"

echo "--- phase 13 workspace awareness check ---"

fail() { echo "FAIL: $1" >&2; exit 1; }
pass() { echo "PASS: $1"; }

# 1. schema tables exist (m13.1)
grep -q "watched_folders" "$STORE_RS" || fail "watched_folders table missing from store.rs"
grep -q "watched_file_registry" "$STORE_RS" || fail "watched_file_registry table missing from store.rs"
grep -q "clipboard_capture_settings" "$STORE_RS" || fail "clipboard_capture_settings table missing from store.rs"
grep -q "recently_learned_log" "$STORE_RS" || fail "recently_learned_log table missing from store.rs"
pass "all 4 phase 13 tables declared in initialize_schema"

# 2. new store methods present (m13.1)
grep -q "fn set_watched_folder" "$STORE_RS" || fail "set_watched_folder method missing"
grep -q "fn get_watched_folder" "$STORE_RS" || fail "get_watched_folder method missing"
grep -q "fn upsert_file_registry_entry" "$STORE_RS" || fail "upsert_file_registry_entry method missing"
grep -q "fn get_file_registry_entry" "$STORE_RS" || fail "get_file_registry_entry method missing"
grep -q "fn remove_file_registry_entry" "$STORE_RS" || fail "remove_file_registry_entry method missing"
grep -q "fn set_clipboard_capture" "$STORE_RS" || fail "set_clipboard_capture method missing"
grep -q "fn get_clipboard_capture" "$STORE_RS" || fail "get_clipboard_capture method missing"
grep -q "fn append_recently_learned" "$STORE_RS" || fail "append_recently_learned method missing"
grep -q "fn list_recently_learned" "$STORE_RS" || fail "list_recently_learned method missing"
pass "all store methods for phase 13 present"

# 3. new DTOs declared (m13.1)
grep -q "WatchedFolderDto" "$MODELS_RS" || fail "WatchedFolderDto missing from models.rs"
grep -q "WatchedFileRegistryEntry" "$MODELS_RS" || fail "WatchedFileRegistryEntry missing from models.rs"
grep -q "RecentlyLearnedItemDto" "$MODELS_RS" || fail "RecentlyLearnedItemDto missing from models.rs"
grep -q "WatcherStatusDto" "$MODELS_RS" || fail "WatcherStatusDto missing from models.rs"
pass "all phase 13 DTOs declared in models.rs"

# 4. filesystem watcher module exists (m13.2)
test -f "$WATCHER_RS" || fail "watcher.rs module file missing"
grep -q "WatcherState" "$WATCHER_RS" || fail "WatcherState not in watcher.rs"
grep -q "fn start_watcher" "$WATCHER_RS" || fail "start_watcher not in watcher.rs"
grep -q "fn stop_watcher" "$WATCHER_RS" || fail "stop_watcher not in watcher.rs"
grep -q "fn should_ignore_file" "$WATCHER_RS" || fail "should_ignore_file not in watcher.rs"
grep -q "DEBOUNCE_MS" "$WATCHER_RS" || fail "DEBOUNCE_MS debounce constant missing"
grep -q "fn stop_all_except" "$WATCHER_RS" || fail "stop_all_except lifecycle helper missing"
pass "filesystem watcher module present with required symbols (m13.2)"

# 5. debounce is implemented (m13.2)
grep -q "tokio::time::interval" "$WATCHER_RS" || fail "debounce interval timer not found in watcher.rs"
grep -q "pending" "$WATCHER_RS" || fail "pending debounce map not found in watcher.rs"
pass "debounce pattern implemented in watcher.rs"

# 6. idempotent auto-ingest in retrieval (m13.2)
grep -q "fn auto_ingest_file_for_task" "$RETRIEVAL_RS" || fail "auto_ingest_file_for_task missing from retrieval.rs"
grep -q "get_file_registry_entry" "$RETRIEVAL_RS" || fail "registry check missing in auto_ingest_file_for_task"
grep -q "replace_artifact_chunks" "$RETRIEVAL_RS" || fail "chunk replacement missing in auto_ingest_file_for_task"
grep -q "upsert_file_registry_entry" "$RETRIEVAL_RS" || fail "registry upsert missing in auto_ingest_file_for_task"
grep -q "as_nanos" "$RETRIEVAL_RS" || fail "high-resolution mtime not used in auto_ingest_file_for_task"
pass "idempotent auto-ingest pipeline present in retrieval.rs (m13.2)"

# 7. clipboard capture (m13.3)
grep -q "fn start_clipboard_poll" "$WATCHER_RS" || fail "start_clipboard_poll missing from watcher.rs"
grep -q "fn stop_clipboard_poll" "$WATCHER_RS" || fail "stop_clipboard_poll missing from watcher.rs"
grep -q "CLIPBOARD_POLL_MS" "$WATCHER_RS" || fail "CLIPBOARD_POLL_MS constant missing"
grep -q "arboard" "$WATCHER_RS" || fail "arboard clipboard library not referenced in watcher.rs"
grep -q "fn ingest_clipboard_snippet" "$WATCHER_RS" || fail "ingest_clipboard_snippet missing from watcher.rs"
grep -q "clipboard_polls" "$WATCHER_RS" || fail "clipboard poll ownership map missing"
pass "clipboard capture implementation present (m13.3)"

# 8. clipboard default is off
grep -q "DEFAULT 0" "$STORE_RS" || fail "clipboard_capture_settings DEFAULT 0 not found in schema"
pass "clipboard capture defaults to 0 (off) in schema"

# 9. ignore rules cover required cases
grep -q "IGNORED_DIRS" "$WATCHER_RS" || fail "IGNORED_DIRS constant missing"
grep -q '"artifacts"' "$WATCHER_RS" || fail "artifacts dir not in IGNORED_DIRS"
grep -q "MAX_FILE_BYTES" "$WATCHER_RS" || fail "MAX_FILE_BYTES size limit missing"
grep -q "supported_artifact_type" "$WATCHER_RS" || fail "extension check via supported_artifact_type missing"
grep -q "EventKind::Remove" "$WATCHER_RS" || fail "remove-event handling missing from watcher"
grep -q "ModifyKind::Name" "$WATCHER_RS" || fail "rename-event handling missing from watcher"
pass "ignore rules cover dirs, size limit, and extension blocklist"

# 10. watcher state on JeffState (m13.2)
grep -q "watcher: Arc<Mutex<WatcherState>>" "$STATE_RS" || fail "watcher field missing from JeffState"
pass "WatcherState added to JeffState"

# 11. commands registered (m13.4)
grep -q "fn start_workspace_watcher" "$COMMANDS_RS" || fail "start_workspace_watcher command missing"
grep -q "fn stop_workspace_watcher" "$COMMANDS_RS" || fail "stop_workspace_watcher command missing"
grep -q "fn get_watcher_status" "$COMMANDS_RS" || fail "get_watcher_status command missing"
grep -q "fn list_recently_learned" "$COMMANDS_RS" || fail "list_recently_learned command missing"
grep -q "fn set_clipboard_capture" "$COMMANDS_RS" || fail "set_clipboard_capture command missing"
grep -q "fn get_clipboard_capture_setting" "$COMMANDS_RS" || fail "get_clipboard_capture_setting command missing"
grep -q "ensure_workspace_awareness_for_task" "$COMMANDS_RS" || fail "active-task watcher sync helper missing"
grep -q "start_watcher_and_persist_folder" "$COMMANDS_RS" || fail "watcher start/persist ordering helper missing"
pass "all 6 phase 13 commands defined in commands.rs"

grep -q "start_workspace_watcher" "$MAIN_RS" || fail "start_workspace_watcher not registered in main.rs"
grep -q "stop_workspace_watcher" "$MAIN_RS" || fail "stop_workspace_watcher not registered in main.rs"
grep -q "list_recently_learned" "$MAIN_RS" || fail "list_recently_learned not registered in main.rs"
grep -q "set_clipboard_capture" "$MAIN_RS" || fail "set_clipboard_capture not registered in main.rs"
grep -q "restore_workspace_awareness_for_active_task" "$MAIN_RS" || fail "startup watcher restore missing in main.rs"
pass "all phase 13 commands registered in invoke_handler"

# 12. frontend types and wrappers (m13.4)
grep -q "WatcherStatusDto" "$TAURI_CLIENT_TS" || fail "WatcherStatusDto missing from tauriClient.ts"
grep -q "RecentlyLearnedItemDto" "$TAURI_CLIENT_TS" || fail "RecentlyLearnedItemDto missing from tauriClient.ts"
grep -q "startWorkspaceWatcher" "$TAURI_CLIENT_TS" || fail "startWorkspaceWatcher wrapper missing"
grep -q "listRecentlyLearned" "$TAURI_CLIENT_TS" || fail "listRecentlyLearned wrapper missing"
grep -q "setClipboardCapture" "$TAURI_CLIENT_TS" || fail "setClipboardCapture wrapper missing"
grep -q "getClipboardCaptureSetting" "$TAURI_CLIENT_TS" || fail "getClipboardCaptureSetting wrapper missing"
pass "frontend TypeScript types and wrappers present"

# 13. recently learned UI in companion view (m13.4)
grep -q "recently-learned-panel" "$APP_TSX" || fail "recently-learned-panel section missing from App.tsx"
grep -q "recently-learned-list" "$APP_TSX" || fail "recently-learned-list missing from App.tsx"
grep -q "clipboard-capture-toggle" "$APP_TSX" || fail "clipboard-capture-toggle UI missing from App.tsx"
grep -q "recently-learned-toggle" "$APP_TSX" || fail "recently-learned-toggle button missing from App.tsx"
grep -q "refreshRecentlyLearned" "$APP_TSX" || fail "refreshRecentlyLearned not called in App.tsx"
pass "recently learned list UI in companion view (m13.4)"

# 14. clipboard capture never ingests when off — verify the enabled guard
grep -q "if !enabled" "$WATCHER_RS" || fail "clipboard enabled guard missing in poll task"
pass "clipboard poll skips ingest when disabled"

# 14b. debounce behavior has a named unit test
grep -q "fn watcher_debounces_rapid_events" "$WATCHER_RS" || \
  fail "watcher_debounces_rapid_events test missing from watcher.rs"
pass "watcher debounce behavior test is present"

# 15. full build + test suite
cd "$ROOT_DIR/desktop"
npm run lint
npm run test
cargo build --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml watcher_debounces_rapid_events -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml watcher
cargo test --manifest-path src-tauri/Cargo.toml store::tests::watched_folder
cargo test --manifest-path src-tauri/Cargo.toml store::tests::file_registry
cargo test --manifest-path src-tauri/Cargo.toml store::tests::clipboard
cargo test --manifest-path src-tauri/Cargo.toml store::tests::recently_learned
cargo test --manifest-path src-tauri/Cargo.toml retrieval::tests::auto_ingest
pass "build and test suite passed"

echo ""
echo "phase 13 checks passed"
