#!/usr/bin/env bash
# apex e3 check: Gmail read + draft -- triage, thread summary, email.draft via
# the bus (never send), reply watches firing AwaitedReplyLanded, triage eval.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT_DIR/desktop/src-tauri/src"
DESKTOP="$ROOT_DIR/desktop"
GMAIL="$SRC/gmail_core.rs"
STORE="$SRC/store.rs"
COMMANDS="$SRC/commands.rs"
MAIN="$SRC/main.rs"
CRISIS="$SRC/crisis.rs"
INBOX_EVAL="$ROOT_DIR/eval/inbox_eval.json"
APP_TSX="$DESKTOP/src/App.tsx"
TAURI_CLIENT="$DESKTOP/src/tauriClient.ts"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

echo "--- apex e3 gmail read/draft check ---"

# 1. Module + capabilities.
test -f "$GMAIL" || fail "gmail_core.rs missing"
grep -q "CREATE TABLE IF NOT EXISTS email_reply_watches" "$STORE" || fail "reply watch table missing"
grep -q "pub fn triage_inbox" "$GMAIL" || fail "inbox triage missing"
grep -q "pub fn summarize_thread" "$GMAIL" || fail "thread summarization missing"
grep -q "pub fn draft_reply" "$GMAIL" || fail "reply drafting missing"
grep -q "pub fn register_reply_watch" "$GMAIL" || fail "reply watch registration missing"
grep -q "pub fn resolve_landed_reply" "$GMAIL" || fail "awaited-reply resolution missing"
grep -q "pub fn connected_messages" "$GMAIL" || fail "connected Gmail read missing"
grep -q "pub fn connected_thread" "$GMAIL" || fail "connected Gmail thread read missing"
grep -q "pub fn connected_triage" "$GMAIL" || fail "connected Judgment-tier triage missing"
grep -q "persist_connected_action" "$GMAIL" || fail "Gmail draft is not persisted for exact-run approval"
grep -q "pub fn propose_message_label" "$GMAIL" || fail "Gmail label action missing"
pass "gmail core: triage, summary, draft, and reply watches present"

# 2. Draft is email.draft (never send); send stays hard-capped.
grep -q "ActionClass::EmailDraft" "$GMAIL" || fail "reply draft is not an email.draft action"
grep -q 'HARD_CAP_ACTION_CLASSES.*email.send' "$SRC/trust.rs" || fail "email.send is not hard-capped"
pass "reply drafts route as email.draft at L1; email.send stays hard-capped"

# 3. AwaitedReplyLanded crisis wired.
grep -q "pub fn maybe_fire_awaited_reply_landed" "$CRISIS" || fail "awaited-reply crisis missing"
grep -q "resolve_connected_reply_watches" "$COMMANDS" || fail "awaited-reply crisis does not read trusted Gmail data"
if grep -q "message: crate::gmail_core::EmailMessage" "$COMMANDS"; then
  fail "caller-supplied email can still trigger the crisis path"
fi
pass "AwaitedReplyLanded crisis fires when a watched reply lands"

# 4. Triage eval: >80% precision on a labeled 50-message inbox.
test -f "$INBOX_EVAL" || fail "inbox_eval.json missing"
MSG_COUNT=$(python3 -c "import json,sys; print(len(json.load(open(sys.argv[1]))['messages']))" "$INBOX_EVAL")
[ "$MSG_COUNT" -ge 50 ] || fail "triage eval needs >=50 messages, got $MSG_COUNT"
EVAL_OUT=$(bash "$ROOT_DIR/scripts/inbox_eval.sh" 2>&1)
echo "$EVAL_OUT" | tail -1
echo "$EVAL_OUT" | grep -qE "triage precision" || { echo "$EVAL_OUT"; fail "inbox eval did not report precision"; }
bash "$ROOT_DIR/scripts/inbox_eval.sh" >/dev/null 2>&1 || fail "triage precision below the 80% bar"
pass "triage eval passes the >80% precision bar on 50 labeled messages"

# 5. Compile + unit tests.
CHECK_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo check --quiet 2>&1)
if [ -n "$CHECK_OUT" ]; then echo "$CHECK_OUT"; fail "cargo check emitted warnings or errors"; fi
pass "cargo check passes without warnings"

for t in \
  e3_triage_flags_matters_and_skips_noise \
  e3_thread_summarizes_on_demand \
  e3_reply_watch_registers_and_landed_reply_resolves \
  e3_reply_watch_rejects_name_only_and_sender_substring_spoofing \
  e3_draft_reply_is_email_draft_receipt_never_send \
  e3_connected_gmail_registers_and_resolves_watch_from_trusted_messages; do
  grep -q "fn $t" "$GMAIL" || fail "expected e3 test $t is missing"
done
if ! E3_TEST_OUT=$(cd "$ROOT_DIR/desktop/src-tauri" && cargo test e3_ --quiet 2>&1); then
  echo "$E3_TEST_OUT"
  fail "e3 tests failed"
fi
echo "$E3_TEST_OUT" | grep -q "test result: ok" || { echo "$E3_TEST_OUT"; fail "e3 tests failed"; }
echo "$E3_TEST_OUT" | grep -q "FAILED" && { echo "$E3_TEST_OUT"; fail "e3 tests failed"; }
pass "e3 triage/summary/watch/draft tests pass"

# 6. Commands + Privacy Center surface.
for cmd in triage_inbox summarize_email_thread register_email_reply_watch list_email_reply_watches draft_email_reply propose_email_labels poll_email_reply_watches approve_connected_action reject_connected_action; do
  grep -q "pub fn $cmd" "$COMMANDS" || fail "$cmd command missing"
  grep -q "commands::$cmd" "$MAIN" || fail "$cmd not registered"
done
grep -q "listEmailReplyWatches" "$TAURI_CLIENT" || fail "frontend reply-watch binding missing"
grep -q "privacy-surface-email" "$APP_TSX" || fail "Privacy Center email surface missing"
grep -q "email-reply-watch-list" "$APP_TSX" || fail "reply watch list surface missing"
pass "commands and Privacy Center email surface are wired"

FRONTEND_LINT_OUT=$(cd "$DESKTOP" && npm run lint 2>&1)
echo "$FRONTEND_LINT_OUT" | grep -q "tsc --noEmit" || { echo "$FRONTEND_LINT_OUT"; fail "frontend lint did not run"; }
pass "frontend TypeScript check passes"

FRONTEND_TEST_OUT=$(cd "$DESKTOP" && npm test -- --run 2>&1)
echo "$FRONTEND_TEST_OUT" | grep -qE "Test Files.*passed" || { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
echo "$FRONTEND_TEST_OUT" | grep -qE "[0-9]+ failed" && { echo "$FRONTEND_TEST_OUT"; fail "frontend tests failed"; }
pass "frontend tests pass"

if [ "${JEFF_SKIP_ADJACENT_GATES:-0}" != "1" ]; then
  if ! ADJACENT_OUT=$(JEFF_SKIP_ADJACENT_GATES=1 bash "$ROOT_DIR/scripts/apex_e2_check.sh" 2>&1); then
    echo "$ADJACENT_OUT"
    fail "apex e2 web research gate regressed"
  fi
  pass "apex e2 web research gate still passes"
fi

echo "--- apex e3 check passed ---"
