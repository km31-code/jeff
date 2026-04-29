#!/usr/bin/env bash
# phase 27 check: synthesis judgment, logging, and ambient monitor consolidation

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$REPO_ROOT/desktop/src-tauri/src"
FRONTEND="$REPO_ROOT/desktop/src"

PASS=0
FAIL=0

check() {
    local desc="$1"
    local result="$2"
    if [ "$result" = "ok" ]; then
        echo "  [pass] $desc"
        PASS=$((PASS + 1))
    else
        echo "  [fail] $desc"
        FAIL=$((FAIL + 1))
    fi
}

grep_check() {
    local desc="$1"
    shift
    if grep -r "$@" >/dev/null 2>&1; then
        check "$desc" "ok"
    else
        check "$desc" "fail"
    fi
}

run_check() {
    local desc="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        check "$desc" "ok"
    else
        check "$desc" "fail"
    fi
}

echo ""
echo "phase 27: synthesis layer"
echo "========================="

check "synthesis.rs exists" "$([ -f "$SRC/synthesis.rs" ] && echo ok || echo fail)"
grep_check "ProactiveSpeechReason enum present" "pub enum ProactiveSpeechReason" "$SRC/awareness_core.rs"
grep_check "TaskReturn speech reason present" "TaskReturn" "$SRC/awareness_core.rs"
grep_check "DeadlinePressure speech reason present" "DeadlinePressure" "$SRC/awareness_core.rs"
grep_check "BlockerDetected speech reason present" "BlockerDetected" "$SRC/awareness_core.rs"
grep_check "WorkQualityObservation reserved reason present" "WorkQualityObservation" "$SRC/awareness_core.rs"
grep_check "UserProfile reads reorientation weight" "trigger_weight_reorientation" "$SRC/awareness_core.rs"
grep_check "should_speak_proactively function present" "pub fn should_speak_proactively" "$SRC/awareness_core.rs"
grep_check "synthesize_proactive_message function present" "pub async fn synthesize_proactive_message" "$SRC/awareness_core.rs"
grep_check "synthesis uses character reorientation prompt" "build_reorientation_system_prompt" "$SRC/awareness_core.rs"
grep_check "synthesis uses gpt-4o-mini" "gpt-4o-mini" "$SRC/awareness_core.rs"
grep_check "synthesis has 5 second timeout" "Duration::from_secs(5)" "$SRC/awareness_core.rs"
grep_check "synthesis strips filler phrases" "strip_filler_phrases" "$SRC/awareness_core.rs"

grep_check "synthesis_log table migration present" "CREATE TABLE IF NOT EXISTS synthesis_log" "$SRC/store.rs"
grep_check "synthesis_log stores snapshot confidence" "snapshot_confidence" "$SRC/store.rs"
grep_check "synthesis_log stores delivered flag" "delivered INTEGER NOT NULL DEFAULT 0" "$SRC/store.rs"
grep_check "log_synthesis_decision store method present" "pub fn log_synthesis_decision" "$SRC/store.rs"
grep_check "get_last_synthesis_at store method present" "pub fn get_last_synthesis_at" "$SRC/store.rs"
grep_check "list_synthesis_log store method present" "pub fn list_synthesis_log" "$SRC/store.rs"
grep_check "synthesis log DTO present" "pub struct SynthesisLogEntryDto" "$SRC/models.rs"

grep_check "main registers synthesis module" "mod synthesis" "$SRC/main.rs"
grep_check "run_synthesis_check present" "pub async fn run_synthesis_check" "$SRC/synthesis.rs"
grep_check "run_synthesis_check reads snapshot through TimeTick update" "SnapshotTrigger::TimeTick" "$SRC/synthesis.rs"
grep_check "run_synthesis_check calls should_speak_proactively" "should_speak_proactively" "$SRC/synthesis.rs"
grep_check "run_synthesis_check logs decisions" "log_synthesis_decision" "$SRC/synthesis.rs"
grep_check "run_synthesis_check suppresses quiet mode before LLM" "quiet_mode" "$SRC/synthesis.rs"
grep_check "run_synthesis_check dispatches native notification when hidden" "dispatch_notification" "$SRC/synthesis.rs"
grep_check "ambient monitor calls synthesis once" "run_synthesis_check" "$SRC/proactive.rs"

if python3 - "$SRC/proactive.rs" <<'PY'
from pathlib import Path
import re
import sys
text = Path(sys.argv[1]).read_text()
match = re.search(r"async fn run_monitor_cycle[\s\S]*?\n}", text)
if not match:
    raise SystemExit(1)
block = match.group(0)
old = [
    "check_reorientation_from_background",
    "check_drift_from_background",
    "check_stuck_from_background",
    "spawn_awareness_update",
]
raise SystemExit(0 if all(name not in block for name in old) else 1)
PY
then
    check "old background checks absent from ambient monitor loop" "ok"
else
    check "old background checks absent from ambient monitor loop" "fail"
fi

grep_check "get_synthesis_log command present" "pub fn get_synthesis_log" "$SRC/commands.rs"
grep_check "get_synthesis_log registered in invoke handler" "commands::get_synthesis_log" "$SRC/main.rs"
grep_check "frontend synthesis log client present" "getSynthesisLog" "$FRONTEND/tauriClient.ts"
grep_check "privacy center renders synthesis audit" "privacy-synthesis-audit-list" "$FRONTEND/App.tsx"
grep_check "frontend mock covers get_synthesis_log" "get_synthesis_log" "$FRONTEND/App.test.tsx"
grep_check "reorientation dismissals downweight reorientation profile key" "dismissProactiveTrigger.*reorientation" "$FRONTEND/App.tsx" "$FRONTEND/Overlay.tsx"
if grep -q "triggerTaskResume" "$FRONTEND/App.tsx" || grep -q "triggerSpeculativeSubtask" "$FRONTEND/App.tsx"; then
    check "workspace focus no longer runs old proactive checks" "fail"
else
    check "workspace focus no longer runs old proactive checks" "ok"
fi

run_check "should_speak unit tests pass" \
    cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" --bin jeff-desktop should_speak

run_check "synthesis helper tests pass" \
    cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" --bin jeff-desktop synthesis::tests

run_check "quiet-mode synthesis log assertion passes" \
    cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" --bin jeff-desktop synthesis_log_records_quiet_mode_suppression

echo ""
echo "phase 27 checks: $PASS passed, $FAIL failed"
if [ "$FAIL" -ne 0 ]; then
    exit 1
fi
