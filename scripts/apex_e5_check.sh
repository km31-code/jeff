#!/usr/bin/env bash
# apex e5 check: Drive/Docs remote read -- on-demand ingestion with provenance,
# retrieval grounding, and purge-on-removal.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
DRIVE="$SRC/drive_core.rs"
STORE="$SRC/store.rs"
COMMANDS="$SRC/commands.rs"
MAIN="$SRC/main.rs"
APP_TSX="$DESKTOP/src/App.tsx"
TAURI_CLIENT="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex e5 drive/docs remote read check ---"

# 1. Module + capabilities.
test -f "$DRIVE" || fail "drive_core.rs missing"
grep -q "CREATE TABLE IF NOT EXISTS remote_ingested_docs" "$STORE" || fail "remote_ingested_docs table missing"
grep -q "pub fn ingest_remote_doc" "$DRIVE" || fail "on-demand ingestion missing"
grep -q "pub fn list_remote_docs" "$DRIVE" || fail "remote doc listing missing"
grep -q "pub fn remove_remote_doc" "$DRIVE" || fail "per-item removal missing"
grep -q "provenance" "$DRIVE" || fail "provenance tagging missing"
grep -q "insert_artifact_with_chunks" "$DRIVE" || fail "ingestion does not enter retrieval"
pass "drive core: ingest-with-provenance, list, remove present"

# 2. Behavioral: ingest grounds with provenance; removal purges chunks.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

for t in \
  e5_ingest_remote_doc_grounds_with_provenance \
  e5_removal_purges_ingested_chunks; do
  grep -q "fn $t" "$DRIVE" || fail "expected e5 test $t is missing"
done
E5_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test e5_ --quiet 2>&1)
echo "$E5_TEST_OUT" | grep -q "test result: ok" || { echo "$E5_TEST_OUT"; fail "e5 tests failed"; }
echo "$E5_TEST_OUT" | grep -q "FAILED" && { echo "$E5_TEST_OUT"; fail "e5 tests failed"; }
pass "e5 ingest-provenance + purge-on-removal tests pass"

# 3. Commands + Privacy Center surface with per-item removal.
for cmd in ingest_remote_doc list_remote_docs remove_remote_doc; do
  grep -q "pub fn $cmd" "$COMMANDS" || fail "$cmd command missing"
  grep -q "commands::$cmd" "$MAIN" || fail "$cmd not registered"
done
grep -q "listRemoteDocs" "$TAURI_CLIENT" || fail "frontend remote doc binding missing"
grep -q "privacy-surface-remote-docs" "$APP_TSX" || fail "Privacy Center remote docs surface missing"
grep -q "remote-doc-remove" "$APP_TSX" || fail "per-item removal control missing"
pass "commands and Privacy Center remote-docs surface (with removal) are wired"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend lint did not run"; }
pass "frontend TypeScript check passes"

FRONTEND_TEST_OUT=$(cd "$DESKTOP" && npm test -- --run 2>&1)
echo "$FRONTEND_TEST_OUT" | grep -qE "Test Files.*passed" || { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
echo "$FRONTEND_TEST_OUT" | grep -qE "[0-9]+ failed" && { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
pass "frontend tests pass"

bash "$ROOT_DIR/scripts/apex_e4_check.sh" >/dev/null 2>&1 || fail "apex e4 calendar gate regressed"
pass "apex e4 calendar gate still passes"

echo "--- apex e5 check passed ---"
