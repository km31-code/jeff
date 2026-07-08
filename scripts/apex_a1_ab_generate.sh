#!/usr/bin/env bash
# generates model outputs for the apex a1 revision a/b preference gate.
# external calls are disabled unless JEFF_RUN_EXTERNAL_EVAL=1.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
CASES="$ROOT_DIR/eval/apex_a1_revision_ab_cases.json"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

if [[ "${1:-}" == "--check" ]]; then
  test -f "$CASES" || fail "missing cases file"
  python3 - "$CASES" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as fh:
    cases = json.load(fh)
if len(cases) != 20:
    raise SystemExit(f"expected 20 cases, found {len(cases)}")
for case in cases:
    for key in ("id", "artifact", "instruction", "judging_focus"):
        if not str(case.get(key, "")).strip():
            raise SystemExit(f"{case.get('id', '<missing id>')} missing {key}")
print("PASS: a1 a/b generator check passed")
PY
  exit 0
fi

MODE="${1:-}"
OUT="${2:-}"

if [[ -z "$MODE" || -z "$OUT" ]]; then
  cat <<'EOF'
usage:
  JEFF_RUN_EXTERNAL_EVAL=1 scripts/apex_a1_ab_generate.sh legacy legacy.jsonl
  JEFF_RUN_EXTERNAL_EVAL=1 scripts/apex_a1_ab_generate.sh apex apex.jsonl

modes:
  legacy  uses OpenAI by default, APEX_A1_LEGACY_MODEL or gpt-4o-mini
  apex    uses Anthropic by default when ANTHROPIC_API_KEY exists, otherwise OpenAI fallback

optional overrides:
  APEX_A1_LEGACY_PROVIDER=openai
  APEX_A1_LEGACY_MODEL=gpt-4o-mini
  APEX_A1_APEX_PROVIDER=anthropic|openai
  APEX_A1_APEX_MODEL=<model name>
  APEX_A1_CASE_LIMIT=<n>  smoke-test only; final scoring requires all 20 cases
EOF
  exit 0
fi

case "$MODE" in
  legacy|apex) ;;
  *) fail "mode must be legacy or apex" ;;
esac

if [[ "${JEFF_RUN_EXTERNAL_EVAL:-}" != "1" ]]; then
  fail "external model generation requires JEFF_RUN_EXTERNAL_EVAL=1"
fi

mkdir -p "$(dirname "$OUT")"

python3 - "$CASES" "$MODE" "$OUT" <<'PY'
import json
import os
import sys
import time
import urllib.error
import urllib.request

cases_path, mode, out_path = sys.argv[1:4]

with open(cases_path, "r", encoding="utf-8") as fh:
    cases = json.load(fh)

case_limit_raw = os.environ.get("APEX_A1_CASE_LIMIT", "").strip()
if case_limit_raw:
    try:
        case_limit = int(case_limit_raw)
    except ValueError as exc:
        raise SystemExit("APEX_A1_CASE_LIMIT must be an integer") from exc
    if case_limit <= 0 or case_limit > len(cases):
        raise SystemExit(f"APEX_A1_CASE_LIMIT must be between 1 and {len(cases)}")
    cases = cases[:case_limit]
    print(
        f"INFO: limiting generation to {case_limit} case(s); "
        "do not use this output for final A1 scoring"
    )

SYSTEM_PROMPT = """You are Jeff, a direct writing coworker.
Revise the given artifact according to the instruction.
Return strict JSON only with this shape:
{"assessment":"one direct sentence naming the tradeoff or quality issue","revision":"the revised text"}
No filler, no praise, no markdown fences."""

def provider_and_model():
    if mode == "legacy":
        return (
            os.environ.get("APEX_A1_LEGACY_PROVIDER", "openai").strip().lower(),
            os.environ.get("APEX_A1_LEGACY_MODEL", "gpt-4o-mini").strip(),
        )

    provider = os.environ.get("APEX_A1_APEX_PROVIDER", "").strip().lower()
    if not provider:
        provider = "anthropic" if os.environ.get("ANTHROPIC_API_KEY", "").strip() else "openai"
    default_model = "claude-sonnet-5" if provider == "anthropic" else "gpt-4o-mini"
    return provider, os.environ.get("APEX_A1_APEX_MODEL", default_model).strip()

PROVIDER, MODEL = provider_and_model()

if PROVIDER not in ("openai", "anthropic"):
    raise SystemExit(f"unsupported provider {PROVIDER!r}")
if not MODEL:
    raise SystemExit("model name cannot be empty")

def case_prompt(case):
    return "\n\n".join([
        f"Artifact title: {case['artifact_title']}",
        f"Artifact:\n{case['artifact']}",
        f"Instruction:\n{case['instruction']}",
        f"Judging focus:\n{case['judging_focus']}",
    ])

def parse_json_text(text, case_id):
    try:
        data = json.loads(text)
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"{case_id}: provider returned non-json: {exc}: {text[:200]!r}") from exc
    assessment = str(data.get("assessment", "")).strip()
    revision = str(data.get("revision", "")).strip()
    if not assessment or not revision:
        raise RuntimeError(f"{case_id}: provider output missing assessment or revision")
    return {"id": case_id, "assessment": assessment, "revision": revision}

def request_json(url, headers, body):
    request = urllib.request.Request(
        url,
        data=json.dumps(body).encode("utf-8"),
        headers={**headers, "content-type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=90) as response:
            return json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        body_text = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"http {exc.code}: {body_text}") from exc

def call_openai(prompt):
    api_key = os.environ.get("OPENAI_API_KEY", "").strip()
    if not api_key:
        raise RuntimeError("OPENAI_API_KEY is required for OpenAI generation")
    payload = request_json(
        "https://api.openai.com/v1/chat/completions",
        {"authorization": f"Bearer {api_key}"},
        {
            "model": MODEL,
            "temperature": 0,
            "response_format": {"type": "json_object"},
            "messages": [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": prompt},
            ],
        },
    )
    return payload["choices"][0]["message"]["content"]

def call_anthropic(prompt):
    api_key = os.environ.get("ANTHROPIC_API_KEY", "").strip()
    if not api_key:
        raise RuntimeError("ANTHROPIC_API_KEY is required for Anthropic generation")
    payload = request_json(
        "https://api.anthropic.com/v1/messages",
        {"x-api-key": api_key, "anthropic-version": "2023-06-01"},
        {
            "model": MODEL,
            "max_tokens": 1200,
            "temperature": 0,
            "system": SYSTEM_PROMPT + "\nRespond with a single valid JSON object and nothing else.",
            "messages": [{"role": "user", "content": prompt}],
        },
    )
    return "".join(block.get("text", "") for block in payload.get("content", [])).strip()

call = call_anthropic if PROVIDER == "anthropic" else call_openai

with open(out_path, "w", encoding="utf-8") as out:
    for index, case in enumerate(cases, start=1):
        case_id = case["id"]
        raw = call(case_prompt(case))
        row = parse_json_text(raw, case_id)
        out.write(json.dumps(row, ensure_ascii=False) + "\n")
        out.flush()
        print(f"PASS: {mode} {index:02d}/{len(cases)} {case_id} via {PROVIDER}:{MODEL}")
        time.sleep(0.1)
PY

pass "wrote $OUT"
