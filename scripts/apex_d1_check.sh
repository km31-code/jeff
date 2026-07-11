#!/usr/bin/env bash
# apex d1 check: action bus, unified receipts, undo cache.
# Verifies mutation routing through the bus, receipt/undo schema, Privacy Center
# audit visibility, and the existing file-write approval regression gate.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
ACTION_BUS="$SRC/action_bus.rs"
STORE="$SRC/store.rs"
MODELS="$SRC/models.rs"
COMMANDS="$SRC/commands.rs"
MAIN="$SRC/main.rs"
APP_TSX="$DESKTOP/src/App.tsx"
TAURI_CLIENT="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex d1 action bus check ---"

# 1. Bus taxonomy, request, adapter seam, receipt statuses, and undo policy.
test -f "$ACTION_BUS" || fail "action_bus.rs missing"
grep -q "pub enum ActionClass" "$ACTION_BUS" || fail "ActionClass taxonomy missing"
for class in "doc.insert" "doc.replace" "doc.suggest" "file.write" "file.delete" "email.draft" "email.send" "calendar.propose" "system.open" "tool.custom."; do
  grep -q "$class" "$ACTION_BUS" || fail "ActionClass missing $class"
done
grep -q "pub struct ActionRequest" "$ACTION_BUS" || fail "ActionRequest missing"
grep -q "pub trait ActionAdapter" "$ACTION_BUS" || fail "ActionAdapter trait missing"
grep -q "UNDO_RETENTION_DAYS: u64 = 30" "$ACTION_BUS" || fail "30-day undo retention constant missing"
grep -q "snapshot_for_file_write" "$ACTION_BUS" || fail "pre-mutation snapshot missing"
grep -q "restore_file_write_snapshot" "$ACTION_BUS" || fail "revert restore path missing"
pass "action bus taxonomy, adapter seam, and undo cache are present"

# 2. Unified receipts schema, DTOs, commands, and dashboard surface.
grep -q "CREATE TABLE IF NOT EXISTS action_receipts" "$STORE" || fail "action_receipts table missing"
for column in "class TEXT" "surface TEXT" "level TEXT" "payload_excerpt TEXT" "undo_ref TEXT"; do
  grep -q "$column" "$STORE" || fail "action_receipts missing $column"
done
grep -q "create_action_receipt" "$STORE" || fail "create_action_receipt store method missing"
grep -q "update_action_receipt_status" "$STORE" || fail "update_action_receipt_status store method missing"
grep -q "list_action_receipts" "$STORE" || fail "list_action_receipts store method missing"
grep -q "pub struct ActionReceiptDto" "$MODELS" || fail "ActionReceiptDto missing"
grep -q "pub action_receipts: Vec<ActionReceiptDto>" "$MODELS" || fail "Privacy Center dashboard lacks action_receipts"
grep -q "pub fn list_action_receipts" "$COMMANDS" || fail "list_action_receipts command missing"
grep -q "pub fn revert_action_receipt" "$COMMANDS" || fail "revert_action_receipt command missing"
grep -q "commands::list_action_receipts" "$MAIN" || fail "list_action_receipts not registered"
grep -q "commands::revert_action_receipt" "$MAIN" || fail "revert_action_receipt not registered"
pass "unified receipts schema, commands, and dashboard DTO are wired"

# 3. Existing file-write approval routes through the bus, not direct writes.
APPROVE_START=$(grep -n "pub fn approve_subtask_file_write" "$COMMANDS" | head -1 | cut -d: -f1)
APPROVE_END=$(grep -n "pub fn reject_subtask_file_write" "$COMMANDS" | head -1 | cut -d: -f1)
test -n "$APPROVE_START" && test -n "$APPROVE_END" || fail "approve/reject section not found"
APPROVE_SECTION=$(sed -n "${APPROVE_START},${APPROVE_END}p" "$COMMANDS")
echo "$APPROVE_SECTION" | grep -q "begin_file_write_proposal_apply" || fail "approve path no longer begins proposal apply"
echo "$APPROVE_SECTION" | grep -q "FileWriteAdapter::execute_file_write" || fail "file write approval does not route through action bus"
echo "$APPROVE_SECTION" | grep -q "complete_file_write_proposal_apply" || fail "approve path no longer completes proposal apply"
echo "$APPROVE_SECTION" | grep -q "rollback_file_write_proposal_apply" || fail "approve path no longer rolls back on bus failure"
if echo "$APPROVE_SECTION" | grep -q "fs::write"; then
  fail "approve path still writes directly instead of through action bus"
