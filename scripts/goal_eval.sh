#!/usr/bin/env bash
# apex b2: goal extraction eval harness.
# always scores the retired prefix matcher and the deterministic heuristic
# against eval/goal_extraction_eval.json (no network). when JEFF_RUN_EXTERNAL_EVAL=1
# with an OpenAI key, also runs the reflex-tier llm extractor and enforces the
# >=85% bar. mirrors the character eval's env-key handling.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
EVAL_PATH="${1:-$ROOT_DIR/eval/goal_extraction_eval.json}"

if [ -z "${OPENAI_API_KEY:-}" ] && [ -f "$ROOT_DIR/.env" ]; then
  OPENAI_API_KEY="$(
    python3 - "$ROOT_DIR/.env" <<'PY'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
for line in path.read_text().splitlines():
    stripped = line.strip()
    if not stripped or stripped.startswith("#") or "=" not in stripped:
        continue
    key, value = stripped.split("=", 1)
    if key.strip() == "OPENAI_API_KEY":
        print(value.strip().strip('"').strip("'"))
        break
PY
  )"
  export OPENAI_API_KEY
fi

if [ -n "${OPENAI_API_KEY:-}" ]; then
  export JEFF_PREFER_ENV_OPENAI_API_KEY=1
fi

cargo run --quiet \
  --manifest-path "$ROOT_DIR/desktop/src-tauri/Cargo.toml" \
  --bin goal_eval \
  -- "$EVAL_PATH"
