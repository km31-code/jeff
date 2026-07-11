#!/usr/bin/env bash
# crisis eval: runs deterministic override-channel fire/no-fire fixtures.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
INPUT_PATH="${1:-$ROOT_DIR/eval/crisis_eval.json}"

cargo run --quiet \
  --manifest-path "$ROOT_DIR/desktop/src-tauri/Cargo.toml" \
  --bin crisis_eval \
  -- "$INPUT_PATH"
