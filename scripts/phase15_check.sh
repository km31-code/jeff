#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
PROACTIVE_RS="$ROOT_DIR/desktop/src-tauri/src/proactive.rs"
STORE_RS="$ROOT_DIR/desktop/src-tauri/src/store.rs"
MODELS_RS="$ROOT_DIR/desktop/src-tauri/src/models.rs"
COMMANDS_RS="$ROOT_DIR/desktop/src-tauri/src/commands.rs"
MAIN_RS="$ROOT_DIR/desktop/src-tauri/src/main.rs"
TAURI_CLIENT_TS="$ROOT_DIR/desktop/src/tauriClient.ts"
APP_TSX="$ROOT_DIR/desktop/src/App.tsx"

echo "--- phase 15 proactive initiation check ---"

fail() { echo "FAIL: $1" >&2; exit 1; }
pass() { echo "PASS: $1"; }

# 1. proactive.rs exists with the three required evaluator symbols (m15.2)
test -f "$PROACTIVE_RS" || fail "proactive.rs missing"
grep -q "fn generate_reorientation" "$PROACTIVE_RS" || fail "generate_reorientation missing from proactive.rs"
grep -q "fn evaluate_drift" "$PROACTIVE_RS" || fail "evaluate_drift missing from proactive.rs"
grep -q "fn propose_speculative_subtask" "$PROACTIVE_RS" || fail "propose_speculative_subtask missing from proactive.rs"
pass "proactive.rs present with all three evaluator functions (m15.2)"

# 2. proactive_trigger_log and task_focus_log tables in store.rs (m15.1)
grep -q "proactive_trigger_log" "$STORE_RS" || fail "proactive_trigger_log table missing from store.rs"
grep -q "task_focus_log" "$STORE_RS" || fail "task_focus_log table missing from store.rs"
pass "proactive_trigger_log and task_focus_log tables present in store.rs (m15.1)"

# 3. all new store methods present (m15.1)
grep -q "fn record_task_focus" "$STORE_RS" || fail "record_task_focus missing from store.rs"
grep -q "fn get_last_task_focus" "$STORE_RS" || fail "get_last_task_focus missing from store.rs"
grep -q "fn record_proactive_trigger" "$STORE_RS" || fail "record_proactive_trigger missing from store.rs"
grep -q "fn get_last_proactive_trigger" "$STORE_RS" || fail "get_last_proactive_trigger missing from store.rs"
pass "all four new store methods present (m15.1)"

# 4. new DTOs present in models.rs (m15.1)
grep -q "ReorientationDto" "$MODELS_RS" || fail "ReorientationDto missing from models.rs"
grep -q "DriftFlagDto" "$MODELS_RS" || fail "DriftFlagDto missing from models.rs"
grep -q "ProactiveTriggerDto" "$MODELS_RS" || fail "ProactiveTriggerDto missing from models.rs"
pass "all three new phase 15 DTOs declared in models.rs (m15.1)"

# 5. all five new commands in commands.rs and registered in main.rs (m15.3)
grep -q "fn trigger_task_resume" "$COMMANDS_RS" || fail "trigger_task_resume missing from commands.rs"
grep -q "fn check_task_drift" "$COMMANDS_RS" || fail "check_task_drift missing from commands.rs"
grep -q "fn trigger_speculative_subtask" "$COMMANDS_RS" || fail "trigger_speculative_subtask missing from commands.rs"
grep -q "fn dismiss_proactive_trigger" "$COMMANDS_RS" || fail "dismiss_proactive_trigger missing from commands.rs"
grep -q "fn record_task_focus" "$COMMANDS_RS" || fail "record_task_focus command missing from commands.rs"
pass "all five phase 15 commands declared in commands.rs (m15.3)"

