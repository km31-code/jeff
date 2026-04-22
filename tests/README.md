# Tests Folder

Phase 10 verification is implemented in:
- backend persistence/runtime tests:
  - `desktop/src-tauri/src/store.rs`
  - `desktop/src-tauri/src/coworking.rs`
  - `desktop/src-tauri/src/retrieval.rs`
  - `desktop/src-tauri/src/chat.rs`
  - `desktop/src-tauri/src/revision.rs`
  - `desktop/src-tauri/src/subtask.rs`
  - `desktop/src-tauri/src/flow.rs`
- frontend integrated shell tests:
  - `desktop/src/App.test.tsx`
- IPC contract check:
  - `scripts/verify_ipc_contract.sh`
- Phase 9 end-to-end scenario:
  - `tests/e2e/history_storymap_session.md`
- deterministic fixtures:
  - `tests/fixtures/history_storymap/`

Use `./scripts/phase10_check.sh` to run the full Phase 10 gate.
