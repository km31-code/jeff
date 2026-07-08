#!/usr/bin/env bash
# builds a blind a/b packet for the apex a1 revision preference gate.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
CASES="$ROOT_DIR/eval/apex_a1_revision_ab_cases.json"
PROTOCOL="$ROOT_DIR/eval/apex_a1_revision_ab_protocol.md"
OUT_DIR="${APEX_A1_AB_OUT_DIR:-$ROOT_DIR/artifacts/apex_a1_ab}"

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

check_only=false
if [[ "${1:-}" == "--check" ]]; then
  check_only=true
fi

test -f "$CASES" || fail "missing cases file"
test -f "$PROTOCOL" || fail "missing protocol file"

python3 - "$CASES" <<'PY'
import json
import sys

path = sys.argv[1]
with open(path, "r", encoding="utf-8") as fh:
    cases = json.load(fh)

required = {"id", "artifact_title", "artifact", "instruction", "judging_focus"}
if len(cases) != 20:
    raise SystemExit(f"expected 20 cases, found {len(cases)}")

seen = set()
for case in cases:
    missing = sorted(required - set(case))
    if missing:
        raise SystemExit(f"{case.get('id', '<missing id>')} missing {missing}")
    case_id = case["id"]
    if case_id in seen:
        raise SystemExit(f"duplicate case id {case_id}")
    seen.add(case_id)
    for key in required:
        if not str(case[key]).strip():
            raise SystemExit(f"{case_id} has empty {key}")

print("PASS: a1 a/b cases valid")
PY

if "$check_only"; then
  pass "a1 a/b protocol present"
  exit 0
fi

LEGACY="${1:-}"
APEX="${2:-}"
if [[ -z "$LEGACY" || -z "$APEX" ]]; then
  cat <<EOF
PASS: a1 a/b case and protocol files are valid

to build a blinded packet, provide:
  scripts/apex_a1_ab_packet.sh legacy.jsonl apex.jsonl

expected output line format:
  {"id":"a1-ab-001","assessment":"...","revision":"..."}
EOF
  exit 0
fi

test -f "$LEGACY" || fail "legacy output file not found: $LEGACY"
test -f "$APEX" || fail "apex output file not found: $APEX"
mkdir -p "$OUT_DIR"

python3 - "$CASES" "$LEGACY" "$APEX" "$OUT_DIR" <<'PY'
import hashlib
import json
import pathlib
import sys

cases_path, legacy_path, apex_path, out_dir = sys.argv[1:5]
out = pathlib.Path(out_dir)

def load_jsonl(path):
    rows = {}
    with open(path, "r", encoding="utf-8") as fh:
        for line_no, line in enumerate(fh, start=1):
            if not line.strip():
                continue
            row = json.loads(line)
            for key in ("id", "assessment", "revision"):
                if not str(row.get(key, "")).strip():
                    raise SystemExit(f"{path}:{line_no} missing {key}")
            if row["id"] in rows:
                raise SystemExit(f"{path}:{line_no} duplicate id {row['id']}")
            rows[row["id"]] = row
    return rows

with open(cases_path, "r", encoding="utf-8") as fh:
    cases = json.load(fh)

legacy = load_jsonl(legacy_path)
apex = load_jsonl(apex_path)
case_ids = [case["id"] for case in cases]

if set(legacy) != set(case_ids):
    raise SystemExit("legacy outputs do not match case ids")
if set(apex) != set(case_ids):
    raise SystemExit("apex outputs do not match case ids")

packet_lines = [
    "# apex a1 blind revision a/b packet",
    "",
    "choose `A`, `B`, or `tie` for each case. judge the assessment and revision together.",
    "",
]
answer_key = []

for case in cases:
    case_id = case["id"]
    digest = hashlib.sha256(case_id.encode("utf-8")).hexdigest()
    apex_is_a = int(digest[-1], 16) % 2 == 0
    first = apex[case_id] if apex_is_a else legacy[case_id]
    second = legacy[case_id] if apex_is_a else apex[case_id]
    answer_key.append({
        "id": case_id,
        "A": "apex" if apex_is_a else "legacy",
        "B": "legacy" if apex_is_a else "apex",
    })
    packet_lines.extend([
        f"## {case_id}: {case['artifact_title']}",
        "",
        f"artifact: {case['artifact']}",
        "",
        f"instruction: {case['instruction']}",
        "",
        f"judging focus: {case['judging_focus']}",
        "",
        "### A",
        "",
        f"assessment: {first['assessment']}",
        "",
        f"revision: {first['revision']}",
        "",
        "### B",
        "",
        f"assessment: {second['assessment']}",
        "",
        f"revision: {second['revision']}",
        "",
        "winner: ",
        "",
    ])

(out / "packet.md").write_text("\n".join(packet_lines), encoding="utf-8")
(out / "answer_key.json").write_text(
    json.dumps(answer_key, indent=2) + "\n",
    encoding="utf-8",
)
print(f"PASS: wrote {out / 'packet.md'}")
print(f"PASS: wrote {out / 'answer_key.json'}")
PY
