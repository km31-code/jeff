#!/usr/bin/env bash
# apex c3 check: briefing and debrief rituals.
# Verifies the briefing/debrief triggers, once-per-day guards, opt-in debrief,
# conversation-shaped delivery via new message kinds, the ambient-tick wiring,
# and the extended character eval. No external API calls required.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
BRIEFING="$SRC/briefing.rs"
MSG_KIND="$SRC/message_kind.rs"
PROACTIVE="$SRC/proactive.rs"
MAIN="$SRC/main.rs"
COMMANDS="$SRC/commands.rs"
APP_TSX="$DESKTOP/src/App.tsx"
TAURI_CLIENT="$DESKTOP/src/tauriClient.ts"
EVAL_JSON="$ROOT_DIR/eval/character_eval.json"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex c3 briefing and debrief rituals check ---"

# 1. briefing module + triggers.
test -f "$BRIEFING" || fail "briefing.rs missing"
grep -q "pub fn should_fire_briefing" "$BRIEFING" || fail "briefing trigger missing"
grep -q "pub fn should_fire_debrief" "$BRIEFING" || fail "debrief trigger missing"
grep -q "BRIEFING_AWAY_SECONDS: i64 = 6 \* 3600" "$BRIEFING" || fail "6h away threshold missing"
grep -q "DEBRIEF_IDLE_SECONDS: i64 = 45 \* 60" "$BRIEFING" || fail "45min idle threshold missing"
grep -q "DEBRIEF_EVENING_HOUR" "$BRIEFING" || fail "evening-hour gate missing"
grep -q "pub fn is_wrapping_up" "$BRIEFING" || fail "wrapping-up detector missing"
grep -q "Tier::Craft" "$BRIEFING" || fail "briefing composition is not Craft-tier"
grep -q "fn deterministic_briefing" "$BRIEFING" || fail "deterministic briefing fallback missing"
pass "briefing module: triggers, 6h/45min/evening gates, wrapping-up, Craft composition"

# 2. once-per-day + opt-in guards.
grep -q "BRIEFING_LAST_FIRED_KEY" "$BRIEFING" || fail "briefing once-per-day guard missing"
grep -q "DEBRIEF_LAST_FIRED_KEY" "$BRIEFING" || fail "debrief once-per-day guard missing"
grep -q "DEBRIEF_ENABLED_KEY" "$BRIEFING" || fail "debrief opt-in key missing"
# should_fire_debrief must short-circuit when disabled.
grep -q "if !enabled" "$BRIEFING" || fail "debrief does not require opt-in"
pass "once-per-day guards and debrief opt-in present"

# 3. conversation-shaped delivery via new message kinds.
grep -q "AssistantProactiveBriefing" "$MSG_KIND" || fail "briefing message kind missing"
grep -q "AssistantProactiveDebrief" "$MSG_KIND" || fail "debrief message kind missing"
grep -q "proactive_briefing" "$MSG_KIND" || fail "briefing kind string missing"
grep -q "proactive_debrief" "$MSG_KIND" || fail "debrief kind string missing"
grep -q "deliver_proactive_as_chat_message" "$BRIEFING" || fail "rituals do not use the phase 28 delivery path"
pass "briefing/debrief delivered as reply-able chat bubbles"

# 4. ambient-tick wiring + wrapping-up capture.
grep -q "mod briefing;" "${MAIN%/*}/lib.rs" || fail "briefing module not registered"
grep -q "maybe_fire_rituals" "$PROACTIVE" || fail "rituals not wired into the ambient tick"
grep -q "is_wrapping_up" "$SRC/chat.rs" || fail "chat does not detect wrapping-up"
grep -q "is_wrapping_up" "$SRC/chat_streaming.rs" || fail "streaming chat does not detect wrapping-up"
pass "rituals fire on the ambient tick; wrapping-up captured from chat"

# 5. debrief opt-in surface.
grep -q "pub fn get_debrief_enabled" "$COMMANDS" || fail "get_debrief_enabled command missing"
grep -q "pub fn set_debrief_enabled" "$COMMANDS" || fail "set_debrief_enabled command missing"
grep -q "commands::set_debrief_enabled" "$MAIN" || fail "debrief command not registered"
grep -q "setDebriefEnabled" "$TAURI_CLIENT" || fail "frontend debrief setter missing"
grep -q "privacy-toggle-debrief" "$APP_TSX" || fail "debrief opt-in toggle missing from Privacy Center"
pass "debrief opt-in command and Privacy Center toggle wired"

# 6. character eval extended with briefing cases.
grep -q "c3_briefing_opener" "$EVAL_JSON" || fail "briefing character-eval case missing"
grep -q "c3_debrief_close" "$EVAL_JSON" || fail "debrief character-eval case missing"
pass "character eval extended with briefing/debrief cases"

# 7. behavioral: clean compile, c3 tests, frontend, character corpus.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then
  echo "$CHECK_OUT"
  fail "cargo check emitted warnings or errors"
fi
pass "cargo check passes without warnings"

C3_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test c3_ --quiet 2>&1)
echo "$C3_TEST_OUT" | grep -q "test result: ok" || { echo "$C3_TEST_OUT"; fail "c3 tests failed"; }
echo "$C3_TEST_OUT" | grep -q "FAILED" && { echo "$C3_TEST_OUT"; fail "c3 tests failed"; }
pass "c3 trigger, opt-in, and composition tests pass"

MSG_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test message_kind --quiet 2>&1)
echo "$MSG_TEST_OUT" | grep -q "test result: ok" || { echo "$MSG_TEST_OUT"; fail "message_kind tests failed"; }
pass "message_kind round-trip covers briefing/debrief"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend TypeScript check did not run"; }
pass "frontend TypeScript check passes"

bash "$ROOT_DIR/scripts/phase32_check.sh" >/dev/null 2>&1 || fail "phase32 character-eval corpus check regressed"
pass "character eval corpus still valid"

echo "--- apex c3 check passed ---"
