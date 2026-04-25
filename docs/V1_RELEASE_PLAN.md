# Jeff v1 Release Plan

Status key: `[ ]` pending · `[~]` in progress · `[x]` done

This file is the live tracking document for wrapping up v1 and making it
ready for distribution. Update status fields inline as each milestone
completes. If handing off to Codex mid-milestone, include the milestone
number, the last completed sub-item, and run the regression gate first.

---

## Current state snapshot (2026-04-25, updated)

- Phase checks 17–24: all pass, 371 individual checks, 0 failures
- Backend: `cargo test` clean (166 unit tests pass)
- Frontend: 28 tests pass
- All Phase 23+24 work committed at 81c8e15, pushed to origin master
- PHASES.md: phases 23 and 24 marked complete at ab1a44f
- ARCHITECTURE.md: phases 15–19 documented at 11f732f
- Tauri release build: Jeff.app + Jeff_0.1.0_aarch64.dmg produced locally (unsigned)
- Updater keypair: generated (keys provided to user; not committed)
- dist/jeff-extension-chrome.zip: built locally, gitignored
- Extension install instructions: added to Privacy Center in App.tsx
- GitHub Actions secrets: **not yet configured — human action required**
- Release branch: **does not exist — human action required (M-V1.6)**
- Runtime smoke test: **not yet run — human action required (M-V1.8)**

---

## v1 completion criteria (from PHASES_NEXT.md)

Each criterion maps to a phase. All checks pass; items below are the
verification checklist for the human runtime proof.

- [ ] Installs without a terminal (Phase 18 + 24) → M-V1.4, M-V1.10
- [ ] Launches and recovers silently at login (Phase 19) → M-V1.8
- [ ] Knows active document without folder setup (Phase 20) → M-V1.8
- [ ] User controls for every sensing surface (Phase 21) → M-V1.8
- [ ] Selection capture hotkey reads draft text (Phase 22) → M-V1.8
- [ ] Voice does not interrupt active typing (Phase 22) → M-V1.8
- [ ] Learns user style and rubrics across sessions (Phase 23) → M-V1.8
- [ ] Sees full workload across all tasks (Phase 23) → M-V1.8
- [ ] Surfaces upcoming calendar events (Phase 23) → M-V1.8
- [ ] Applies approved edits in live apps with fallback (Phase 23) → M-V1.8
- [ ] Parallel multi-step work with approval-gated writes (Phase 16) → passes
- [ ] Initiates re-orientation and drift flagging (Phase 15) → passes
- [ ] Updates safely through signed release channel (Phase 24) → M-V1.10

---

## Milestone execution order

```
M-V1.1 → M-V1.2 → M-V1.3 → M-V1.4 → M-V1.9
                                             ↓
                  M-V1.5 → M-V1.7 → M-V1.8 → M-V1.6 → M-V1.10
```

M-V1.1 through M-V1.4: Codex/Claude can execute autonomously.
M-V1.5: Codex runs the command; human stores the keypair output securely.
M-V1.6: Human only — requires Apple Developer credentials and GitHub repo access.
M-V1.7: Codex can package; Chrome Web Store submission is human.
M-V1.8: Human only — requires running the app.
M-V1.9: Codex/Claude — regression gate, must be run after M-V1.1.
M-V1.10: Human — pushes release branch, monitors CI pipeline.

---

## M-V1.1: Commit Phase 23+24 implementation

**Status: [x] done — commit 81c8e15, pushed to origin master**
**Who:** Codex / Claude
**Depends on:** nothing

### Goal

Stage and commit all 23 modified files and the untracked
docs/PHASES_25_26_PLAN.md as a single clean commit representing the
completion of Phases 23 and 24.

### Why this must happen first

The entire Phase 23+24 implementation — user_model.rs, workload.rs,
calendar.rs, live-actions in selection_capture.rs, browser extension
live-apply, CI pipeline, tauri.conf.json distribution config — is
uncommitted. It cannot be recovered, released, or handed to Codex until
it is in git history.

### Files to stage

