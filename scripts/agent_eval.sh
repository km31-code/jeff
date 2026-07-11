#!/usr/bin/env bash
# agent eval: runs delegated-job contracts against the deterministic runtime and
# asserts delivery-contract adherence. web-research contracts are e2-gated.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
INPUT_PATH="${1:-$ROOT_DIR/eval/agent_eval/contracts.json}"
MIN_PASSES="${JEFF_AGENT_EVAL_MIN_PASSES:-17}"

cargo run --quiet \
  --manifest-path "$ROOT_DIR/desktop/src-tauri/Cargo.toml" \
  --bin agent_eval \
  -- "$INPUT_PATH" \
  --min-passes "$MIN_PASSES"
