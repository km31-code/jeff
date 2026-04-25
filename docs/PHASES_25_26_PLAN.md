# Phases 25 and 26 — Implementation Plan

Status key: `[ ]` pending · `[~]` in progress · `[x]` done

This file is the live tracking document for Phases 25 and 26.
Update status fields inline as each milestone completes.
If handing off to Codex mid-phase, include the milestone number and the
last completed sub-item so it can resume from the right point.

---

## Rationale for this sequencing

Phase 24 shipped a signed, distributed v1. The two highest-leverage post-v1
investments are:

1. **Phase 25 — Web Grounding + Universal Browser Coverage**: Jeff's subtask
   chains are currently limited to local knowledge and ingested workspace files.
   Adding a search step makes Jeff genuinely useful for any research-adjacent
   task (the Georgetown sophomore writing a history paper, the lawyer pulling
   precedent, the PM researching competitors). Universal browser coverage
   completes the Phase 22 selection-capture story, which currently has partial
   browser support.

2. **Phase 26 — Email Integration**: Email is the highest-frequency async work
   surface for the target users. Bringing Jeff into email context (read threads,
   draft replies) directly extends the "already knows your task" and "does parts
   of the work" felt properties into a surface users visit constantly.

Both phases build on existing infrastructure rather than introducing new layers:
Phase 25 extends Phase 16 (subtask chains) and Phase 22 (browser extension).
Phase 26 extends Phase 18 (Keychain management) and Phase 21 (Privacy Center).

Windows support (tracked in the Phase 24 architecture note) is next after Phase 26.

---

## Non-negotiable constraints (inherited from all prior phases)

- Local-first. No user data leaves the device unless the user explicitly
  connects an external service (search API, Gmail).
- No silent writes. All file writes and email sends remain approval-gated.
- New sensing surfaces are opt-in, surfaced in the Privacy Center.
- Every phase ships with a `scripts/phaseN_check.sh`.
- Preserve all five felt properties in VISION.md.

---

## Phase 25: Web Grounding + Universal Browser Coverage

Four milestones:

1. M25.1: Search API tool in subtask chains
2. M25.2: Universal browser coverage (Firefox + Safari)
3. M25.3: Cross-browser live-apply parity
4. M25.4: phase25_check.sh

---

### M25.1 — Search API Tool in Subtask Chains

**Status: [ ]**

Goal: give subtask chains a web search step so Jeff can research and retrieve
external information as part of multi-step parallel work.

Scope is subtask chains only. Direct chat does not gain search capability in this
phase — searches are always part of an approved subtask, never ambient background
queries.

**Provider: Brave Search API** (reference implementation). Simple REST endpoint,
no OAuth, API key stored in Keychain alongside the OpenAI key. A second provider
can be added via the Phase 17 provider abstraction seam without touching call sites.

**Backend: new `search.rs` module**

Responsibilities:
- `search_web(query: &str, max_results: usize) -> Result<Vec<SearchResult>>`
  — calls `https://api.search.brave.com/res/v1/web/search` with the user's API key.
  Returns up to `max_results` results (default: 5, hard cap: 10).
- `SearchResult { title: String, url: String, snippet: String, retrieved_at: String }`
- Rate limiting: no more than 10 search calls per subtask chain. Counter stored
  per-chain in memory. Exceeding the limit returns `Err("search_limit_reached")`.
- Results are never stored in SQLite. They are appended to the subtask step
  output in memory and included in the step's context for subsequent chain steps.
- Graceful degradation: if the API key is absent or the call fails, the subtask
  step receives an error result and continues with what it has. No chain abort.

**Integration point: subtask.rs step execution**

The subtask chain planner (existing in `subtask.rs`) can emit a step with
`execution_type: "web_search"`. When the executor encounters this type:
1. Extracts `query` from the step's structured arguments.
2. Calls `search::search_web(query, 5)`.
3. Appends formatted results to the step output: source title, URL, snippet.
   Format: plain text block, not Markdown (for consistent downstream injection).
4. Passes results as context to the next chain step.

The chain planner prompt is updated with one instruction: "You may use a
web_search step when external information would help. Limit to 3 search steps
per chain. Always cite the source URL in your final output."