fi
grep -q "record_rejected_action" "$COMMANDS" || fail "reject path does not create unified receipt"
pass "file-write approval and rejection flow through the action bus"

# 3b. Global mutation gate: no production filesystem mutation outside the action
# bus adapter and the explicitly allowlisted infrastructure modules. This is the
# codebase-wide enforcement of the "every mutation flows through one spine"
# invariant -- a future direct fs write added to any command/agent/action module
# fails this gate. Allowlist owns legitimate non-action i/o: the bus adapter and
# undo cache (action_bus), model downloads (local_runtime), db/workspace/undo
# management (store), the revision version store (revision), the transient
# clipboard snippet (watcher), and the native scripting adapter (native_docs).
MUTATION_ALLOWLIST="action_bus.rs local_runtime.rs store.rs revision.rs watcher.rs native_docs.rs"
MUTATION_RE='fs::write\(|fs::remove_(file|dir)|fs::rename\(|File::create\('
for src_file in "$SRC"/*.rs; do
  base=$(basename "$src_file")
  case " $MUTATION_ALLOWLIST " in *" $base "*) continue;; esac
  # only inspect production code -- everything before the unit test module.
  test_line=$( { grep -n "^mod tests" "$src_file" || true; } | head -1 | cut -d: -f1)
  if [ -z "$test_line" ]; then test_line=999999; fi
  prod_hits=$(awk -v tl="$test_line" 'NR<tl' "$src_file" | grep -nE "$MUTATION_RE" || true)
  if [ -n "$prod_hits" ]; then
    echo "$base:"; echo "$prod_hits"
    fail "direct filesystem mutation outside the action bus in $base (route it through action_bus)"
  fi
done
pass "no production filesystem mutation outside the action bus and allowlisted infra"

# 4. Live edit bridge creates and updates unified receipts.
grep -q "action_receipt_id" "$STORE" || fail "live_edit_receipts not linked to action receipts"
grep -q "doc.replace" "$STORE" || fail "live edit requests do not create doc.replace action receipts"
grep -q "update_action_receipt_status(action_receipt_id" "$STORE" || fail "live edit status does not update action receipt"
pass "legacy live-edit bridge is wrapped by unified receipts"

# 5. Frontend can inspect/revert receipts.
grep -q "interface ActionReceiptDto" "$TAURI_CLIENT" || fail "frontend ActionReceiptDto missing"
grep -q "listActionReceipts" "$TAURI_CLIENT" || fail "frontend listActionReceipts binding missing"
grep -q "revertActionReceipt" "$TAURI_CLIENT" || fail "frontend revertActionReceipt binding missing"
grep -q "privacy-surface-action-receipts" "$APP_TSX" || fail "Privacy Center action receipt surface missing"
grep -q "action-receipt-revert" "$APP_TSX" || fail "Privacy Center revert control missing"
pass "Privacy Center unified action audit is visible and reversible"

# 6. Compile/tests/regressions.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

D1_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test d1_ --quiet 2>&1)
echo "$D1_TEST_OUT" | grep -q "test result: ok" || { echo "$D1_TEST_OUT"; fail "d1 tests failed"; }
echo "$D1_TEST_OUT" | grep -q "FAILED" && { echo "$D1_TEST_OUT"; fail "d1 tests failed"; }
pass "d1 taxonomy and undo/revert tests pass"

bash "$ROOT_DIR/scripts/phase16_check.sh" >/dev/null 2>&1 || fail "phase16 file-write regression gate failed"
pass "phase16 file-write approval regression still passes"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend TypeScript check did not run"; }
pass "frontend TypeScript check passes"

echo "--- apex d1 check passed ---"