grep -q "commands::trigger_task_resume" "$MAIN_RS" || fail "trigger_task_resume not registered in main.rs"
grep -q "commands::check_task_drift" "$MAIN_RS" || fail "check_task_drift not registered in main.rs"
grep -q "commands::trigger_speculative_subtask" "$MAIN_RS" || fail "trigger_speculative_subtask not registered in main.rs"
grep -q "commands::dismiss_proactive_trigger" "$MAIN_RS" || fail "dismiss_proactive_trigger not registered in main.rs"
grep -q "commands::record_task_focus" "$MAIN_RS" || fail "record_task_focus not registered in main.rs"
pass "all five phase 15 commands registered in main.rs invoke_handler (m15.3)"

# 6. throttling: proactive_trigger_log query present in proactive.rs (m15.2)
grep -q "get_last_proactive_trigger" "$PROACTIVE_RS" || fail "cooldown check (get_last_proactive_trigger) missing from proactive.rs"
pass "per-trigger cooldown enforcement present in proactive.rs (m15.2)"

# 7. cooldown constants documented in proactive.rs (m15.2)
grep -q "REORIENTATION_COOLDOWN_SECONDS" "$PROACTIVE_RS" || fail "REORIENTATION_COOLDOWN_SECONDS constant missing from proactive.rs"
grep -q "DRIFT_COOLDOWN_SECONDS" "$PROACTIVE_RS" || fail "DRIFT_COOLDOWN_SECONDS constant missing from proactive.rs"
grep -q "STUCK_COOLDOWN_SECONDS" "$PROACTIVE_RS" || fail "STUCK_COOLDOWN_SECONDS constant missing from proactive.rs"
grep -q "DRIFT_SIMILARITY_THRESHOLD" "$PROACTIVE_RS" || fail "DRIFT_SIMILARITY_THRESHOLD constant missing from proactive.rs"
pass "all cooldown and threshold constants declared in proactive.rs (m15.2)"

# 7b. quiet-mode reorientation suppression has a named unit test
grep -q "fn quiet_mode_suppresses_reorientation" "$PROACTIVE_RS" || \
  fail "quiet_mode_suppresses_reorientation test missing from proactive.rs"
cargo test --manifest-path "$ROOT_DIR/desktop/src-tauri/Cargo.toml" \
  quiet_mode_suppresses_reorientation -- --test-threads=1
pass "quiet_mode_suppresses_reorientation test passes"

# 8. quiet mode guard present in all three trigger commands (m15.3)
grep -q "is_quiet_mode" "$COMMANDS_RS" || fail "quiet mode guard (is_quiet_mode) missing from commands.rs"
pass "quiet mode guard present in proactive commands (m15.3)"

# 9. frontend: reorientationBanner and triggerTaskResume in App.tsx (m15.4)
grep -q "reorientationBanner" "$APP_TSX" || fail "reorientationBanner state missing from App.tsx"
grep -q "triggerTaskResume" "$APP_TSX" || fail "triggerTaskResume call missing from App.tsx"
pass "re-orientation banner and triggerTaskResume present in App.tsx (m15.4)"

# 10. frontend: drift detection after send and drift flag notice (m15.4)
grep -q "checkTaskDrift" "$APP_TSX" || fail "checkTaskDrift call missing from App.tsx"
grep -q "drift-flag-notice" "$APP_TSX" || fail "drift-flag-notice test id missing from App.tsx"
pass "drift detection and drift-flag-notice present in App.tsx (m15.4)"

# 11. frontend: triggerSpeculativeSubtask in App.tsx (m15.4)
grep -q "triggerSpeculativeSubtask" "$APP_TSX" || fail "triggerSpeculativeSubtask call missing from App.tsx"
pass "triggerSpeculativeSubtask wired in App.tsx (m15.4)"

# 12. speculative subtask offer card with correct test id (m15.4)
grep -q "speculative-subtask-card" "$APP_TSX" || fail "speculative-subtask-card test id missing from App.tsx"
pass "speculative-subtask-card present in App.tsx (m15.4)"