**Privacy and control:**
- Toggle: `privacy_web_search_enabled` in app_settings. Default: false (opt-in).
- Toggle surfaced in Privacy Center (Phase 21 dashboard) under a new
  "Web search" row. Changing the toggle takes effect immediately.
- When the toggle is off, `execution_type: "web_search"` steps are skipped and
  the step output records "web search disabled by user."
- API key setup: a "Set up web search" entry in companion settings opens a text
  field for the Brave Search API key. Stored in Keychain under key
  `jeff_brave_search_api_key`. Validated with a single test query on save.
  Error message if key is invalid: actionable, not generic.

Files to create:
- `desktop/src-tauri/src/search.rs` — new module

Files to modify:
- `desktop/src-tauri/src/subtask.rs` — add web_search execution_type handler;
  inject search context into subsequent step prompts; enforce per-chain limit
- `desktop/src-tauri/src/state.rs` — add per-chain search counter field
  (or pass as mutable argument through chain execution)
- `desktop/src-tauri/src/commands.rs` — add `set_search_api_key(key)`,
  `get_search_api_key_status` commands (status: present/absent, not the key itself)
- `desktop/src-tauri/src/main.rs` — register new commands; register search.rs module
- `desktop/src-tauri/Cargo.toml` — confirm `reqwest` already present (it is from
  Phase 12/16); no new HTTP dependency needed
- `desktop/src/App.tsx` — "Web search" toggle and API key setup UI in settings panel
- `desktop/src/tauriClient.ts` — typed wrappers for new commands

Verification:
- With toggle on and a valid API key, a subtask chain containing a web_search step
  returns results with source URLs in the step output.
- With toggle off, web_search steps are skipped with a user-visible note.
- Per-chain limit of 10 is enforced; exceeding it logs `search_limit_reached`.
- Invalid or absent API key: actionable error, chain continues with no results.
- No search result text is written to SQLite.
- `cargo test --manifest-path ... search` passes.

---

### M25.2 — Firefox Browser Extension

**Status: [ ]**

Goal: port the existing Chrome extension (Phase 22) to Firefox using
Manifest V2, which is the current stable Firefox extension API.

The Chrome extension in `browser-extension/selection-capture/` uses Manifest V3.
Firefox requires MV2 for full background script support. The logic is identical;
only the manifest and a few API surface differences need to be handled.

Files to create:
- `browser-extension/selection-capture-firefox/` — new directory with Firefox variant
  - `manifest.json` — MV2 manifest; `background` uses `scripts` array, not `service_worker`
  - `background.js` — copy of Chrome background.js with browser API substitutions:
    `chrome.runtime.*` → `browser.runtime.*` (or polyfill)
  - `content.js` — identical to Chrome content.js; Firefox supports the same Selection API
  - `popup.html`, `popup.js` — same UI, ported to use `browser.*` API

Manifest V2 specifics:
```json
{
  "manifest_version": 2,
  "background": { "scripts": ["background.js"], "persistent": false },
  "permissions": ["activeTab", "storage", "tabs"],
  "content_scripts": [{ "matches": ["<all_urls>"], "js": ["content.js"] }]
}
```

Bridge connection: the Firefox extension connects to the same local HTTP bridge
server as Chrome (same port, same token, same endpoints). No backend changes needed.

Per-browser opt-in: add `browser_extension_firefox_enabled` to app_settings.
Default: false. Companion settings shows "Firefox extension" row with install
instructions link when the user toggles it on (open addons.mozilla.org to install
the packaged extension). Instructions shown in companion, not external docs.

Packaging: `zip -r jeff-firefox.zip browser-extension/selection-capture-firefox/`.
Firefox addons require a zip; Chrome requires an unpacked dir or crx.

Files to modify:
- `desktop/src/App.tsx` — Firefox extension toggle + instructions in settings panel
- `desktop/src/tauriClient.ts` — `browser_extension_firefox_enabled` setting wrapper
- `desktop/src-tauri/src/store.rs` — add `browser_extension_firefox_enabled` setting key

Verification:
- Firefox extension installed in Firefox connects to the bridge and captures selected text.
- Selection capture indicator in companion shows app name and word count.
- Provenance (browser: Firefox, tab URL, title) is included in captured context.
- Per-browser opt-in toggle persists across sessions.
- Chrome extension continues to work unchanged (regression guard).

---

### M25.3 — Safari Web Extension + Cross-Browser Live-Apply Parity

