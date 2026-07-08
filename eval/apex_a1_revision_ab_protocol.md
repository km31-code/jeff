# apex a1 revision a/b protocol

Purpose: satisfy A1 done criterion 3 without letting model names bias the
judgment. The evaluator sees two anonymous outputs per case and chooses the one
with the stronger assessment and revision.

## inputs

- `eval/apex_a1_revision_ab_cases.json`: twenty revision requests.
- `legacy.jsonl`: one line per case from the pre-Apex/v2 path.
- `apex.jsonl`: one line per case from the Apex router path.

Each output line must be:

```json
{"id":"a1-ab-001","assessment":"...","revision":"..."}
```

## procedure

1. Generate `legacy.jsonl` using the old single-model configuration.
2. Generate `apex.jsonl` using the A1 router defaults.
3. Run:

```sh
scripts/apex_a1_ab_packet.sh legacy.jsonl apex.jsonl
```

4. Give the generated `packet.md` to the evaluator. Do not give them
   `answer_key.json`.
5. Fill `scorecard_template.json` with one winner per case: `A`, `B`, or
   `tie`.
6. Reveal the answer key and score the result:

```sh
scripts/apex_a1_ab_score.sh \
  artifacts/apex_a1_ab/answer_key.json \
  artifacts/apex_a1_ab/filled_scorecard.json \
  artifacts/apex_a1_ab/summary.json
```

## pass bar

A1 passes this gate only when Apex is preferred in at least 16 of the 20 cases.
Ties do not count as Apex wins.

## status discipline

If model outputs are generated but the evaluator has not judged them, A1 remains
`pending user A/B`. Do not mark A1 complete from automated checks alone.
