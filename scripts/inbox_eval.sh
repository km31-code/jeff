#!/usr/bin/env bash
# inbox eval: deterministic Gmail triage precision over a labeled 50-message set.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
INPUT_PATH="${1:-$ROOT_DIR/eval/inbox_eval.json}"

cargo run --quiet \
  --manifest-path "$ROOT_DIR/desktop/src-tauri/Cargo.toml" \
  --bin inbox_eval \
  -- "$INPUT_PATH"