All modified (M) files from `git status`:
- .github/workflows/release.yml
- browser-extension/selection-capture/background.js
- browser-extension/selection-capture/content.js
- desktop/src-tauri/Cargo.lock
- desktop/src-tauri/Cargo.toml
- desktop/src-tauri/src/calendar.rs
- desktop/src-tauri/src/chat_streaming.rs
- desktop/src-tauri/src/commands.rs
- desktop/src-tauri/src/main.rs
- desktop/src-tauri/src/models.rs
- desktop/src-tauri/src/revision.rs
- desktop/src-tauri/src/selection_capture.rs
- desktop/src-tauri/src/state.rs
- desktop/src-tauri/src/store.rs
- desktop/src-tauri/src/subtask.rs
- desktop/src-tauri/src/user_model.rs
- desktop/src-tauri/src/workload.rs
- desktop/src-tauri/tauri.conf.json
- desktop/src/App.tsx
- desktop/src/tauriClient.ts
- docs/PHASE_23_24_PLAN.md
- scripts/phase23_check.sh
- scripts/phase24_check.sh

Untracked file to stage:
- docs/PHASES_25_26_PLAN.md

### Commit message

```
Phase 23+24: personalization, workload, calendar, live app actions, distribution

Phase 23:
- user_model.rs: user profile table, style signals, delegation patterns,
  rubric injection into chat/revision prompts, "Jeff remembers" panel
- workload.rs: cross-task workload summary, stale-task notifications,
  cross-task collision detection
- calendar.rs: EventKit integration, upcoming event in companion header,
  calendar context in reorientation
- selection_capture.rs: live-app action bridge (apply-edit, apply-fallback,
  receipt logging, anchor validation), browser extension live-apply support

Phase 24:
- tauri.conf.json: universal binary target, signing config, updater endpoint
- release.yml: test → build → sign → notarize → release CI pipeline
- Cargo.toml: tauri-plugin-updater dependency
- main.rs: background update check with Install/Later dialog
- entitlements.plist: hardened runtime entitlements
- scripts/phase23_check.sh, phase24_check.sh: behavioral verification
- PHASES_25_26_PLAN.md: post-v1 phase plan (web grounding, email integration)
```

### Done when

- `git log --oneline -1` shows this commit
- `git status` shows clean working tree
- `git diff HEAD~1 --stat` shows all 24 files

---

## M-V1.2: Update PHASES.md to mark 23+24 complete

**Status: [x] done — commit ab1a44f**
**Who:** Codex / Claude
**Depends on:** M-V1.1

### Goal

Update the status section of docs/PHASES.md to mark Phases 23 and 24
as complete, consistent with all other completed phases.

### Change

In the status list at the top of docs/PHASES.md, after the Phase 22 line:

```
- Phase 23: complete (live app actions, personalization, workload awareness, calendar context)
- **Phase 24: complete (distribution + auto-update)**
```

Remove the `**...**` bold from Phase 22 and move it to Phase 24 (bold =
current last complete phase, consistent with the existing pattern).

### Done when

- docs/PHASES.md status section lists 23 and 24 as complete
- Phase 22 line is not bold; Phase 24 line is bold
- Committed as a follow-on commit to M-V1.1

---

## M-V1.3: Update ARCHITECTURE.md for Phase 23+24

**Status: [x] done — commit 11f732f, layers 15–19 added**
**Who:** Codex / Claude
**Depends on:** M-V1.1

### Goal

ARCHITECTURE.md was last updated at Phase 14. Six phases of new modules
have been added since then. Add accurate entries for each.

### What to add

Add the following numbered entries to the layering section in
docs/ARCHITECTURE.md, after the existing entry 14 (Richer parallel work):

**15. User model layer** (`desktop/src-tauri/src/user_model.rs`)
- `user_profile` table: key-value pairs, incremental update after each session
- signals: writing style (avg sentence length + formality), delegation patterns,
  work rhythm, response length preference, quality rubrics
- `inject_user_profile_context(conn)`: compact summary (< 100 tokens) for
  chat, revision, and reorientation system prompts
- "Jeff remembers" panel: 2–3 active signals, delete per-signal, clear-all

**16. Workload awareness layer** (`desktop/src-tauri/src/workload.rs`)
- `get_workload_summary`: active tasks (focused last 14 days), stale tasks
  (> 14 days), tasks with pending items; cached 5 min in memory
- stale-task notification: one native notification per task per 24 hours
- cross-task collision check: cosine similarity > 0.8 triggers soft notice

