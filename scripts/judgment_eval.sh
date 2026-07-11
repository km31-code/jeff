#!/usr/bin/env bash
# judgment eval: runs the deterministic stage-2 economics over labeled fixtures.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
INPUT_PATH="${1:-$ROOT_DIR/eval/judgment_eval.json}"
PASS_BAR="${JEFF_JUDGMENT_EVAL_PASS_BAR:-0.85}"

cargo run --quiet \
  --manifest-path "$ROOT_DIR/desktop/src-tauri/Cargo.toml" \
  --bin judgment_eval \
  -- "$INPUT_PATH" \
  --pass-bar "$PASS_BAR"