**Status: [ ]**

Goal: add Safari support via Apple's Safari Web Extension format and ensure
live-apply (Phase 23 M23.5) works identically across Chrome, Firefox, and Safari.

**Safari extension:**

Safari Web Extensions use the same WebExtensions API as Chrome/Firefox but are
packaged as a macOS app. Conversion is handled by Xcode's built-in converter:
```
xcrun safari-web-extension-converter browser-extension/selection-capture \
  --project-location browser-extension/ \
  --app-name "JeffBridge" \
  --bundle-identifier com.jeff.bridge.safari
```
This generates `browser-extension/JeffBridge/` — an Xcode project containing
the extension. The JS logic is identical to the Chrome version.

Safari requires the extension host app to be signed with the same Developer ID
as Jeff. The Xcode project uses the same signing identity from CI secrets.

The Safari extension is distributed bundled with the main Jeff app (not separately).
Users enable it from Safari Preferences → Extensions → Jeff Bridge.

Per-browser opt-in: add `browser_extension_safari_enabled` to app_settings.
Companion settings shows instructions to enable the extension in Safari.

**Cross-browser live-apply parity:**

The `/apply-edit` bridge endpoint (Phase 23 M23.5) was built for Chrome.
Firefox and Safari need the same polling loop for approval events.

Changes to background.js (Chrome) and background.js (Firefox copy):
- Already polls `/pending-approval/<token>` for the Chrome path.
- Confirm the same polling works for Firefox MV2 background scripts — it does,
  since `browser.alarms` can replace `setInterval` for background persistence.
  Firefox version uses `browser.alarms.create("poll", { periodInMinutes: 0.05 })`.

Safari background service: Safari MV3 uses service workers. The Xcode-converted
extension uses the same approach as Chrome (service worker + alarm). No changes
needed to the bridge server; Safari's service worker calls the same endpoints.

Files to create:
- `browser-extension/JeffBridge/` — generated by safari-web-extension-converter
  (committed as-is; Xcode project file is in the repo)

Files to modify:
- `browser-extension/selection-capture-firefox/background.js` — use browser.alarms
  for polling instead of setInterval (MV2 background persistence)
- `desktop/src/App.tsx` — Safari extension toggle + enable instructions in settings panel
- `desktop/src/tauriClient.ts` — `browser_extension_safari_enabled` wrapper
- `desktop/src-tauri/src/store.rs` — add `browser_extension_safari_enabled` setting key
- `desktop/src-tauri/tauri.conf.json` — include JeffBridge.app in bundle resources
  if Safari extension is distributed with the main app

Verification:
- Safari extension enabled in Safari Preferences; selecting text in a supported
  web editor and pressing capture hotkey delivers text to Jeff.
- Live-apply works in Safari with the same preview + approval + anchor-validation
  path as Chrome.
- Firefox live-apply polling works correctly with browser.alarms.
- All three browsers (Chrome, Firefox, Safari) show the selection indicator with
  provenance (browser name, URL, word count).
- No new backend routes needed; bridge server unchanged.

---

### M25.4 — phase25_check.sh

**Status: [ ]**

Write `scripts/phase25_check.sh` covering:

Search API checks:
- `search.rs` module exists with `search_web` function signature
- `execution_type: "web_search"` handler in subtask.rs
- Per-chain search limit enforced (grep for limit constant)
- `privacy_web_search_enabled` toggle in app_settings store
- `set_search_api_key` and `get_search_api_key_status` commands registered
- No SQLite write path in search.rs (confirm no INSERT in search module)
- `browser_extension_firefox_enabled` setting key in store.rs
- Firefox extension manifest.json exists and is MV2
- `browser_extension_safari_enabled` setting key in store.rs
- JeffBridge Xcode project directory exists (or safari extension marker file)

Behavioral tests:
- `cargo test --manifest-path desktop/src-tauri/Cargo.toml search` passes
- `npm --prefix desktop test -- --run` passes
- `bash scripts/phase24_check.sh` passes (regression guard)

---

## Phase 26: Email Integration

Four milestones:

1. M26.1: OAuth + secure token management (Gmail)
2. M26.2: Email context in active task
3. M26.3: Email drafting with approval
4. M26.4: phase26_check.sh