**17. Calendar context layer** (`desktop/src-tauri/src/calendar.rs`)
- EventKit integration via macOS-only objc bridge
- polls next 8 hours of events on a 60-second interval
- emits `calendar://next_event` with title + minutes-until to frontend
- all data transient in memory; nothing persisted to SQLite
- toggle gated by `privacy_calendar_context_enabled` in app_settings

**18. Live app actions layer** (`desktop/src-tauri/src/selection_capture.rs`, extended)
- `/apply-edit` route: receives anchor + patch from browser extension,
  validates anchor hash + context match, applies in-place in editor
- `/apply-fallback` route: degraded path when anchor drifts; returns
  guided-apply payload for user-assisted paste
- receipt log: every apply attempt recorded in `live_edit_receipts` table
  (app, document, before/after hash, timestamp, status)
- full approval gate: `approve_live_edit` Tauri command required before
  any extension write; `reject_live_edit` discards without write

**19. Distribution + auto-update** (`tauri.conf.json`, `main.rs`, `.github/workflows/release.yml`)
- universal binary: `universal-apple-darwin` target (arm64 + x86_64)
- CI pipeline: test → build (unsigned) → sign (codesign + hardened runtime)
  → notarize (xcrun notarytool) → release (GitHub Releases + latest.json)
- auto-update: background check on launch via tauri-plugin-updater;
  native Install/Later dialog; signed .app.tar.gz + latest.json feed
- minimum macOS: 13.0 (Ventura), consistent with SMAppService (Phase 19)

### Done when

- All five new entries are present in docs/ARCHITECTURE.md
- Descriptions match the actual code (verify against source files)
- Committed as a follow-on commit to M-V1.1

---

## M-V1.4: Verify Tauri release build locally (unsigned)

**Status: [x] done — Jeff.app (7.8MB arm64) + Jeff_0.1.0_aarch64.dmg produced; human launch verification pending (M-V1.8)**
**Who:** Codex / Claude (build command); Human (app launch verification)
**Depends on:** M-V1.1

### Goal

Run an actual Tauri bundle — not just `cargo build` or `cargo check` —
to confirm the full build pipeline produces a valid .app and .dmg.

The Phase 24 check uses `cargo check` as a build proxy. This milestone
runs the real thing.

### Command

```bash
npm --prefix desktop run tauri -- build --no-sign
```

This skips code signing (which requires Apple Developer credentials) but
produces a fully-linked .app and .dmg.

Expected output path:
```
desktop/src-tauri/target/release/bundle/macos/Jeff.app
desktop/src-tauri/target/release/bundle/dmg/Jeff_0.1.0_aarch64.dmg
```

Note: this takes 5–10 minutes on first run (full release compile).

### Done when

- Build completes with no errors
- Jeff.app exists in target/release/bundle/macos/
- .dmg exists in target/release/bundle/dmg/
- Human: double-clicks the .dmg, drags Jeff to a test location, and
  launches it — confirms app opens without Gatekeeper blocking (expected
  on unsigned build: user must right-click → Open)

### Known issue

Unsigned builds will show a "unidentified developer" Gatekeeper warning.
This is expected. The signed CI build (M-V1.10) will pass Gatekeeper.

---

## M-V1.5: Generate Tauri updater keypair

**Status: [x] done — keypair generated; keys provided to user (see below); not committed**
**Who:** Codex runs command; Human stores output
**Depends on:** M-V1.1

### Goal

Generate the public/private keypair for the auto-update channel.
Without this keypair, the TAURI_PUBLIC_KEY secret cannot be set in
GitHub Actions, and auto-update will not function.

### Command

```bash
npm --prefix desktop run tauri -- signer generate
```

This outputs a public key and private key (base64-encoded).

### What to do with the output

- Public key → store as GitHub Actions secret `TAURI_PUBLIC_KEY`
- Private key → store as GitHub Actions secret `TAURI_PRIVATE_KEY`
- If the tool prompts for a password, store it as `TAURI_KEY_PASSWORD`
- Do NOT commit either key to the repository

### Done when

- Command has been run
- Public and private keys are stored securely outside the repo
- Both values are ready to be entered as GitHub secrets in M-V1.6

---

## M-V1.6: Configure GitHub repository for release

**Status: [ ]**
**Who:** Human only (requires Apple Developer credentials + GitHub admin)
**Depends on:** M-V1.5

### Goal

Configure everything external to the codebase that the CI pipeline needs
in order to produce a signed, notarized, distributable .dmg.

