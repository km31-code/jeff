#!/usr/bin/env bash
# apex eval: the quality spine. Runs every deterministic eval suite from one
# entry point and reports a per-suite result. Env-gated (live-LLM) suites run
# only with JEFF_RUN_EXTERNAL_EVAL=1.
#
# Usage: scripts/apex_eval.sh [suite]
#   suite in {judgment, crisis, agent, inbox, latency, all}. Default: all.

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SUITE="${1:-all}"

pass_count=0
fail_count=0

run_suite() {
  local name="$1"
  shift
  echo "=== $name ==="
  if "$@"; then
    echo "[OK] $name"
    pass_count=$((pass_count + 1))
  else
    echo "[FAIL] $name"
    fail_count=$((fail_count + 1))
  fi
}

want() { [ "$SUITE" = "all" ] || [ "$SUITE" = "$1" ]; }

want judgment && run_suite "judgment eval" bash "$ROOT_DIR/scripts/judgment_eval.sh"
want crisis   && run_suite "crisis override eval" bash "$ROOT_DIR/scripts/crisis_eval.sh"
want agent    && run_suite "agent eval" bash "$ROOT_DIR/scripts/agent_eval.sh"
want inbox    && run_suite "inbox triage eval" bash "$ROOT_DIR/scripts/inbox_eval.sh"
want latency  && run_suite "latency matrix (phase17)" bash "$ROOT_DIR/scripts/phase17_check.sh"

# env-gated live-LLM suites (need API keys); opt in explicitly.
if [ "${JEFF_RUN_EXTERNAL_EVAL:-0}" = "1" ]; then
  want all && run_suite "character eval (live)" bash "$ROOT_DIR/scripts/character_eval.sh"
  want all && run_suite "goal extraction eval (live)" bash "$ROOT_DIR/scripts/goal_eval.sh"
else
  echo "note: character/goal live-LLM evals skipped (set JEFF_RUN_EXTERNAL_EVAL=1 to run)"
fi

echo "----------------------------------------"
echo "apex eval: $pass_count suite(s) passed, $fail_count failed"
[ "$fail_count" -eq 0 ]
