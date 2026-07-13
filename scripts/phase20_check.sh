#!/usr/bin/env bash
# phase 20 behavioral check script
# verifies: context_observer module, ContextState, commands, frontend
# integration, LLM injection, and SQLite safety.
# grep checks confirm symbol presence; cargo test confirms runtime behavior.

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
    if grep -r "$@" > /dev/null 2>&1; then
        check "$desc" "ok"
    else
        check "$desc" "fail"
    fi
}

echo ""
echo "phase 20: active window context (title-level)"
echo "=================================================================="

echo ""
echo "--- m20.1: context_observer module ---"

# module file must exist
if [ -f "$SRC/context_observer.rs" ]; then
    check "context_observer.rs exists" "ok"
else
    check "context_observer.rs exists" "fail"
fi

# module declared in main.rs
grep_check "context_observer mod in main.rs" \
    "mod context_observer" "$SRC/lib.rs"

# module exported in lib.rs for tests
grep_check "context_observer pub mod in lib.rs" \
    "pub mod context_observer" "$SRC/lib.rs"

# key symbols in context_observer.rs
grep_check "AXIsProcessTrustedWithOptions in context_observer.rs" \
    "AXIsProcessTrustedWithOptions" "$SRC/context_observer.rs"

grep_check "poll_active_window function in context_observer.rs" \
    "pub fn poll_active_window" "$SRC/context_observer.rs"

grep_check "is_accessibility_trusted function in context_observer.rs" \
    "pub fn is_accessibility_trusted" "$SRC/context_observer.rs"

grep_check "request_accessibility_permission function in context_observer.rs" \
    "pub fn request_accessibility_permission" "$SRC/context_observer.rs"

grep_check "ActiveWindowContext struct in context_observer.rs" \
    "pub struct ActiveWindowContext" "$SRC/context_observer.rs"

# non-macos stubs present
grep_check "non-macos cfg gate in context_observer.rs" \
    "cfg(not(target_os" "$SRC/context_observer.rs"

# title-suffix stripping for cleaner LLM context
grep_check "strip_title_suffix function in context_observer.rs" \
    "pub fn strip_title_suffix" "$SRC/context_observer.rs"

echo ""
echo "--- m20.2: ContextState + polling task + commands ---"

grep_check "ContextState struct in state.rs" \
    "pub struct ContextState" "$SRC/state.rs"

grep_check "should_nudge method in state.rs" \
    "fn should_nudge" "$SRC/state.rs"

grep_check "switch-aware should_nudge_for_switch in state.rs" \
    "fn should_nudge_for_switch" "$SRC/state.rs"

grep_check "mark_nudged method in state.rs" \
    "fn mark_nudged" "$SRC/state.rs"

grep_check "nudged title set prevents repeats" \
    "HashSet" "$SRC/state.rs"

grep_check "ContextState managed in main.rs" \
    "app.manage(ContextState::new())" "$SRC/main.rs"

# apex f1a moved the background polling loops into core_runtime.
grep_check "active-window polling loop present" \
    "async_runtime::spawn" "$SRC/main.rs" "$SRC/core_runtime.rs"

grep_check "context://document-switch event emitted by the context loop" \
    "context://document-switch" "$SRC/main.rs" "$SRC/core_runtime.rs"

grep_check "context://context-updated event emitted by the context loop" \
    "context://context-updated" "$SRC/main.rs" "$SRC/core_runtime.rs"

grep_check "nudged_titles cap constant in state.rs" \
    "MAX_NUDGED_TITLES" "$SRC/state.rs"

grep_check "context://context-updated listener in Overlay.tsx" \
    "context://context-updated" "$FRONTEND/Overlay.tsx"

grep_check "context://context-updated listener in App.tsx" \
    "context://context-updated" "$FRONTEND/App.tsx"

grep_check "get_active_window_context command in commands.rs" \
    "pub fn get_active_window_context" "$SRC/commands.rs"

grep_check "get_accessibility_permission_status command in commands.rs" \
    "pub fn get_accessibility_permission_status" "$SRC/commands.rs"

grep_check "request_accessibility_permission command in commands.rs" \
    "pub fn request_accessibility_permission" "$SRC/commands.rs"

grep_check "get_active_window_context registered in invoke_handler" \
    "commands::get_active_window_context" "$SRC/main.rs"

grep_check "get_accessibility_permission_status registered in invoke_handler" \
    "commands::get_accessibility_permission_status" "$SRC/main.rs"

grep_check "request_accessibility_permission registered in invoke_handler" \
    "commands::request_accessibility_permission" "$SRC/main.rs"

grep_check "ActiveWindowContextDto in models.rs" \
    "pub struct ActiveWindowContextDto" "$SRC/models.rs"

echo ""
echo "--- m20.3: companion header context display ---"

grep_check "get_active_window_context in Overlay.tsx" \
    "getActiveWindowContext" "$FRONTEND/Overlay.tsx"

grep_check "activeContext state in Overlay.tsx" \
    "activeContext" "$FRONTEND/Overlay.tsx"

grep_check "getActiveWindowContext in App.tsx" \
    "getActiveWindowContext" "$FRONTEND/App.tsx"

