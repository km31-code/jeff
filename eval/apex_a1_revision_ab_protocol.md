# apex a1 revision a/b protocol

Purpose: satisfy A1 done criterion 3 without letting model names bias the
judgment. The evaluator sees two anonymous outputs per case and chooses the one
with the stronger assessment and revision.

## inputs

- `eval/apex_a1_revision_ab_cases.json`: twenty revision requests.
- `legacy.jsonl`: one line per case from the pre-Apex/v2 path.
- `apex.jsonl`: one line per case from the Apex router path.

Generating model outputs and running the live intent eval send prompts to the
configured external provider. Do that only after explicit approval for external
eval disclosure. The A1 check requires `JEFF_RUN_EXTERNAL_EVAL=1` before it will
run the live eval.

Each output line must be:

```json
{"id":"a1-ab-001","assessment":"...","revision":"..."}
```

## procedure

1. Generate `legacy.jsonl` using the old single-model configuration:

```sh
JEFF_RUN_EXTERNAL_EVAL=1 scripts/apex_a1_ab_generate.sh legacy \
  artifacts/apex_a1_ab/legacy.jsonl
```

2. Generate `apex.jsonl` using the A1 router default intent:

```sh
JEFF_RUN_EXTERNAL_EVAL=1 scripts/apex_a1_ab_generate.sh apex \
  artifacts/apex_a1_ab/apex.jsonl
```

The generator supports model/provider overrides for reproducibility:
`APEX_A1_LEGACY_MODEL`, `APEX_A1_APEX_PROVIDER`, and `APEX_A1_APEX_MODEL`.

## spend discipline

Use `gpt-4o-mini` and OpenAI-only provider overrides for smoke tests. To verify
plumbing without spending on the full packet, set `APEX_A1_CASE_LIMIT` to a
small number:

```sh
JEFF_RUN_EXTERNAL_EVAL=1 APEX_A1_CASE_LIMIT=2 \
  APEX_A1_APEX_PROVIDER=openai APEX_A1_APEX_MODEL=gpt-4o-mini \
  scripts/apex_a1_ab_generate.sh apex artifacts/apex_a1_ab/apex-smoke.jsonl
```

Limited outputs are smoke-test artifacts only. Final A1 scoring must unset
`APEX_A1_CASE_LIMIT` and generate all twenty cases for both `legacy.jsonl` and
`apex.jsonl`. Harder model calls should be explicit model overrides, not the
default test path.

The scripted live router eval sets `JEFF_PREFER_ENV_OPENAI_API_KEY=1` so a fresh
repo `.env` key is used even when the app keychain has an older stored key.
It also widens the classifier timeout and latency assertion for the live network
eval. Normal app key resolution and the runtime classifier timeout remain
keychain-first and tight by default.

3. Run:

```sh
scripts/apex_a1_ab_packet.sh \
  artifacts/apex_a1_ab/legacy.jsonl \
  artifacts/apex_a1_ab/apex.jsonl
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
