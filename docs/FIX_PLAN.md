# Jeff — Fix Plan

Four confirmed bugs, one voice gap relative to the vision. Each section
names the root cause, the exact change, and the files touched. No code
changes happen until this plan is approved.

---

## Issue A — Full Workspace Doesn't Sync When Overlay Creates a Task

### What you saw
Status IDLE, "no active task" in the workspace even after the overlay created
a task via the Finder doc-switch banner and you sent a message successfully.

### Root cause
`set_active_task` in `commands.rs` writes to SQLite and starts the watcher,
but emits no cross-window event. The workspace window (App.tsx) only reads
the active task at mount time (`refreshShellState`). If the overlay creates
or switches a task, App.tsx never finds out.

### Exact fix
**`commands.rs`** — after the `set_active_task` command writes the task and
the `switch_active_task_from_companion` command writes the task, emit a
`task://active-changed` Tauri event to all windows.

```rust
// in set_active_task (needs AppHandle added to signature):
let _ = app.emit("task://active-changed", serde_json::json!({ "task_id": task.id }));

// same in switch_active_task_from_companion:
let _ = app.emit("task://active-changed", serde_json::json!({ "task_id": new_task.id }));
```

**`App.tsx`** — add one `useEffect` that listens for `task://active-changed`
and calls `refreshShellState()`. This runs for the lifetime of the workspace
window.

### Vision alignment
"Already knows your task. When you return, Jeff knows what you're working on."
Currently the workspace window is stale after the overlay switches context.

### Files
- `desktop/src-tauri/src/commands.rs`
- `desktop/src/App.tsx`

---

## Issue B — New Files Added to Watched Folder Not Retrieved

### What you saw
Added `test_jeff_note.txt` to the connected folder. Step 13 query returned
"retrieved context does not provide information about the deadline."

### Root cause — two parts

**Part 1: Wrong folder in test guide (test issue, not code)**
Step 12 in TESTING_GUIDE.md says to create the file at `~/Desktop/test_jeff_note.txt`.
That only works if the watched folder IS the Desktop. The file must be created
inside the exact folder chosen in Step 7 of onboarding. The testing guide
will be corrected.

**Part 2: Doc-switch task doesn't inherit folder path from Finder context**
When the user opens a folder in Finder and Jeff's doc-switch banner fires,
`handleStartTaskFromDocumentTitle` creates a new task and calls `setActiveTask`.
The backend then runs `ensure_workspace_awareness_for_task`, which falls back
to the global `preferred_workspace_folder`. But the folder the user opened in
Finder is what should be watched — and the overlay has no way to pass that
path because `active_context` only carries app name and document title,
not a filesystem path.

The symptom: the task is created and the `preferred_workspace_folder` IS
watched, but if the user then adds a file to a different folder (the one
they opened in Finder), the watcher misses it.

**Part 3: No visible watcher feedback in the overlay**
The overlay shows no indication of whether the watcher is running or which
folder is being watched. If ingest fails silently, the user has no way to
know. The full workspace shows watcher status; the overlay does not.

### Exact fix

**`Overlay.tsx`** — add a watcher status line in the expanded view. After the
task label (already shows task title), show one of:
- `watching [folder name]` when the watcher is running for the active task
- `no folder connected · connect` (button) when it is not

This requires calling `getWatcherStatus(taskId)` on task switch (already done
in `refreshRecentlyLearned` in App.tsx — port the same call to the overlay's
`refreshMessages` flow).

**`tauriClient.ts`** — already exports `getWatcherStatus`; no changes needed.

**`watcher.rs`** — after `auto_ingest_file_for_task` succeeds, emit a
`workspace://file-indexed` event with `{ task_id, file_name }`. The overlay
subscribes to this and briefly shows "just indexed: [filename]" so the user
can confirm ingest worked.

**`TESTING_GUIDE.md`** — Step 12 updated: create the test file inside the
folder chosen at Step 7, not at `~/Desktop`.

