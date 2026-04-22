# Phase 9 E2E Scenario: `history_storymap_full_session_check`

This is the source-of-truth acceptance scenario for Phase 9 integrated workflow quality.

## Fixture

- Fixture directory: `tests/fixtures/history_storymap/`
- Required files:
  - `notes.md`
  - `rubric.txt`
  - `intro_draft.md`

## Automation Hooks

1. Backend integrated check:
   - `cd desktop`
   - `cargo test --manifest-path src-tauri/Cargo.toml history_storymap_full_session_check -- --nocapture`
2. Frontend integrated check:
   - `cd desktop`
   - `npm run test`
3. Full gate:
   - `./scripts/phase9_check.sh`

## Scenario Steps

1. Resume active task `history storymap`.
2. Confirm task materials are visible and retrieval debug is enabled.
3. Ask a grounded requirements question (typed).
4. Ask one message via voice path.
5. Pause and confirm at least one proactive suggestion appears.
6. Accept a revision-oriented suggestion.
7. Review pending revision proposal.
8. Accept revision and confirm artifact + version state update.
9. Accept a subtask-oriented suggestion.
10. Keep chatting while subtask runs (parallel responsiveness check).
11. Review completed subtask result.
12. Convert subtask result to revision proposal.
13. Apply or reject revision explicitly (no silent apply).
14. Revert at least one accepted change.
15. Confirm action center and runtime inspector stay coherent throughout.
16. Restart app and confirm task/suggestions/subtasks/revisions remain coherent.

## Expected Outcomes

- Ask/answer remains retrieval-grounded.
- Voice/text are unified in one timeline.
- Suggestions route correctly to revision or bounded subtask paths.
- No silent artifact modification occurs.
- Pending user-action items are visible in one action center.
- Runtime inspector shows why the system is in its current state.
- Restart/resume returns to coherent state with persisted task context.
