#!/usr/bin/env bash
# phase 31 check: live content observation

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

absent_check() {
    local desc="$1"
    shift
    if grep -r "$@" >/dev/null 2>&1; then
        check "$desc" "fail"
    else
        check "$desc" "ok"
    fi
}

echo ""
echo "phase 31: live content observation"
echo "==================================="

# --- context_observer.rs structs and functions ---
grep_check "ContentObservationState struct in context_observer.rs" \
    "pub struct ContentObservationState" "$SRC/context_observer.rs"

grep_check "ContentObservation struct in context_observer.rs" \
    "pub struct ContentObservation" "$SRC/context_observer.rs"

grep_check "DraftState enum in context_observer.rs" \
    "pub enum DraftState" "$SRC/context_observer.rs"

grep_check "ChangeMagnitude enum in context_observer.rs" \
    "pub enum ChangeMagnitude" "$SRC/context_observer.rs"

grep_check "CONTENT_OBSERVATION_POLL_INTERVAL_SECONDS constant" \
    "CONTENT_OBSERVATION_POLL_INTERVAL_SECONDS" "$SRC/context_observer.rs"

grep_check "read_ax_document_text function in context_observer.rs" \
    "pub fn read_ax_document_text" "$SRC/context_observer.rs"

grep_check "summarize_content_observation function in context_observer.rs" \
    "pub fn summarize_content_observation" "$SRC/context_observer.rs"

grep_check "get_frontmost_pid function in context_observer.rs" \
    "pub fn get_frontmost_pid" "$SRC/context_observer.rs"

grep_check "JEFF_DISABLE_CONTENT_OBSERVATION env var guard in main.rs" \
    "JEFF_DISABLE_CONTENT_OBSERVATION" "$SRC/main.rs"

# --- state.rs: content_observation field in JeffState ---
grep_check "content_observation field in JeffState" \
    "pub content_observation" "$SRC/state.rs"

grep_check "ContentObservationState imported in state.rs" \
    "ContentObservationState" "$SRC/state.rs"

# --- awareness_core.rs: snapshot extensions ---
grep_check "active_document_excerpt field in SituationalSnapshot" \
    "pub active_document_excerpt" "$SRC/awareness_core.rs"

grep_check "content_idle_seconds field in SituationalSnapshot" \
    "pub content_idle_seconds" "$SRC/awareness_core.rs"

grep_check "ContentObservation trigger variant in awareness_core.rs" \
    "ContentObservation," "$SRC/awareness_core.rs"

grep_check "WorkQualityObservation check in should_speak_proactively" \
    "WorkQualityObservation" "$SRC/awareness_core.rs"

grep_check "active_document_excerpt in snapshot_summary" \
    "active document:" "$SRC/awareness_core.rs"

# --- commands.rs: new commands ---
grep_check "set_content_observation_enabled command present" \
    "pub fn set_content_observation_enabled" "$SRC/commands.rs"

grep_check "get_content_observation_enabled command present" \
    "pub fn get_content_observation_enabled" "$SRC/commands.rs"

grep_check "clear_content_observation command present" \
    "pub fn clear_content_observation" "$SRC/commands.rs"

grep_check "content_observation commands registered in main.rs" \
    "commands::set_content_observation_enabled" "$SRC/main.rs"

grep_check "clear_content_observation registered in main.rs" \
    "commands::clear_content_observation" "$SRC/main.rs"

# --- models.rs: PrivacyCenterDashboardDto extensions ---
grep_check "content_observation_enabled in PrivacyCenterDashboardDto" \
    "content_observation_enabled" "$SRC/models.rs"

grep_check "content_observation_capture_failed in PrivacyCenterDashboardDto" \
    "content_observation_capture_failed" "$SRC/models.rs"

# --- store.rs: per-task setting ---
grep_check "get_content_observation_enabled in store.rs" \
    "pub fn get_content_observation_enabled" "$SRC/store.rs"

grep_check "set_content_observation_enabled in store.rs" \
    "pub fn set_content_observation_enabled" "$SRC/store.rs"

# --- frontend ---
grep_check "setContentObservationEnabled in tauriClient.ts" \
    "setContentObservationEnabled" "$FRONTEND/tauriClient.ts"

grep_check "clearContentObservation in tauriClient.ts" \
    "clearContentObservation" "$FRONTEND/tauriClient.ts"

grep_check "content_observation_enabled in PrivacyCenterDashboardDto (ts)" \
    "content_observation_enabled" "$FRONTEND/tauriClient.ts"

grep_check "privacy-toggle-content-observation in App.tsx" \
    "privacy-toggle-content-observation" "$FRONTEND/App.tsx"

grep_check "Active document reading toggle in App.tsx" \
    "Active document reading" "$FRONTEND/App.tsx"

grep_check "content-observation-clear button in App.tsx" \
    "content-observation-clear" "$FRONTEND/App.tsx"

grep_check "content-observation-status in App.tsx" \
    "content-observation-status" "$FRONTEND/App.tsx"

grep_check "explanation text in App.tsx" \
    "This text never leaves your device" "$FRONTEND/App.tsx"

# --- behavioral: unit tests ---
echo ""
echo "  running content observation unit tests..."
if cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" \
    context_observer::tests::summarize >/dev/null 2>&1 \
    && cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" \
    awareness_core::tests::content_idle >/dev/null 2>&1 \
    && cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" \
    awareness_core::tests::work_quality_observation >/dev/null 2>&1 \
    && cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" \
    awareness_core::tests::snapshot_has_active_document >/dev/null 2>&1; then
    check "content observation and snapshot unit tests pass" "ok"
else
    check "content observation and snapshot unit tests pass" "fail"
fi

echo ""
echo "  running full backend test suite..."
if cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" >/dev/null 2>&1; then
    check "full backend test suite passes" "ok"
else
    check "full backend test suite passes" "fail"
fi

echo ""
if [ "$FAIL" -eq 0 ]; then
    echo "phase 31 checks: $PASS passed, $FAIL failed"
else
    echo "phase 31 checks: $PASS passed, $FAIL failed"
    exit 1
fi