### Prerequisites

- Apple Developer Program membership (paid, $99/year)
- A Developer ID Application certificate in your Apple account
  (distinct from App Store distribution; for direct distribution)
- App Store Connect API key for notarytool
  (create at appstoreconnect.apple.com → Users → Integrations)

### Step-by-step

#### 1. Export Developer ID Application certificate

In macOS Keychain Access:
- Find your "Developer ID Application: [Your Name]" certificate
- Right-click → Export → save as .p12, set a password
- Base64-encode it: `base64 -i certificate.p12 -o certificate.b64`

#### 2. Get your signing identity string

```bash
security find-identity -v -p codesigning | grep "Developer ID Application"
```
Copy the full string: `Developer ID Application: Your Name (TEAMID)`

#### 3. Create a release branch

```bash
git push origin master:release
```

#### 4. Add GitHub Actions secrets

Go to github.com/km31-code/jeff → Settings → Secrets and variables →
Actions → New repository secret for each:

| Secret name                 | Value                                               |
|-----------------------------|-----------------------------------------------------|
| APPLE_CERTIFICATE           | contents of certificate.b64 (base64 .p12)           |
| APPLE_CERTIFICATE_PASSWORD  | the .p12 export password                            |
| APPLE_SIGNING_IDENTITY      | "Developer ID Application: Your Name (TEAMID)"      |
| APPLE_ID                    | your Apple ID email                                 |
| APPLE_APP_PASSWORD          | app-specific password from appleid.apple.com        |
| APPLE_TEAM_ID               | your 10-character team ID (visible in dev portal)   |
| APPLE_PROVIDER_SHORT_NAME   | provider short name from App Store Connect          |
| TAURI_PUBLIC_KEY            | public key from M-V1.5                              |
| TAURI_PRIVATE_KEY           | private key from M-V1.5                             |
| TAURI_KEY_PASSWORD          | keypair password from M-V1.5 (empty string if none) |

#### 5. Verify repository permissions

Under Settings → Actions → General:
- Workflow permissions: "Read and write permissions" must be enabled
  (required for `gh release create` in the release job)

### Done when

- All 10 secrets are saved in GitHub Actions secrets
- `release` branch exists and points to same commit as master
- Repo has write workflow permissions

---

## M-V1.7: Browser extension distribution

**Status: [x] done — dist/jeff-extension-chrome.zip built; install instructions added to Privacy Center; commit 57c3940**
**Who:** Codex packages; Human submits to Chrome Web Store (or documents manual install)
**Depends on:** M-V1.1

### Goal

The Chrome extension at browser-extension/selection-capture/ is complete
and working. It has no distribution path yet. Users need a way to install
it. Two options:

**Option A (preferred): Chrome Web Store**
- Package and submit to Chrome Web Store
- Requires a Chrome developer account ($5 one-time fee)
- Review takes 1–7 days
- Extension will be installable from a URL with one click

**Option B (immediate): packaged .zip with in-app install instructions**
- Package as a .zip for sideloading
- User enables developer mode in Chrome and loads unpacked
- Faster but more friction

### Steps for Option A (Chrome Web Store)

1. Codex: create the submission package:
   ```bash
   cd browser-extension/selection-capture
   zip -r ../../dist/jeff-extension-chrome.zip . --exclude "*.DS_Store"
   ```
2. Human: go to chrome.google.com/webstore/devconsole
3. Human: upload jeff-extension-chrome.zip, fill in listing details,
   submit for review

### Steps for Option B (immediate sideload)

1. Codex: create .zip and verify it exists:
   ```bash
   cd browser-extension/selection-capture
   zip -r ../../dist/jeff-extension-chrome.zip . --exclude "*.DS_Store"
   ls -la ../../dist/jeff-extension-chrome.zip
   ```
2. Codex: verify the install instructions are present in App.tsx
   companion settings section (search for "extension" or "browser" in
   the selection capture settings panel). If instructions are absent,
   add a plain-text note:
   "To install the browser extension: download jeff-extension-chrome.zip,
   open Chrome → chrome://extensions, enable Developer Mode, click
   Load unpacked, and select the unzipped folder."

### Done when

- dist/jeff-extension-chrome.zip exists
- Install instructions are visible from within the app's companion
  settings panel (not just external docs)

---

## M-V1.8: Runtime smoke test (human required)

