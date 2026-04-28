#!/usr/bin/env bash
# phase 25 check: character operationalization

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
echo "phase 25: character operationalization"
echo "====================================="

check "character.rs exists" "$([ -f "$SRC/character.rs" ] && echo ok || echo fail)"
grep_check "base_character_prompt exported" "pub fn base_character_prompt" "$SRC/character.rs"
grep_check "strip_filler_phrases exported" "pub fn strip_filler_phrases" "$SRC/character.rs"
grep_check "chat prompt builder exported" "pub fn build_chat_system_prompt" "$SRC/character.rs"
grep_check "revision prompt builder exported" "pub fn build_revision_system_prompt" "$SRC/character.rs"
grep_check "reorientation prompt builder exported" "pub fn build_reorientation_system_prompt" "$SRC/character.rs"
grep_check "subtask prompt builder exported" "pub fn build_subtask_system_prompt" "$SRC/character.rs"

if grep -q "GROUNDING_SYSTEM_PROMPT" "$SRC/chat.rs"; then
    check "chat.rs no longer owns hardcoded grounding prompt" "fail"
else
    check "chat.rs no longer owns hardcoded grounding prompt" "ok"
fi

grep_check "chat.rs calls character chat builder through build_system_prompt" \
    "build_chat_system_prompt" "$SRC/chat.rs"
grep_check "revision.rs calls character revision builder" \
    "build_revision_system_prompt" "$SRC/revision.rs"
grep_check "proactive.rs calls character reorientation builder" \
    "build_reorientation_system_prompt" "$SRC/proactive.rs"
grep_check "subtask.rs calls character subtask builder" \
    "build_subtask_system_prompt" "$SRC/subtask.rs"
grep_check "chat_streaming strips filler before finalizing output" \
    "strip_filler_phrases" "$SRC/chat_streaming.rs"
grep_check "subtask result storage strips filler" \
    "strip_filler_phrases" "$SRC/subtask.rs"
grep_check "character module exported by lib.rs" \
    "pub mod character" "$SRC/lib.rs"

run_check "character unit tests pass" \
    cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" --bin jeff-desktop character::tests

if [ -n "${OPENAI_API_KEY:-}" ]; then
    echo "  [info] OPENAI_API_KEY set; live character behavior should be verified manually through the app flow"
else
    echo "  [skip] live no-filler API assertion skipped because OPENAI_API_KEY is not set"
fi

echo ""
echo "phase 25 checks: $PASS passed, $FAIL failed"
if [ "$FAIL" -ne 0 ]; then
    exit 1
fi
