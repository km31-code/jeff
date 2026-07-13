#!/usr/bin/env bash
# phase 26 check: awareness core and persistent situational snapshot

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$REPO_ROOT/desktop/src-tauri/src"

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
echo "phase 26: awareness core"
echo "========================"

check "awareness_core.rs exists" "$([ -f "$SRC/awareness_core.rs" ] && echo ok || echo fail)"
grep_check "SituationalSnapshot struct present" "pub struct SituationalSnapshot" "$SRC/awareness_core.rs"
grep_check "AttentionState enum present" "pub enum AttentionState" "$SRC/awareness_core.rs"
grep_check "PendingItem struct present" "pub struct PendingItem" "$SRC/awareness_core.rs"
grep_check "TimePressure struct present" "pub struct TimePressure" "$SRC/awareness_core.rs"
grep_check "SnapshotTrigger enum present" "pub enum SnapshotTrigger" "$SRC/awareness_core.rs"
grep_check "AwarenessCore struct present" "pub struct AwarenessCore" "$SRC/awareness_core.rs"
grep_check "tokio mutex backs snapshot" "snapshot: Mutex<SituationalSnapshot>" "$SRC/awareness_core.rs"
grep_check "snapshot_summary function present" "pub fn snapshot_summary" "$SRC/awareness_core.rs"
grep_check "deterministic update function present" "pub async fn update" "$SRC/awareness_core.rs"
if grep -q "generate_response" "$SRC/awareness_core.rs"; then
    check "awareness_core has no llm calls" "fail"
else
    check "awareness_core has no llm calls" "ok"
fi

grep_check "JeffState contains awareness_core field" "awareness_core: Arc<AwarenessCore>" "$SRC/state.rs"
grep_check "main registers awareness_core module" "mod awareness_core" "$SRC/lib.rs"
grep_check "new-turn awareness update wired for non-streaming chat" "SnapshotTrigger::NewTurn" "$SRC/commands.rs"
grep_check "new-turn awareness update wired for streaming chat" "SnapshotTrigger::NewTurn" "$SRC/chat_streaming.rs"
grep_check "focus-event update wired" "SnapshotTrigger::FocusEvent" "$SRC/commands.rs"
grep_check "window-switch update wired" "SnapshotTrigger::WindowSwitch" "$SRC/core_runtime.rs"
grep_check "subtask-completed update wired" "SnapshotTrigger::SubtaskCompleted" "$SRC/main.rs"
grep_check "time-tick update wired" "SnapshotTrigger::TimeTick" "$SRC/proactive.rs" "$SRC/synthesis.rs"
grep_check "calendar-event update wired" "SnapshotTrigger::CalendarEvent" "$SRC/core_runtime.rs"
grep_check "chat prompt accepts snapshot summary" "snapshot_summary" "$SRC/chat.rs" "$SRC/character.rs"
grep_check "reorientation prompt accepts snapshot summary" "snapshot_summary" "$SRC/proactive.rs" "$SRC/character.rs"
grep_check "debug snapshot command present" "get_situational_snapshot" "$SRC/commands.rs" "$SRC/main.rs"

run_check "awareness core unit tests pass" \
    cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" --bin jeff-desktop awareness_core::tests

run_check "full backend unit suite passes" \
    cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" --bin jeff-desktop

echo ""
echo "phase 26 checks: $PASS passed, $FAIL failed"
if [ "$FAIL" -ne 0 ]; then
    exit 1
fi