grep_check "activeContext state in App.tsx" \
    "activeContext" "$FRONTEND/App.tsx"

grep_check "companion-active-context in App.tsx" \
    "companion-active-context" "$FRONTEND/App.tsx"

grep_check "requestAccessibilityPermission in Overlay.tsx" \
    "requestAccessibilityPermission" "$FRONTEND/Overlay.tsx"

grep_check "requestAccessibilityPermission in App.tsx" \
    "requestAccessibilityPermission" "$FRONTEND/App.tsx"

grep_check "plain-language accessibility prompt in frontend" \
    "Jeff needs accessibility permission to know which document you have open" "$FRONTEND"

echo ""
echo "--- m20.4: llm system prompt injection ---"

grep_check "active_context param in chat.rs send_message_for_task" \
    "active_context" "$SRC/chat.rs"

grep_check "build_system_prompt helper in chat.rs" \
    "pub fn build_system_prompt" "$SRC/chat.rs"

grep_check "active_context threaded through chat_streaming.rs" \
    "active_context" "$SRC/chat_streaming.rs"

grep_check "active app context in commands.rs helper" \
    "User's active app" "$SRC/commands.rs"

grep_check "active_context in proactive.rs generate_reorientation" \
    "active_context" "$SRC/proactive.rs"

grep_check "chat grounding allows active-window title context" \
    "active_window_section" "$SRC/chat.rs"

echo ""
echo "--- m20.5: document-switch nudge ---"

grep_check "context://document-switch in Overlay.tsx" \
    "context://document-switch" "$FRONTEND/Overlay.tsx"

grep_check "context://document-switch in App.tsx" \
    "context://document-switch" "$FRONTEND/App.tsx"

grep_check "doc-switch-banner in Overlay.tsx" \
    "doc-switch-banner" "$FRONTEND/Overlay.tsx"

grep_check "doc-switch-banner in App.tsx" \
    "doc-switch-banner" "$FRONTEND/App.tsx"

grep_check "document-switch start-task CTA in Overlay.tsx" \
    "doc-switch-start-task" "$FRONTEND/Overlay.tsx"

grep_check "document-switch start-task CTA in App.tsx" \
    "doc-switch-start-task" "$FRONTEND/App.tsx"

grep_check "accessibility step in onboarding wizard (step 4)" \
    "onboarding-step-4" "$FRONTEND/Overlay.tsx"

grep_check "accessibility permission button in onboarding" \
    "onboarding-enable-accessibility" "$FRONTEND/Overlay.tsx"

grep_check "onboarding now has 5 steps" \
    "of 5" "$FRONTEND/Overlay.tsx"

echo ""
echo "--- safety: context never written to sqlite ---"

if grep -r "INSERT.*context_observer\|context_observer.*INSERT" "$SRC/store.rs" > /dev/null 2>&1; then
    check "context not persisted in store.rs (no INSERT)" "fail"
else
    check "context not persisted in store.rs (no INSERT)" "ok"
fi

# ContextState must not appear in store.rs at all (no accidental persistence)
if grep -q "ContextState" "$SRC/store.rs" > /dev/null 2>&1; then
    check "ContextState not referenced in store.rs" "fail"
else
    check "ContextState not referenced in store.rs" "ok"
fi

echo ""
echo "--- behavioral: cargo test ---"

echo "  running context_state_should_nudge..."
if (cd "$REPO_ROOT/desktop/src-tauri" && cargo test --lib context_state_should_nudge >/dev/null); then
    check "context_state_should_nudge passes" "ok"
else
    check "context_state_should_nudge passes" "fail"
fi

echo "  running context_state_nudges_only_on_real_unseen_switches..."
if (cd "$REPO_ROOT/desktop/src-tauri" && cargo test --lib context_state_nudges_only_on_real_unseen_switches >/dev/null); then
    check "context_state_nudges_only_on_real_unseen_switches passes" "ok"
else
    check "context_state_nudges_only_on_real_unseen_switches passes" "fail"
fi

echo "  running context_state_update_and_current..."
if (cd "$REPO_ROOT/desktop/src-tauri" && cargo test --lib context_state_update_and_current >/dev/null); then
    check "context_state_update_and_current passes" "ok"
else
    check "context_state_update_and_current passes" "fail"
fi

echo "  running is_accessibility_trusted_does_not_panic..."
if (cd "$REPO_ROOT/desktop/src-tauri" && cargo test --lib is_accessibility_trusted_does_not_panic >/dev/null); then
    check "is_accessibility_trusted_does_not_panic passes" "ok"
else
    check "is_accessibility_trusted_does_not_panic passes" "fail"
fi

echo "  running all bin tests (regression check)..."
if (cd "$REPO_ROOT/desktop/src-tauri" && cargo test --lib >/dev/null); then
    check "all bin tests pass (no regressions)" "ok"
else
    check "all bin tests pass (no regressions)" "fail"
fi

echo "  running frontend tests..."
if (cd "$REPO_ROOT/desktop" && npm run test >/dev/null); then
    check "frontend tests pass" "ok"
else
    check "frontend tests pass" "fail"
fi

echo ""
echo "=================================================================="
echo "phase 20 check: $PASS passed, $FAIL failed"
echo "=================================================================="

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
