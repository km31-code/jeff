#!/usr/bin/env bash
# phase 29 check: opinionated output — assessment-first revision cards

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
echo "phase 29: opinionated output"
echo "============================="

# --- backend ---
grep_check "extract_assessment_sentence in revision.rs" \
    "pub fn extract_assessment_sentence" "$SRC/revision.rs"

grep_check "extract_assessment_sentence uses first-person check" \
    "i'm\|i've\|\" i \"\|starts_with.*I " "$SRC/revision.rs"

grep_check "generate_revision_alternative in revision.rs" \
    "pub fn generate_revision_alternative" "$SRC/revision.rs"

grep_check "generate_revision_alternative in commands.rs" \
    "generate_revision_alternative" "$SRC/commands.rs"

grep_check "list_revision_alternatives command in commands.rs" \
    "list_revision_alternatives" "$SRC/commands.rs"

grep_check "parent_revision_id in models.rs" \
    "parent_revision_id" "$SRC/models.rs"

grep_check "parent_revision_id migration in store.rs" \
    "ADD COLUMN parent_revision_id" "$SRC/store.rs"

grep_check "parent_revision_id in NewRevisionProposalInput" \
    "parent_revision_id.*Option" "$SRC/store.rs"

grep_check "list_alternative_revisions in store.rs" \
    "list_alternative_revisions" "$SRC/store.rs"

grep_check "parent_revision_id in revision_from_row" \
    "parent_revision_id: row.get(16)" "$SRC/store.rs"

grep_check "generate_revision_alternative registered in main.rs" \
    "generate_revision_alternative" "$SRC/main.rs"

grep_check "list_revision_alternatives registered in main.rs" \
    "list_revision_alternatives" "$SRC/main.rs"

# --- unit tests ---
grep_check "extract_assessment test: first person extracts" \
    "extract_assessment_sentence_extracts_first_person_sentence" "$SRC/revision.rs"

grep_check "extract_assessment test: no first person returns none" \
    "extract_assessment_sentence_returns_none_for_no_first_person" "$SRC/revision.rs"

grep_check "parse_generated_revision extracts missing rationale" \
    "parse_generated_revision_extracts_assessment_from_plain_text" "$SRC/revision.rs"

grep_check "revision system prompt test includes assessment instruction" \
    "revision_system_prompt_includes_assessment_instruction" "$SRC/revision.rs"

# --- frontend ---
grep_check "parent_revision_id in tauriClient.ts RevisionProposalDto" \
    "parent_revision_id.*null" "$FRONTEND/tauriClient.ts"

grep_check "generateRevisionAlternative in tauriClient.ts" \
    "generateRevisionAlternative" "$FRONTEND/tauriClient.ts"

grep_check "listRevisionAlternatives in tauriClient.ts" \
    "listRevisionAlternatives" "$FRONTEND/tauriClient.ts"

grep_check "listTaskPendingRevisions imported in Overlay.tsx" \
    "listTaskPendingRevisions" "$FRONTEND/Overlay.tsx"

grep_check "generateRevisionAlternative imported in Overlay.tsx" \
    "generateRevisionAlternative" "$FRONTEND/Overlay.tsx"

grep_check "pendingRevisions state in Overlay.tsx" \
    "pendingRevisions" "$FRONTEND/Overlay.tsx"

grep_check "overlay-revision-proposal card rendered" \
    "overlay-revision-proposal" "$FRONTEND/Overlay.tsx"

grep_check "overlay-revision-rationale rendered before proposed text" \
    "overlay-revision-rationale" "$FRONTEND/Overlay.tsx"

grep_check "overlay-revision-proposed rendered" \
    "overlay-revision-proposed" "$FRONTEND/Overlay.tsx"

grep_check "see alternative button rendered" \
    "see alternative" "$FRONTEND/Overlay.tsx"

grep_check "handleLoadAlternative defined" \
    "handleLoadAlternative" "$FRONTEND/Overlay.tsx"

grep_check "handleApplyRevision defined" \
    "handleApplyRevision" "$FRONTEND/Overlay.tsx"

grep_check "handleRejectRevision defined" \
    "handleRejectRevision" "$FRONTEND/Overlay.tsx"

grep_check "alternativeRevisions state in Overlay.tsx" \
    "alternativeRevisions" "$FRONTEND/Overlay.tsx"

grep_check "overlay-revision-alt-card rendered" \
    "overlay-revision-alt-card" "$FRONTEND/Overlay.tsx"

grep_check "subtask result_summary in SpeculativeSubtaskState" \
    "result_summary.*null" "$FRONTEND/Overlay.tsx"

grep_check "overlay-subtask-assessment data-testid rendered" \
    "overlay-subtask-assessment" "$FRONTEND/Overlay.tsx"

grep_check "overlay-revision-rationale CSS defined" \
    "overlay-revision-rationale" "$FRONTEND/styles.css"

grep_check "overlay-revision-proposed CSS defined" \
    "overlay-revision-proposed" "$FRONTEND/styles.css"

grep_check "overlay-revision-alt-btn CSS defined" \
    "overlay-revision-alt-btn" "$FRONTEND/styles.css"

# --- behavioral: unit tests pass ---
echo ""
echo "  running unit tests..."
if cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" \
    -- extract_assessment revision_system_prompt >/dev/null 2>&1; then
    check "extract_assessment and revision_system_prompt unit tests pass" "ok"
else
    check "extract_assessment and revision_system_prompt unit tests pass" "fail"
fi

echo ""
if [ "$FAIL" -eq 0 ]; then
    echo "phase 29 checks: $PASS passed, $FAIL failed"
else
    echo "phase 29 checks: $PASS passed, $FAIL failed"
    exit 1
fi