Design constraints for the whole phase:
- **No auto-send under any circumstances.** Jeff may only create drafts in Gmail.
  Sending requires the user to open Gmail and click Send themselves.
- **Read scope is limited.** OAuth scope: `gmail.readonly` + `gmail.compose`.
  No `gmail.send`. No `gmail.modify` (no label changes, no deletions).
- **No email content stored in SQLite.** Thread data is transient in memory,
  scoped to the session. Cleared on app quit.
- **One provider first.** Gmail only. Other providers (Outlook, Apple Mail) are
  deferred until the Gmail path is stable.

---

### M26.1 — Gmail OAuth + Secure Token Management

**Status: [ ]**

Goal: let the user connect their Gmail account with a minimal read+compose scope,
store tokens securely in Keychain, and revoke on clear-all-data.

**OAuth flow:**

Jeff uses the OAuth 2.0 Authorization Code flow with PKCE (no client secret
needed for a desktop app). The redirect URI is a localhost loopback:
`http://127.0.0.1:PORT/oauth/callback` where PORT is a random ephemeral port
chosen at connection time.

Flow steps:
1. User clicks "Connect Gmail" in companion settings.
2. Backend: `gmail.rs::start_oauth_flow()` — chooses ephemeral port, builds the
   authorization URL with PKCE challenge, spawns a one-shot HTTP listener on that port.
3. Jeff calls `tauri::Shell::open()` to open the authorization URL in the user's
   default browser. User authenticates and grants scopes in the browser.
4. Google redirects to `http://127.0.0.1:PORT/oauth/callback?code=...`.
   The one-shot listener captures the code, exchanges it for tokens via
   `POST https://oauth2.googleapis.com/token`, then closes the listener.
5. Tokens (`access_token`, `refresh_token`, `expires_at`) stored in Keychain:
   - `jeff_gmail_access_token`
   - `jeff_gmail_refresh_token`
   - `jeff_gmail_token_expires_at` (ISO 8601 string)
6. Companion settings updates to show "Gmail connected — [Disconnect]".

Token refresh: before every Gmail API call, check `expires_at`. If < 5 minutes
remaining, call `POST https://oauth2.googleapis.com/token` with the refresh token
to get a new access token. Update Keychain. Silent, never user-facing unless the
refresh fails (in which case: actionable "Gmail session expired — reconnect" message).

Disconnect / revoke:
- "Disconnect" in companion settings calls `gmail.rs::revoke_oauth()`:
  `POST https://oauth2.googleapis.com/revoke?token=<access_token>`
  Then clears all three Keychain entries and sets `gmail_connected = false` in
  app_settings.
- The existing clear-all-data path (Phase 21) must also revoke and clear tokens.
  Add a `gmail.rs::revoke_and_clear()` call to the clear-all-data command in
  commands.rs.

Privacy Center (Phase 21 dashboard): add a new "Email context" row:
- Status: connected (shows Gmail address) | not connected.
- Toggle: `privacy_email_context_enabled`. Default: false.
- Connect / Disconnect button.

Google API project setup note (for the developer — not committed to code):
- OAuth client ID and client secret (or client ID only for PKCE) configured in
  a Google Cloud project with Gmail API enabled.
- Client ID committed to source as a non-secret constant (this is standard practice
  for OAuth public clients). Client secret is NOT used in PKCE flow.

Files to create:
- `desktop/src-tauri/src/gmail.rs` — new module

Files to modify:
- `desktop/src-tauri/src/commands.rs` — add `start_gmail_oauth`, `disconnect_gmail`,
  `get_gmail_connection_status` commands; add gmail revoke to clear-all-data path
- `desktop/src-tauri/src/main.rs` — register commands; register gmail.rs module
- `desktop/src-tauri/src/store.rs` — add `gmail_connected`, `gmail_address`,
  `privacy_email_context_enabled` setting keys
- `desktop/src/App.tsx` — Gmail connect/disconnect UI in settings; Privacy Center row
- `desktop/src/tauriClient.ts` — typed wrappers

Verification:
- OAuth flow opens browser, user authorizes, companion updates to "Gmail connected."
- Tokens are in Keychain under the expected keys (verify with `security find-generic-password`).
- Disconnect revokes token at Google and clears Keychain entries.
- Clear-all-data revokes Gmail token and resets `gmail_connected` to false.
- `privacy_email_context_enabled` toggle persists across sessions.
- `cargo test --manifest-path ... gmail` passes.

