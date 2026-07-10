#!/usr/bin/env bash
# apex c1 check: two-stage judgment.
# Verifies stage 1 deterministic candidate generation (existing reasons plus new
# sources), stage 2 judgment-tier decision owning verdict/channel/wording with
# recall injected, channel-aware delivery, the retired 600s cooldown, and the
# extended synthesis_log. No external API calls required.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
SYNTH="$SRC/synthesis.rs"
AWARE="$SRC/awareness_core.rs"
STORE="$SRC/store.rs"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex c1 two-stage judgment check ---"

# 1. stage 1: deterministic candidate generation with new sources.
grep -q "pub fn generate_proactive_candidate" "$AWARE" || fail "stage 1 candidate generator missing"
grep -q "fn select_base_reason" "$AWARE" || fail "base reason selection not extracted"
grep -q "ComprehensionObservation" "$AWARE" || fail "b7 comprehension candidate missing"
grep -q "PendingApprovalAging" "$AWARE" || fail "pending-approval aging candidate missing"
grep -q "fn pending_approval_aging" "$AWARE" || fail "pending-approval aging helper missing"
pass "stage 1 generates candidates from existing reasons + comprehension + pending aging"

# 2. the fixed 600s cooldown is retired for a 300s soft minimum.
grep -q "PROACTIVE_MIN_DELIVERY_GAP_SECONDS: i64 = 300" "$AWARE" \
  || fail "300s soft-minimum delivery gap missing"
if grep -q "PROACTIVE_COOLDOWN_SECONDS" "$AWARE"; then
  fail "fixed 600s cooldown constant not retired"
fi
grep -q "generate_proactive_candidate" "$SYNTH" || fail "synthesis does not use stage 1 candidate generation"
if grep -q "should_speak_proactively(" "$SYNTH"; then
  fail "synthesis still on the legacy single-stage path"
fi
pass "600s cooldown retired; live path uses stage 1 candidate generation"

# 3. stage 2: one judgment-tier decision owns verdict, channel, and wording.
grep -q "STAGE2_SYSTEM_PROMPT" "$SYNTH" || fail "stage 2 system prompt missing"
grep -q "fn decide_proactive_stage2" "$SYNTH" || fail "stage 2 decision missing"
grep -q "pub fn parse_stage2_json" "$SYNTH" || fail "stage 2 JSON parsing missing"
grep -q "enum Stage2Verdict" "$SYNTH" || fail "speak/hold/drop verdict missing"
grep -q "enum Stage2Channel" "$SYNTH" || fail "channel enum missing"
grep -q "Tier::Judgment" "$SYNTH" || fail "stage 2 does not run at Judgment tier"
grep -q "STAGE2_TIMEOUT_MS: u64 = 4000" "$SYNTH" || fail "stage 2 <4s latency budget missing"
grep -q "build_recall_block" "$SYNTH" || fail "stage 2 does not inject the B5 recall block"
grep -q "ledger" "$SYNTH" || fail "stage 2 lacks the interruption-ledger slot (C2 stub)"
pass "stage 2 judgment call owns verdict/channel/wording with recall + ledger slot"

# 4. channel-aware delivery + hold/drop handling.
grep -q "fn deliver_by_channel" "$SYNTH" || fail "channel-aware delivery missing"
grep -q "Stage2Channel::Notification" "$SYNTH" || fail "notification channel delivery missing"
grep -q "Stage2Channel::SilentCard" "$SYNTH" || fail "silent-card (non-notifying) delivery missing"
grep -q "Stage2Verdict::Hold | Stage2Verdict::Drop" "$SYNTH" || fail "hold/drop are not logged without delivery"
pass "delivery honors channel; hold/drop recorded without delivery"

# 5. synthesis_log gains stage 2 columns + logger.
grep -q "ADD COLUMN stage2_decision" "$STORE" || fail "stage2_decision migration missing"
grep -q "ADD COLUMN stage2_channel" "$STORE" || fail "stage2_channel migration missing"
grep -q "ADD COLUMN stage2_reason" "$STORE" || fail "stage2_reason migration missing"
grep -q "pub fn log_synthesis_decision_staged" "$STORE" || fail "staged logger missing"
pass "synthesis_log stage 2 columns and staged logger present"

# 6. behavioral: clean compile + c1 tests.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

C1_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test c1_ --quiet 2>&1)
echo "$C1_TEST_OUT" | grep -q "test result: ok" || { echo "$C1_TEST_OUT"; fail "c1 tests failed"; }
echo "$C1_TEST_OUT" | grep -q "FAILED" && { echo "$C1_TEST_OUT"; fail "c1 tests failed"; }
pass "c1 stage-1 candidate + stage-2 parse/log tests pass"

# 7. existing synthesis surfaces still pass.
bash "$ROOT_DIR/scripts/phase27_check.sh" >/dev/null 2>&1 || fail "phase27 synthesis check regressed"
bash "$ROOT_DIR/scripts/phase28_check.sh" >/dev/null 2>&1 || fail "phase28 conversational proactivity check regressed"
pass "phase27 and phase28 checks still pass"

echo "--- apex c1 check passed ---"