# 13. speculative card supports cancel and dismiss actions (m15.4)
grep -q "speculative-subtask-cancel" "$APP_TSX" || fail "speculative-subtask-cancel button missing from App.tsx"
grep -q "speculative-subtask-dismiss" "$APP_TSX" || fail "speculative-subtask-dismiss button missing from App.tsx"
pass "speculative card Cancel and Dismiss actions present (m15.4)"

# 14. dismissProactiveTrigger in App.tsx (m15.4)
grep -q "dismissProactiveTrigger" "$APP_TSX" || fail "dismissProactiveTrigger call missing from App.tsx"
pass "dismissProactiveTrigger wired in App.tsx (m15.4)"

# 15. quiet mode toggle in companion header (m15.4)
grep -q "quiet-mode-toggle" "$APP_TSX" || fail "quiet-mode-toggle test id missing from App.tsx"
grep -q "handleToggleQuietMode" "$APP_TSX" || fail "handleToggleQuietMode missing from App.tsx"
pass "quiet mode toggle button present in companion header (m15.4)"

# 16. reorientation banner test id and auto-dismiss (m15.4)
grep -q "reorientation-banner" "$APP_TSX" || fail "reorientation-banner test id missing from App.tsx"
grep -q "8000" "$APP_TSX" || fail "8 second auto-dismiss timer (8000ms) missing from App.tsx"
pass "reorientation banner with auto-dismiss timer present (m15.4)"

# 17. typescript wrappers in tauriClient.ts (m15.4)
grep -q "ReorientationDto" "$TAURI_CLIENT_TS" || fail "ReorientationDto interface missing from tauriClient.ts"
grep -q "DriftFlagDto" "$TAURI_CLIENT_TS" || fail "DriftFlagDto interface missing from tauriClient.ts"
grep -q "ProactiveTriggerDto" "$TAURI_CLIENT_TS" || fail "ProactiveTriggerDto interface missing from tauriClient.ts"
grep -q "triggerTaskResume" "$TAURI_CLIENT_TS" || fail "triggerTaskResume wrapper missing from tauriClient.ts"
grep -q "checkTaskDrift" "$TAURI_CLIENT_TS" || fail "checkTaskDrift wrapper missing from tauriClient.ts"
grep -q "triggerSpeculativeSubtask" "$TAURI_CLIENT_TS" || fail "triggerSpeculativeSubtask wrapper missing from tauriClient.ts"
grep -q "dismissProactiveTrigger" "$TAURI_CLIENT_TS" || fail "dismissProactiveTrigger wrapper missing from tauriClient.ts"
grep -q "recordTaskFocus" "$TAURI_CLIENT_TS" || fail "recordTaskFocus wrapper missing from tauriClient.ts"
pass "all phase 15 TypeScript types and wrappers in tauriClient.ts (m15.4)"

# 18. proactive.rs mod declared in main.rs
grep -q "mod proactive" "$MAIN_RS" || fail "mod proactive not declared in main.rs"
pass "mod proactive declared in main.rs"

# 19. regression gates: prior phase checks still pass
echo "--- running regression gate: phase13_check.sh ---"
"$ROOT_DIR/scripts/phase13_check.sh"
pass "phase13_check.sh still passes (regression gate)"

echo "--- running regression gate: phase14_check.sh ---"
"$ROOT_DIR/scripts/phase14_check.sh"
pass "phase14_check.sh still passes (regression gate)"

# 20. full build + test suite
cd "$ROOT_DIR/desktop"
echo "--- running npm lint ---"
npm run lint
echo "--- running npm test ---"
npm run test
echo "--- running cargo build ---"
cargo build --manifest-path src-tauri/Cargo.toml
echo "--- running cargo test (proactive + store) ---"
cargo test --manifest-path src-tauri/Cargo.toml proactive
cargo test --manifest-path src-tauri/Cargo.toml store::tests
pass "full build, lint, and test suite passed"

echo ""
echo "=== phase 15 all checks passed ==="
