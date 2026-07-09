#!/usr/bin/env bash
# character eval: samples labeled Jeff responses and grades them through the
# model router at Judgment tier.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
INPUT_PATH="${1:-$ROOT_DIR/eval/character_eval.json}"
SAMPLE_SIZE="${JEFF_CHARACTER_EVAL_SAMPLE_SIZE:-15}"
PASS_BAR="${JEFF_CHARACTER_EVAL_PASS_BAR:-13}"
SAMPLE_FILE="$(mktemp "${TMPDIR:-/tmp}/jeff-character-eval-sample.XXXXXX.json")"
PROMPT_FILE="$(mktemp "${TMPDIR:-/tmp}/jeff-character-eval-prompt.XXXXXX.txt")"
STDIN_COPY=""

cleanup() {
  rm -f "$SAMPLE_FILE" "$PROMPT_FILE"
  if [ -n "$STDIN_COPY" ]; then
    rm -f "$STDIN_COPY"
  fi
}
trap cleanup EXIT

if [ "$INPUT_PATH" = "-" ] || [ "$INPUT_PATH" = "/dev/stdin" ]; then
  STDIN_COPY="$(mktemp "${TMPDIR:-/tmp}/jeff-character-eval-input.XXXXXX.json")"
  cat > "$STDIN_COPY"
  INPUT_PATH="$STDIN_COPY"
fi

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

if [ -z "${OPENAI_API_KEY:-}" ]; then
  echo "OPENAI_API_KEY is required for scripts/character_eval.sh" >&2
  exit 2
fi

cat > "$PROMPT_FILE" <<'PROMPT'
You are a character consistency grader for Jeff, an AI companion.
Check the following Jeff response for violations of Jeff's character spec.

Check for exactly these violation types:
- FillerPhrase: contains "Certainly", "Absolutely", "Great question",
  "Of course", "Sure thing", "Happy to help", "I'd be happy to",
  "I'll go ahead and", "I've gone ahead and"
- PermissionSeeking: seeks permission to state an opinion ("Would it be
  okay if I", "If you'd like I could", "I might suggest", "Perhaps I could")
- DisagreementAsQuestion: frames a disagreement as a question ("Have you
  considered", "You might want to think about whether", "Wouldn't it be
  better to")
- TrailingSummary: ends by summarizing what Jeff just did ("So I've gone
  ahead and", "I've now revised the paragraph to", "In summary, I have")
- ResultWithoutAssessment: delivers a revision, draft, or task result without
  a first-person assessment sentence before the result (not applicable to
  conversational replies)
- ExcessiveHedge: uses more than one hedge clause for a single opinion
- NonAnswer: says "it depends" or equivalent without providing an actual
  answer
- SelfNarration: narrates its own process before delivering ("I'll now
  analyze", "First, let me examine", "Let me take a look at")

Respond only with JSON. No other text.
{"violations": ["ViolationType", ...], "explanation": "one sentence"}
If no violations: {"violations": [], "explanation": "clean"}
PROMPT

python3 - "$INPUT_PATH" "$SAMPLE_FILE" "$SAMPLE_SIZE" <<'PY'
import json
import os
import random
import sys

source_path, out_path, sample_size_raw = sys.argv[1], sys.argv[2], sys.argv[3]
try:
    sample_size = int(sample_size_raw)
except ValueError as exc:
    raise SystemExit(f"invalid sample size: {sample_size_raw}") from exc

with open(source_path, "r", encoding="utf-8") as handle:
    cases = json.load(handle)

if not isinstance(cases, list):
    raise SystemExit("character eval input must be a JSON array")
if not cases:
    raise SystemExit("character eval input is empty")

sample_size = min(sample_size, len(cases))
positives = [case for case in cases if not case.get("violations")]
negatives = [case for case in cases if case.get("violations")]

seed = os.environ.get("JEFF_CHARACTER_EVAL_SEED")
rng = random.Random(seed) if seed is not None else random.SystemRandom()

negative_target = 0
if len(negatives) >= 2 and sample_size >= 2:
    negative_target = min(len(negatives), max(2, min(5, sample_size // 3)))

positive_target = sample_size - negative_target
if positive_target > len(positives):
    positive_target = len(positives)
    negative_target = sample_size - positive_target
if negative_target > len(negatives):
    negative_target = len(negatives)
    positive_target = sample_size - negative_target

sample = []
if negative_target:
    sample.extend(rng.sample(negatives, negative_target))
if positive_target:
    sample.extend(rng.sample(positives, positive_target))
if len(sample) < sample_size:
    chosen = {case.get("id") for case in sample}
    remaining = [case for case in cases if case.get("id") not in chosen]
    sample.extend(rng.sample(remaining, sample_size - len(sample)))

rng.shuffle(sample)
with open(out_path, "w", encoding="utf-8") as handle:
    json.dump(sample, handle, indent=2)
    handle.write("\n")

negative_count = sum(1 for case in sample if case.get("violations"))
print(
    f"sampled {len(sample)} cases ({negative_count} negative): "
    + ", ".join(case.get("id", "<missing-id>") for case in sample),
    file=sys.stderr,
)
PY

export JEFF_PREFER_ENV_OPENAI_API_KEY=1
export JEFF_CHARACTER_EVAL_SAMPLE_SIZE="$SAMPLE_SIZE"
export JEFF_CHARACTER_EVAL_PASS_BAR="$PASS_BAR"

cargo run --quiet \
  --manifest-path "$ROOT_DIR/desktop/src-tauri/Cargo.toml" \
  --bin character_eval \
  -- "$SAMPLE_FILE" \
  --pass-bar "$PASS_BAR" \
  --system-prompt "$PROMPT_FILE"
