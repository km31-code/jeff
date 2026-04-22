# Jeff

Jeff is a local-first desktop AI coworker focused on one active task.

This repository is currently at **Phase 12**: ambient presence + streaming-everywhere on top of the companion-first interaction layer.

## Phase 10-12 Scope

Implemented across these phases:
- default **Companion Mode** UI:
  - minimal context header
  - chat + voice as primary surface
  - low-cognitive-load entry
- conversational routing from normal input:
  - revision-like requests -> revision proposal path
  - drafting requests -> bounded subtask path
  - next-step requests -> suggestion evaluation
  - regular questions -> grounded answer path
- inline conversational action cards in companion chat flow:
  - revision actions: `Apply`, `See diff`, `Ignore`
  - subtask actions: `View result`, `Convert to edit`, `Ignore`
  - suggestion actions: `Yes`, `Not now`, `Tell me more`
- workspace complexity hidden by default and available via:
  - `Open Full Workspace` toggle
- ambient presence:
  - tray-first app lifecycle + global hotkey overlay
  - close-to-tray behavior and single-instance routing
  - native notification plumbing with quiet mode
- streaming interaction:
  - streaming `send_message_streaming` path (token-by-token UI updates)
  - phrase-chunked streaming TTS with barge-in cancellation
  - partial STT fast-path with Whisper fallback

Not implemented in these phases:
- new backend capabilities
- external browsing or tool-use expansion
- new autonomous loops/chains
- feature-scope expansion beyond interaction redesign

## Environment

Create `jeff/.env` with:

```bash
OPENAI_API_KEY=your_key_here
```

## Local Data Locations

- DB file: `<app_local_data_dir>/jeff_data/jeff.sqlite3`
- task workspaces: `<app_local_data_dir>/jeff_data/tasks/<task-slug>/`
- imported artifacts: `<workspace>/artifacts/<file>`

## Commands

### Install

```bash
cd desktop
npm install
```

### Dev

```bash
cd desktop
npm run tauri dev
```

### Build

```bash
cd desktop
npm run build
cargo build --manifest-path src-tauri/Cargo.toml
```

### Test

```bash
cd desktop
npm run test
cargo test --manifest-path src-tauri/Cargo.toml
```

### Full Phase 10 Gate

```bash
./scripts/phase10_check.sh
```

### Phase 11 Gate

```bash
./scripts/phase11_check.sh
```

### Phase 12 Gate

```bash
./scripts/phase12_check.sh
```

### End-to-End Validation

```bash
./scripts/history_storymap_full_session_check.sh
```

## Manual Try-Use Path

1. Run `cd desktop && npm run tauri dev`.
2. Continue your active task from Home.
3. In companion chat, try:
   - `what's missing from the rubric`
   - `fix this intro`
   - `draft better intro`
4. Use inline action buttons directly from chat cards.
5. Use `Open Full Workspace` only when you want detailed panels/debug tools.

## Docs

- `docs/PHASES.md`
- `docs/BUILD_PLAN.md`
- `docs/ARCHITECTURE.md`