---

### M26.2 — Email Context in Active Task

**Status: [ ]**

Goal: surface recent Gmail threads relevant to the active task as context for
Jeff's responses, without storing any email content in SQLite.

**Relevance matching:**

On task load (or on explicit "check email" request from the user), Jeff fetches
recent Gmail threads using a search query derived from the active task:
- Primary query: task title words (stop-words removed) joined with OR.
  Example: active task "Marketing deck for Q3" → Gmail query `marketing deck Q3`.
- Secondary: workspace folder filename stems if folder is set.
- Limit: last 7 days, max 5 threads. If no threads match, no error — context is
  simply absent.

Fetch path (`gmail.rs::fetch_relevant_threads(task_title, workspace_files)`):
1. `GET https://gmail.googleapis.com/gmail/v1/users/me/threads?q=<query>&maxResults=5`.
2. For each thread ID returned: `GET .../threads/<id>?format=metadata` — fetch
   subject + sender + date from headers only. **Do not fetch body text** unless the
   user explicitly asks Jeff to "read" a specific thread.
3. Store as `Vec<ThreadSummary { id, subject, from, date }>` in a
   `GmailState { relevant_threads, fetched_at }` mutex in AppState.

LLM context injection: when `privacy_email_context_enabled` is true and
`GmailState.relevant_threads` is non-empty, prepend a compact block to chat and
reorientation system prompts:
```
recent email threads for this task:
- "{subject}" from {from} ({relative_date})
- ...
(ask me to read a thread for the full content)
```
This block is under 50 tokens. It tells Jeff that relevant email exists without
exposing body content by default.

Full thread read: if the user asks Jeff to "read" or "summarize" a specific thread
by subject or sender, Jeff calls `gmail.rs::fetch_thread_body(thread_id)` which
fetches the thread's full text content. Body text is passed to the LLM in that
single turn's context. It is not stored in SQLite.

Polling: thread relevance is fetched once per task load and once per hour if the
task remains active. No continuous background polling.

Companion display: in the companion header or a collapsible section (below calendar,
above workload), show "N email threads" when threads exist. Clicking expands to
show subject + sender for each. This is read-only display; action (draft reply)
is in M26.3.

Files to modify:
- `desktop/src-tauri/src/gmail.rs` — add `fetch_relevant_threads`, `fetch_thread_body`
- `desktop/src-tauri/src/state.rs` — add `GmailState` mutex to AppState
- `desktop/src-tauri/src/commands.rs` — add `get_gmail_relevant_threads`,
  `get_gmail_thread_body` commands
- `desktop/src-tauri/src/main.rs` — register commands; spawn hourly thread refresh task
- `desktop/src-tauri/src/proactive.rs` — inject thread summaries into reorientation prompt
- `desktop/src-tauri/src/chat.rs` — inject thread summaries into chat system prompt
- `desktop/src/App.tsx` — email context section in companion
- `desktop/src/tauriClient.ts` — typed wrappers

Verification:
- With Gmail connected, toggle on, and an active task whose title matches recent emails,
  the companion shows relevant thread count within 10 seconds of task load.
- LLM response to "what emails are related to this task?" correctly lists subject + sender.
- Fetching full thread body on request returns content to the LLM; content is not in SQLite.
- With toggle off, no Gmail API calls are made and no email context appears.
- Privacy Center row shows correct connected status and thread count.

---

### M26.3 — Email Drafting with Approval

**Status: [ ]**

Goal: Jeff can draft an email reply on the user's behalf. The draft is saved to
Gmail Drafts. The user sends it from Gmail. Jeff never sends email autonomously.

**Draft flow:**

User can ask Jeff: "Draft a reply to [subject/sender] about [topic]" or Jeff can
proactively suggest a draft when a relevant thread is open in context.

1. Jeff generates draft text using the LLM, incorporating:
   - The thread context (from M26.2 — full body fetched if not already in context).
   - The active task context and user profile signals (writing style, rubrics).
2. Companion view shows a draft card:
   - "Draft reply to [sender] — [subject]"
   - Full draft text in an editable textarea (user can edit before saving).
   - Two buttons: "Save to Gmail Drafts" and "Discard".