**Status: [ ]**
**Who:** Human only — requires running the app
**Depends on:** M-V1.4 (local build), M-V1.1

### Goal

Confirm every v1 completion criterion works end-to-end in a running app.
Phase check scripts verify code structure. This confirms actual behavior.

Per CLAUDE.md: "runtime proof (app launches, feature works end to end)"
is non-negotiable before declaring a milestone complete.

### Checklist

Run these in order. Check off each one when confirmed working.

**Onboarding + key management**
- [ ] Fresh launch shows onboarding wizard (not the companion directly)
- [ ] Entering a valid OpenAI API key passes inline validation
- [ ] Keychain entry is written (verify: Security app or `security find-generic-password -s "jeff_api_key"`)
- [ ] Wizard completes and sets onboarding_complete flag
- [ ] "Set up Jeff again" in tray menu re-runs the wizard

**Ambient presence**
- [ ] App starts in system tray with no window stealing focus from frontmost app
- [ ] Cmd+Shift+J summons overlay without the previous app losing focus
- [ ] Cmd+Shift+J again dismisses overlay; previous app still has focus
- [ ] Closing the overlay window (red button) hides it, does not quit process
- [ ] Quit is only accessible from tray menu
- [ ] "Launch at Login" toggle appears in macOS System Settings → Login Items
  after enabling

**Session restore**
- [ ] Quit and relaunch: active task is restored in overlay without focus steal
- [ ] Overlay state (collapsed vs expanded) is restored from prior session

**Active window context**
- [ ] With accessibility permission granted: companion header shows app name
  and document title within 5 seconds of opening a document in Pages or TextEdit
- [ ] Document-switch nudge fires once when switching to a different document
  (does not fire repeatedly for the same document)

**Selection capture**
- [ ] With text selected in TextEdit/Pages, Cmd+Shift+V shows "Captured N words
  from [App]" indicator in companion
- [ ] Captured text is included as context in the next Jeff response
- [ ] Dismiss button clears the indicator and the capture
- [ ] Apps where AX selected text is unavailable show fallback message
  (not a silent failure)

**Voice naturalness**
- [ ] When actively typing, Jeff's spoken response is delayed
- [ ] After 3 seconds of continued typing, response is delivered text-only (no audio)
- [ ] Short responses (< 15 words) are prefixed with a natural interjection
  ("got it", "on it", etc.)
- [ ] Filler phrases ("certainly", "absolutely", "of course") are absent from
  spoken output
- [ ] TTS voice setting in companion changes the voice on next response

**Privacy Center**
- [ ] "What Jeff knows" dashboard is accessible from tray menu
- [ ] All sensing surfaces have working toggles that persist across sessions:
  - [ ] Workspace watcher
  - [ ] Clipboard capture
  - [ ] Active window context
  - [ ] Proactive triggers
  - [ ] User profile memory
  - [ ] Calendar context
- [ ] "Clear active task data" removes chat history and chunks but keeps task record
- [ ] "Clear all Jeff data" returns app to first-run state (no residual data)

**Personalization**
- [ ] After 5 accepted revisions, open "Jeff remembers" panel — at least one
  style signal should appear
- [ ] Entering a quality rubric ("always use bullet points") causes the next
  revision prompt to include it verbatim
- [ ] Deleting a signal from the panel removes it; clear-all empties the panel

**Workload**
- [ ] Workload section shows all tasks focused in the last 14 days
- [ ] Each task shows its last-focused timestamp and pending item count
- [ ] Clicking a task in workload view switches the active task

**Calendar**
- [ ] With calendar permission, companion header shows upcoming event within 10s
- [ ] Re-orientation message references the upcoming event when within 2 hours

**Proactive initiation**
- [ ] After 5+ minutes away from a task, returning shows a re-orientation message
  without asking
- [ ] Drift detection fires when conversation diverges from task goal
- [ ] Quiet mode suppresses all proactive surfaces (audio, overlay, notifications)

**Live app actions (browser extension)**
- [ ] Install the Chrome extension from dist/jeff-extension-chrome.zip
- [ ] With Google Docs open, select a paragraph, press Cmd+Shift+V — indicator
  appears in Jeff with word count and source
- [ ] Ask Jeff to revise the selected text — a live edit proposal card appears
  with preview