### Vision alignment
"Already knows your task." Jeff can't know what's in your files if the watcher
is silent about what it ingested. Visible feedback closes the loop.

### Files
- `desktop/src/Overlay.tsx` (watcher status line + `workspace://file-indexed` listener)
- `desktop/src-tauri/src/watcher.rs` (`workspace://file-indexed` emission)
- `desktop/docs/TESTING_GUIDE.md` (Step 12 correction)

---

## Issue C — "What am I currently working on?" Returns Task Folder, Not Active Document

### What you saw
With a PDF open in Preview, asking Jeff "what am I currently working on?"
returns the jeff-v1-smoke folder/task name, not the PDF.

### Root cause
The active window context ("User's active app: Preview. Document: [PDF name].")
is injected as a prefix in the **system prompt** via `build_system_prompt`.
The **user prompt** ends with:

> "Answer strictly from retrieved context and any active-window title or
> selected-text context in the system prompt."

When the user asks "what am I currently working on?", the retrieved chunks
are about the task's files (which were ingested from the folder) and the task
summary says the task title. The LLM correctly follows "strictly from retrieved
context" and returns the task/folder name. The active window context is in
the system prompt but gets drowned out by the task summary + chunks in the
user prompt.

Two compounding problems:
1. The active window context is a short phrase buried in the system prompt;
   the task summary and retrieved chunks in the user prompt are longer and
   more detailed.
2. The word "strictly" instructs the model to ignore its general reasoning
   about the question, so it anchors to the task summary.

### Exact fix

**`chat.rs` — `build_user_prompt`** — add `active_context: Option<&str>`
parameter and include it in the prompt as a clearly labeled section between
the task summary and the user query:

```
Task Summary:
{task_summary}

Active Window:
{active_context}  ← new section, present only when Some

User Query:
{message}

Retrieved Context Chunks:
{chunks}
```

**`chat.rs` — `GROUNDING_SYSTEM_PROMPT`** — change the last sentence from
"Answer strictly from retrieved context and any active-window title..." to:

> "For questions about what the user is currently doing, the Active Window
> section is the primary signal. For questions about the task, use retrieved
> context. Be concise. One to three sentences unless asked for more. No filler."

**`chat_streaming.rs`** — `run_llm_stream` calls `build_user_prompt` with
only `(message, context_pack)`. Update to pass `active_ctx` through.

**`chat.rs` — `send_message_for_task`** — already receives `active_context`;
pass it through to `build_user_prompt`.

### Vision alignment
"Already knows your task." Jeff is watching your screen and knows what's open.
But it only helps if the LLM actually uses that information when it's the
most relevant signal for the question asked.

### Files
- `desktop/src-tauri/src/chat.rs`
- `desktop/src-tauri/src/chat_streaming.rs`

---

## Issue D — Interactive Voice (TTS + Barge-in) Is Not in the Overlay

### What you saw
Overlay voice = record → tap to stop → transcription sent → text response.
No audio playback of Jeff's response. No ability to interrupt Jeff mid-sentence.
No continuous listening.

### Root cause
The full interactive voice system was built in App.tsx (full workspace):
- `scheduleStreamTtsPlayback` / `stopStreamingTtsPlayback` — phrase-ordered TTS queue
- `EVENT_TTS_CHUNK` listener — receives streamed audio phrases from backend
- `tryStartPartialStt` / `stopPartialStt` — Web Speech API with interim results
- `interruptCurrentInteraction` — barge-in while Jeff is speaking

The overlay (Overlay.tsx) has none of these. It only has manual tap-to-record
and manual tap-to-stop.

The vision states: "Can be interrupted and can interrupt you. Mid-sentence,
either direction." That requires TTS playback + barge-in in the primary surface,
which is the overlay.

### Exact fix (three sub-steps, implement together)

**Sub-step D1: TTS playback in overlay**
Port the streaming TTS queue from App.tsx to Overlay.tsx.