3. "Save to Gmail Drafts": calls `gmail.rs::create_draft(to, subject, body)`.
   - `POST https://gmail.googleapis.com/gmail/v1/users/me/drafts` with the MIME message.
   - On success: companion shows "Saved to Gmail Drafts — open Gmail to send." with
     a button that opens `https://mail.google.com/mail/u/0/#drafts` in the browser.
   - On failure: actionable error. Draft text remains in the textarea.
4. "Discard": clears the draft card. No API call.

**No auto-send guarantee enforcement:**
- `gmail.rs` has no `send_email` function. The only write operation is `create_draft`.
- The OAuth scope `gmail.compose` allows draft creation only.
  (`gmail.send` is not requested in M26.1 and is not added here.)
- A code comment at the top of `gmail.rs`: `// no send path exists by design. drafts only.`

**Draft quality:**
- The draft generation prompt includes the user's quality rubrics (from Phase 23
  personalization) and a brevity instruction: keep replies concise unless the thread
  warrants length.
- User edits in the textarea before saving are not fed back as personalization signals
  (email is a privacy-sensitive surface; personalization from email content is explicitly
  out of scope).

Files to modify:
- `desktop/src-tauri/src/gmail.rs` — add `create_draft(to, subject, body)` function
- `desktop/src-tauri/src/commands.rs` — add `create_gmail_draft(to, subject, body)` command
- `desktop/src-tauri/src/main.rs` — register command
- `desktop/src/App.tsx` — draft card UI, editable textarea, save/discard buttons,
  "open Gmail Drafts" link
- `desktop/src/tauriClient.ts` — typed wrapper

Verification:
- Asking Jeff to draft a reply generates a draft card with editable text.
- Clicking "Save to Gmail Drafts" creates a draft in Gmail (visible in Gmail Drafts).
- "Open Gmail to send" button opens the Gmail Drafts URL in the browser.
- `gmail.rs` has no function named `send` or `send_email`.
- Draft text is not stored in SQLite.
- Discard clears the card with no side effects.

---

### M26.4 — phase26_check.sh

**Status: [ ]**

Write `scripts/phase26_check.sh` covering:

OAuth checks:
- `gmail.rs` module exists
- `start_gmail_oauth` command registered in main.rs
- `disconnect_gmail` command registered
- `get_gmail_connection_status` command registered
- Keychain write calls present in `gmail.rs` for access_token, refresh_token, expires_at
- Revocation call present in `gmail.rs` (POST to /revoke endpoint)
- Gmail revoke added to clear-all-data path in commands.rs
- `gmail_connected`, `privacy_email_context_enabled` keys in store.rs

Email context checks:
- `fetch_relevant_threads` function in gmail.rs
- `GmailState` struct in state.rs
- Thread summary injection in chat.rs system prompt
- Thread summary injection in proactive.rs reorientation prompt
- No SQLite INSERT in gmail.rs (grep for INSERT in that file — must be absent)
- Hourly refresh task spawned in main.rs

Email drafting checks:
- `create_draft` function in gmail.rs
- `create_gmail_draft` command registered
- No `send_email` or `send_message` function in gmail.rs
- Draft card render in App.tsx
- "open Gmail Drafts" link in App.tsx

Behavioral tests:
- `cargo test --manifest-path desktop/src-tauri/Cargo.toml gmail` passes
- `npm --prefix desktop test -- --run` passes
- `bash scripts/phase25_check.sh` passes (regression guard)

---

## Milestone execution order

```
M25.1 → M25.2 → M25.3 → M25.4
                              ↓
              M26.1 → M26.2 → M26.3 → M26.4
```

M25.2 (Firefox) and M25.3 (Safari) are sequential because M25.3 adds
cross-browser live-apply parity that depends on M25.2's Firefox background
script pattern being settled first.

M26.2 and M26.3 are strictly sequential: email context must be fetched and
injected before drafting can reference thread content.

---

## Handoff notes for Codex resume

If resuming from Codex, specify:
1. The last completed milestone number and sub-item.
2. The exact file currently being edited.
3. Any deviation from this plan discovered during implementation.
4. Run `scripts/phase24_check.sh` first to confirm baseline is green.

Do not start a new milestone until the previous one's verification items all pass.

Phase 25 can begin as soon as this plan is approved. Phase 26 begins only after
M25.4 (phase25_check.sh) passes.
