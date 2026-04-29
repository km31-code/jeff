#!/usr/bin/env bash
# phase 28 check: proactive synthesis lands in the chat stream

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
echo "phase 28: proactive chat delivery"
echo "================================="

grep_check "deliver_proactive_as_chat_message exists" \
    "pub async fn deliver_proactive_as_chat_message" "$SRC/proactive.rs"
grep_check "proactive message inserted event constant present" \
    "proactive://message_inserted" "$SRC/proactive.rs"
grep_check "proactive.rs emits message inserted event" \
    "EVENT_PROACTIVE_MESSAGE_INSERTED" "$SRC/proactive.rs"
grep_check "proactive delivery stores assistant chat message" \
    "append_chat_message(task_id, \"assistant\", \"assistant\"" "$SRC/proactive.rs"
grep_check "proactive delivery accepts reorientation kind" \
    "proactive_reorientation" "$SRC/message_kind.rs"
grep_check "proactive delivery accepts drift kind" \
    "proactive_drift" "$SRC/message_kind.rs"
grep_check "proactive delivery accepts blocker kind" \
    "proactive_blocker" "$SRC/message_kind.rs"
grep_check "proactive delivery accepts deadline kind" \
    "proactive_deadline" "$SRC/message_kind.rs"
grep_check "speculative subtask kind remains available" \
    "proactive_speculative_subtask" "$SRC/message_kind.rs"
grep_check "speculative subtask command delivers chat message" \
    "proactive_speculative_subtask" "$SRC/commands.rs"
grep_check "speculative subtask card event remains emitted" \
    "proactive://speculative_subtask" "$SRC/commands.rs"

grep_check "synthesis calls deliver_proactive_as_chat_message" \
    "deliver_proactive_as_chat_message" "$SRC/synthesis.rs"
grep_check "hidden overlay notification uses jeff title" \
    "title: \"jeff\".to_string()" "$SRC/synthesis.rs"
grep_check "hidden overlay notification uses synthesized body" \
    "body: message.to_string()" "$SRC/synthesis.rs"
grep_check "notification context kind uses proactive kind" \
    "context_kind: Some(kind.to_string())" "$SRC/synthesis.rs"
absent_check "synthesis no longer emits old proactive event" \
    "jeff://proactive-message-inserted" "$SRC/synthesis.rs"
absent_check "synthesis no longer stores assistant_proactive kind" \
    "AssistantProactive" "$SRC/synthesis.rs"

grep_check "overlay listens for proactive message event" \
    "proactive://message_inserted" "$FRONTEND/Overlay.tsx"
grep_check "overlay styles proactive messages by prefix" \
    "message_kind.startsWith(\"proactive_\")" "$FRONTEND/Overlay.tsx"
absent_check "overlay has no showReorientationBanner state" \
    "showReorientationBanner" "$FRONTEND/Overlay.tsx"
absent_check "overlay has no driftFlag state" \
    "driftFlag" "$FRONTEND/Overlay.tsx"
absent_check "overlay has no notification context banner state" \
    "notificationContext" "$FRONTEND/Overlay.tsx"
absent_check "overlay does not render opened-from-notification banner" \
    "opened from notification" "$FRONTEND/Overlay.tsx"
absent_check "overlay no longer checks assistant_proactive" \
    "assistant_proactive" "$FRONTEND/Overlay.tsx"
absent_check "overlay no longer renders proactive action card" \
    "overlay-proactive-actions" "$FRONTEND/Overlay.tsx"
grep_check "notification click refreshes proactive task messages" \
    "kind?.startsWith(\"proactive_\")" "$FRONTEND/Overlay.tsx"

grep_check "testing guide expects assistant chat bubble" \
    "normal Jeff assistant bubble" "$REPO_ROOT/docs/TESTING_GUIDE.md"
grep_check "delivery unit test present" \
    "deliver_proactive_inserts_message_with_correct_kind" "$SRC/proactive.rs"
grep_check "delivery test loads messages after insertion" \
    "list_chat_messages(task.id)" "$SRC/proactive.rs"
grep_check "delivery test asserts proactive prefix" \
    "starts_with(\"proactive_\")" "$SRC/proactive.rs"

run_check "proactive delivery unit test passes" \
    cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" --bin jeff-desktop deliver_proactive_inserts_message_with_correct_kind
run_check "proactive message kind round-trip test passes" \
    cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" --bin jeff-desktop proactive_message_kinds_round_trip_from_db
run_check "synthesis reason mapping test passes" \
    cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" --bin jeff-desktop synthesis_reasons_map_to_proactive_message_kinds
run_check "legacy proactive message normalization test passes" \
    cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" --bin jeff-desktop legacy_assistant_proactive_rows_normalize_to_phase_28_kind

echo ""
echo "phase 28 checks: $PASS passed, $FAIL failed"
if [ "$FAIL" -ne 0 ]; then
    exit 1
fi