New refs in overlay:
```tsx
const ttsActiveTurnIdRef = useRef<string | null>(null);
const streamTtsQueueRef = useRef<Map<number, { audio: HTMLAudioElement; url: string }>>(new Map());
const streamTtsCurrentRef = useRef<HTMLAudioElement | null>(null);
const streamTtsNextPhraseRef = useRef<number>(0);
```

New `scheduleStreamTtsPlayback` function — identical logic to App.tsx. Gated
on `!ambient?.quiet_mode`.

New `stopStreamingTtsPlayback` function — drain queue, revoke URLs, null refs.

Add `EVENT_TTS_CHUNK` listener in the existing streaming `useEffect` block.
When the streaming turn starts (`sendMessageStreaming`), set
`ttsActiveTurnIdRef.current = turnId`. When `finalizeStreamingTurn` runs,
`stopStreamingTtsPlayback()` only if the turn was cancelled (not on normal
complete, so late-arriving chunks still play).

**Sub-step D2: Barge-in from overlay**

New `stopAndBargeIn` async function in overlay:
```tsx
async function stopAndBargeIn() {
  stopStreamingTtsPlayback();
  if (streamingTurnIdRef.current) {
    await cancelStreamingTurn(streamingTurnIdRef.current, "user_barge_in").catch(() => undefined);
  }
}
```

Tie this to the mic button: if Jeff is currently speaking (TTS playing), tapping
the mic calls `stopAndBargeIn()` before starting the new recording. This delivers
the "interrupt mid-sentence" property.

**Sub-step D3: Partial STT in overlay (continuous listening)**

The Web Speech API partial STT is already built in App.tsx as `tryStartPartialStt`.
Port it to the overlay:
- After `sendMessageStreaming` succeeds, start listening for the next voice
  input automatically (continuous mode) using Web Speech API.
- `onresult` with confidence >= 0.7 calls `stopAndBargeIn()` then submits
  the interim transcript as a new message.
- Canceled when TTS finishes (switch back to idle).

This delivers the "neither party waits for the other to finish" property from
the vision.

### What this does NOT include (explicitly out of scope for this fix)
- Jeff initiating conversation unprompted (that is the proactive/reorientation
  system already built in phases 13/15)
- Calendar-context voice nudges
- Hotword detection ("Hey Jeff") — requires a native always-listening process

### Vision alignment
This is the single most important gap between what's built and the vision.
"Can be interrupted and can interrupt you. Mid-sentence, either direction."
A text interface with a mic button is a phone-call model, not a coworker model.
D1 makes Jeff audibly present. D2+D3 make it genuinely interruptible.

### Files
- `desktop/src/Overlay.tsx` (TTS refs, scheduleStreamTtsPlayback, stopStreamingTtsPlayback, stopAndBargeIn, partial STT)
- `desktop/src/streamClient.ts` (EVENT_TTS_CHUNK — already exists, just needs importing in overlay)

---

## Summary Table

| # | Issue | Root Cause | Files Changed |
|---|-------|-----------|---------------|
| A | Workspace stale after overlay creates task | No cross-window event on active task change | `commands.rs`, `App.tsx` |
| B | New files not retrieved | No watcher feedback; test guide put file in wrong place | `Overlay.tsx`, `watcher.rs`, `TESTING_GUIDE.md` |
| C | Screen reading context doesn't answer "what am I doing?" | Active context buried in system prompt, "strictly" overrides it | `chat.rs`, `chat_streaming.rs` |
| D | No TTS/barge-in in overlay | Full voice system only in App.tsx, not overlay | `Overlay.tsx`, imports from `streamClient.ts` |

## Implementation Order

1. A (simplest — 2 emits + 1 listener, fully isolated)
2. C (prompt-only change, no structural risk)
3. B (watcher feedback + testing guide fix)
4. D (largest — D1 first, then D2, then D3)

Each step compiles and passes `tsc --noEmit` before moving to the next.
After all four are done: commit, then run the full TESTING_GUIDE.md flow.