- [ ] Click Approve — the extension applies the edit in-place in Google Docs
- [ ] Click Reject — no edit is applied; card dismisses
- [ ] If anchor drifts (edit the doc between proposal and approval), guided-apply
  fallback is shown with reason

**Multi-step parallel work**
- [ ] Ask Jeff to "write an outline and then draft a summary" — subtask chain
  shows intermediate steps with individual progress
- [ ] A file write proposal (if workspace folder set) shows approval card
- [ ] Approve → file is written to workspace folder
- [ ] Reject → no file write, card dismisses
- [ ] Cancel mid-chain → remaining steps skipped, no partial artifacts

**Auto-update** (requires M-V1.10 to be complete first)
- [ ] On next launch after a new version is released, a native dialog appears:
  "Jeff update available — [Install] [Later]"
- [ ] Clicking Install downloads and applies the update; app restarts to new version
- [ ] Clicking Later dismisses the dialog; app continues on current version

---

## M-V1.9: Final regression gate

**Status: [x] done — phases 17–24: 371 checks, 0 failures; cargo test: 166 pass; npm test: 28 pass**
**Who:** Codex / Claude
**Depends on:** M-V1.1

### Goal

Run the full phase check battery + test suite as a single verification
pass. This is the formal programmatic gate for v1.

### Commands

```bash
for i in 17 18 19 20 21 22 23 24; do
  echo "=== phase ${i} ===" && bash scripts/phase${i}_check.sh
done

cargo test --manifest-path desktop/src-tauri/Cargo.toml

npm --prefix desktop test -- --run
```

### Done when

- All 8 phase check scripts report 0 failures
- `cargo test` passes (all unit + integration tests)
- `npm test` passes (all frontend tests)
- No regressions from prior passing state

---

## M-V1.10: Push release branch and ship

**Status: [ ]**
**Who:** Human
**Depends on:** M-V1.6 (secrets), M-V1.9 (regression gate), M-V1.8 (runtime smoke test)

### Goal

Push to the `release` branch to trigger the CI pipeline. Monitor the
full run. Confirm the final GitHub Release contains a signed, notarized
.dmg and a valid latest.json.

### Command

```bash
git push origin master:release
```

### CI pipeline stages to monitor

1. **test** — cargo test + phase17_check.sh + npm test. Must pass before build.
2. **build** — universal binary (arm64 + x86_64) compiled, unsigned .app uploaded.
3. **sign** — Developer ID certificate imported, codesign applied with hardened
   runtime and entitlements.plist, signed .dmg created.
4. **notarize** — xcrun notarytool submits .dmg to Apple, staple ticket applied.
   This step takes 1–10 minutes.
5. **release** — .app.tar.gz signed with updater key, latest.json generated,
   GitHub Release created with Jeff_0.1.0.dmg + latest.json as assets.

### Done when

- All 5 CI jobs complete green
- GitHub Release at github.com/km31-code/jeff/releases shows:
  - Jeff_0.1.0.dmg (signed, notarized)
  - Jeff_0.1.0_universal.app.tar.gz (updater archive)
  - latest.json (with correct version, signature, URL)
- Downloading and opening the .dmg passes Gatekeeper without warning
- Non-technical user can complete onboarding and send a first message

---

## Codex handoff notes

If resuming from Codex, specify:
1. The last completed milestone and the exact sub-item where work stopped.
2. Run `bash scripts/phase24_check.sh` first to confirm the code baseline
   is still green (this also runs the full regression cascade 17→24).
3. For M-V1.1 (commit): use the exact file list and commit message in
   the milestone — do not deviate from it.
4. For M-V1.3 (ARCHITECTURE.md): read desktop/src-tauri/src/user_model.rs,
   workload.rs, calendar.rs, and selection_capture.rs before writing
   descriptions — match actual function signatures.
5. Never start M-V1.10 unless M-V1.6 (secrets) and M-V1.9 (regression gate)
   are both marked done.
6. M-V1.5, M-V1.6, M-V1.8: these require human action. Do not attempt to
   simulate or skip them.

---

## What is explicitly out of scope for v1

These are deferred to Phases 25+26 and are NOT required for this release:

- Firefox and Safari extension support (Phase 25)
- Web search in subtask chains (Phase 25)
- Gmail OAuth and email drafting (Phase 26)
- Windows support (post Phase 26)
- Local/offline model runtime
- Team/multi-user features
- Mobile companion
