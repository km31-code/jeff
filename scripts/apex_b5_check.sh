#!/usr/bin/env bash
# apex b5 check: ranked recall injection.
# Verifies local memory recall over durable facts and high-salience episodes,
# prompt injection in chat/revision/reorientation session blocks, and the
# no-memory/latency behavioral tests. No external API calls required.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
MEMORY_RS="$SRC/memory.rs"
CHARACTER_RS="$SRC/character.rs"
CHAT_RS="$SRC/chat.rs"
CHAT_STREAMING_RS="$SRC/chat_streaming.rs"
REVISION_RS="$SRC/revision.rs"
PROACTIVE_RS="$SRC/proactive.rs"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex b5 recall injection check ---"

# 1. Recall data model and ranking contract.
grep -q "pub struct RecalledItem" "$MEMORY_RS" || fail "RecalledItem missing"
grep -q "pub fn recall" "$MEMORY_RS" || fail "memory::recall missing"
grep -q "pub fn build_recall_block" "$MEMORY_RS" || fail "recall block builder missing"
grep -q "HIGH_SALIENCE_EPISODE_THRESHOLD: f32 = 0.70" "$MEMORY_RS" \
  || fail "high-salience episode threshold missing"
grep -q "MAX_RECALL_BLOCK_WORDS: usize = 120" "$MEMORY_RS" \
  || fail "120-word recall block cap missing"
grep -q "RECALL_FACT_LIMIT: usize = 500" "$MEMORY_RS" \
  || fail "500-fact recall candidate limit missing"
grep -q "cosine_similarity" "$MEMORY_RS" || fail "recall does not use cosine similarity"
grep -q "recency_weight" "$MEMORY_RS" || fail "recall does not weight recency"
grep -q "recall_score" "$MEMORY_RS" || fail "recall score function missing"
pass "ranked recall reads facts and high-salience episodes with similarity, salience, and recency"

# 2. Prompt-block contract.
grep -q "memory_recall: Option<String>" "$CHARACTER_RS" || fail "character contexts lack memory_recall"
grep -q "ctx.memory_recall.as_deref" "$CHARACTER_RS" || fail "memory recall not injected into prompts"
grep -q "b5_chat_recall_sits_after_relational_context_in_session_block" "$CHARACTER_RS" \
  || fail "session block order test missing"
grep -q "build_system_blocks_with_recall" "$CHAT_RS" || fail "non-streaming chat recall builder missing"
grep -q "build_system_blocks_with_recall" "$CHAT_STREAMING_RS" \
  || fail "streaming chat recall builder missing"
grep -q "memory::build_recall_block" "$REVISION_RS" || fail "revision recall injection missing"
grep -q "memory::build_recall_block" "$PROACTIVE_RS" || fail "reorientation recall injection missing"
pass "chat, streaming chat, revision, and reorientation prompt paths inject recall"

# 3. Behavioral tests.
grep -q "b5_empty_memory_injects_no_recall_block" "$MEMORY_RS" \
  || fail "empty-memory test missing"
grep -q "b5_recall_latency_under_30ms_at_500_facts" "$MEMORY_RS" \
  || fail "500-fact latency test missing"
grep -q "b5_seeded_user_fact_shapes_revision_assessment_prompt" "$REVISION_RS" \
  || fail "revision assessment memory test missing"
pass "B5 behavioral tests are present"

CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

B5_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test b5_ --quiet 2>&1)
echo "$B5_TEST_OUT" | grep -q "test result: ok" || { echo "$B5_TEST_OUT"; fail "b5 tests failed"; }
echo "$B5_TEST_OUT" | grep -q "FAILED" && { echo "$B5_TEST_OUT"; fail "b5 tests failed"; }
pass "B5 empty-memory, ranking, compact block, latency, prompt-order, and revision tests pass"

echo "--- apex b5 check passed ---"
