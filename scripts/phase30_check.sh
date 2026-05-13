#!/usr/bin/env bash
# phase 30 check: relational understanding — goals, patterns, collaboration style

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

echo ""
echo "phase 30: relational understanding"
echo "=================================="

grep_check "relational_model.rs exists" \
    "pub struct RelationalProfile" "$SRC/relational_model.rs"

grep_check "StatedGoal defined" \
    "pub struct StatedGoal" "$SRC/relational_model.rs"

grep_check "GoalStatus defined" \
    "pub enum GoalStatus" "$SRC/relational_model.rs"

grep_check "StrugglePattern defined" \
    "pub struct StrugglePattern" "$SRC/relational_model.rs"

grep_check "CollaborationStyle defaults present" \
    "prefers_opinions: 0.5" "$SRC/relational_model.rs"

grep_check "TrustMetrics defined" \
    "pub struct TrustMetrics" "$SRC/relational_model.rs"

grep_check "stated_goals migration present" \
    "CREATE TABLE IF NOT EXISTS stated_goals" "$SRC/store.rs"

grep_check "struggle_patterns migration present" \
    "CREATE TABLE IF NOT EXISTS struggle_patterns" "$SRC/store.rs"

grep_check "collaboration_style_signals migration present" \
    "CREATE TABLE IF NOT EXISTS collaboration_style_signals" "$SRC/store.rs"

grep_check "trust_metrics migration present" \
    "CREATE TABLE IF NOT EXISTS trust_metrics" "$SRC/store.rs"

grep_check "record_goal_stated implemented" \
    "pub fn record_goal_stated" "$SRC/relational_model.rs"

grep_check "record_struggle implemented" \
    "pub fn record_struggle" "$SRC/relational_model.rs"

grep_check "opinion accepted signal implemented" \
    "record_opinion_accepted" "$SRC/relational_model.rs"

grep_check "opinion pushback signal implemented" \
    "record_opinion_pushback" "$SRC/relational_model.rs"

grep_check "asked-for-more signal implemented" \
    "record_asked_for_more" "$SRC/relational_model.rs"

grep_check "chat records message relational signals" \
    "record_message_signals" "$SRC/chat.rs"

grep_check "streaming chat records message relational signals" \
    "record_message_signals" "$SRC/chat_streaming.rs"

grep_check "user_model acceptance updates relational model" \
    "record_opinion_accepted" "$SRC/user_model.rs"

grep_check "user_model rewrite updates relational model" \
    "record_opinion_pushback" "$SRC/user_model.rs"

grep_check "ambient monitor records recurring drift struggle" \
    "maybe_record_drift_struggle" "$SRC/proactive.rs"

grep_check "build_relational_context implemented" \
    "pub fn build_relational_context" "$SRC/relational_model.rs"

grep_check "chat prompt injects relational context" \
    "relational_context" "$SRC/character.rs"

grep_check "chat prompt builds relational context from store" \
    "build_relational_context" "$SRC/chat.rs"

grep_check "revision prompt has opinion-preference conditional" \
    "assessment_instruction_for_preference" "$SRC/character.rs"

grep_check "revision prompt reads prefers_opinions" \
    "prefers_opinions" "$SRC/revision.rs"

grep_check "subtask prompt reads prefers_opinions" \
    "prefers_opinions" "$SRC/subtask.rs"

grep_check "get_relational_profile command present" \
    "pub fn get_relational_profile" "$SRC/commands.rs"

grep_check "delete_stated_goal command present" \
    "pub fn delete_stated_goal" "$SRC/commands.rs"

grep_check "delete_struggle_pattern command present" \
    "pub fn delete_struggle_pattern" "$SRC/commands.rs"

grep_check "clear_relational_profile command present" \
    "pub fn clear_relational_profile" "$SRC/commands.rs"

grep_check "relational commands registered" \
    "commands::get_relational_profile" "$SRC/main.rs"

grep_check "frontend relational profile type present" \
    "RelationalProfileDto" "$FRONTEND/tauriClient.ts"

grep_check "frontend relational profile command present" \
    "getRelationalProfile" "$FRONTEND/tauriClient.ts"

grep_check "Jeff remembers goals section present" \
    "jeff-remembers-goals" "$FRONTEND/App.tsx"

grep_check "Jeff remembers patterns section present" \
    "jeff-remembers-patterns" "$FRONTEND/App.tsx"

grep_check "communication style section present" \
    "jeff-remembers-communication" "$FRONTEND/App.tsx"

echo ""
echo "  running behavioral unit tests..."
if cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" \
    relational_model::tests >/dev/null 2>&1; then
    check "relational_model behavioral unit tests pass" "ok"
else
    check "relational_model behavioral unit tests pass" "fail"
fi

if cargo test --manifest-path "$REPO_ROOT/desktop/src-tauri/Cargo.toml" \
    parse_generated_revision_extracts_assessment_from_plain_text >/dev/null 2>&1; then
    check "phase 29 regression: missing rationale is extracted" "ok"
else
    check "phase 29 regression: missing rationale is extracted" "fail"
fi

echo ""
if [ "$FAIL" -eq 0 ]; then
    echo "phase 30 checks: $PASS passed, $FAIL failed"
else
    echo "phase 30 checks: $PASS passed, $FAIL failed"
    exit 1
fi
