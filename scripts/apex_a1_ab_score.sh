#!/usr/bin/env bash
# scores the blind a/b judgments for the apex a1 revision preference gate.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
CASES="$ROOT_DIR/eval/apex_a1_revision_ab_cases.json"
PASS_BAR=16

fail() { echo "FAIL: $1"; exit 1; }
pass() { echo "PASS: $1"; }

if [[ "${1:-}" == "--check" ]]; then
  python3 - "$CASES" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as fh:
    cases = json.load(fh)
ids = [case["id"] for case in cases]
if len(ids) != 20 or len(set(ids)) != 20:
    raise SystemExit("a1 a/b scorer requires exactly 20 unique cases")
print("PASS: a1 a/b scorer check passed")
PY
  exit 0
fi

ANSWER_KEY="${1:-}"
JUDGMENTS="${2:-}"
OUT="${3:-}"

if [[ -z "$ANSWER_KEY" || -z "$JUDGMENTS" ]]; then
  cat <<EOF
usage:
  scripts/apex_a1_ab_score.sh answer_key.json filled_scorecard.json [summary.json]

judgment format:
  [{"id":"a1-ab-001","winner":"A","notes":"optional"}]

winner must be A, B, or tie. Apex must win at least ${PASS_BAR}/20.
EOF
  exit 0
fi

test -f "$ANSWER_KEY" || fail "answer key not found: $ANSWER_KEY"
test -f "$JUDGMENTS" || fail "judgments not found: $JUDGMENTS"

python3 - "$CASES" "$ANSWER_KEY" "$JUDGMENTS" "$PASS_BAR" "$OUT" <<'PY'
import json
import sys

cases_path, answer_key_path, judgments_path, pass_bar_raw, out_path = sys.argv[1:6]
pass_bar = int(pass_bar_raw)

def load_json(path):
    with open(path, "r", encoding="utf-8") as fh:
        return json.load(fh)

cases = load_json(cases_path)
answer_key = load_json(answer_key_path)
judgments = load_json(judgments_path)

case_ids = [case["id"] for case in cases]
expected_ids = set(case_ids)

if len(answer_key) != len(case_ids):
    raise SystemExit(f"answer key must contain {len(case_ids)} rows")
if len(judgments) != len(case_ids):
    raise SystemExit(f"judgments must contain {len(case_ids)} rows")

key_by_id = {}
for row in answer_key:
    case_id = row.get("id")
    if case_id not in expected_ids:
        raise SystemExit(f"answer key has unknown id {case_id}")
    if row.get("A") not in ("apex", "legacy") or row.get("B") not in ("apex", "legacy"):
        raise SystemExit(f"answer key has invalid mapping for {case_id}")
    if row.get("A") == row.get("B"):
        raise SystemExit(f"answer key maps both sides to same source for {case_id}")
    key_by_id[case_id] = row

judgment_by_id = {}
for row in judgments:
    case_id = row.get("id")
    winner = str(row.get("winner", "")).strip().lower()
    if case_id not in expected_ids:
        raise SystemExit(f"judgments have unknown id {case_id}")
    if winner not in ("a", "b", "tie"):
        raise SystemExit(f"{case_id} winner must be A, B, or tie")
    if case_id in judgment_by_id:
        raise SystemExit(f"duplicate judgment for {case_id}")
    judgment_by_id[case_id] = winner

missing = [case_id for case_id in case_ids if case_id not in key_by_id or case_id not in judgment_by_id]
if missing:
    raise SystemExit(f"missing ids: {missing}")

apex_wins = 0
legacy_wins = 0
ties = 0
rows = []
for case_id in case_ids:
    winner = judgment_by_id[case_id]
    source = "tie" if winner == "tie" else key_by_id[case_id][winner.upper()]
    if source == "apex":
        apex_wins += 1
    elif source == "legacy":
        legacy_wins += 1
    else:
        ties += 1
    rows.append({"id": case_id, "winner": winner, "source": source})

summary = {
    "apex_wins": apex_wins,
    "legacy_wins": legacy_wins,
    "ties": ties,
    "pass_bar": pass_bar,
    "passed": apex_wins >= pass_bar,
    "rows": rows,
}

print(json.dumps({k: summary[k] for k in ("apex_wins", "legacy_wins", "ties", "pass_bar", "passed")}, indent=2))
if out_path:
    with open(out_path, "w", encoding="utf-8") as fh:
        json.dump(summary, fh, indent=2)
        fh.write("\n")

if apex_wins < pass_bar:
    raise SystemExit(f"Apex wins {apex_wins}/{len(case_ids)}; pass bar is {pass_bar}")
PY

pass "a1 a/b score passes"
