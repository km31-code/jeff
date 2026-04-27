# Jeff — Testing Guide

This guide has two parts.

**Part 1 — User Experience Tests** (no terminal required). Tests the five felt properties as scenarios. Run this before a release to verify the experience is correct.

**Part 2 — Developer Verification Appendix**. Shell commands and technical tests. Run before a release build. Not for non-technical testers.

Use step numbers to report issues precisely ("Scenario B, Step 2: X doesn't work").

---

## Part 1 — User Experience Tests

### Before You Start

You need:
- Jeff installed from a `.dmg` or running from a dev build
- An OpenAI API key starting with `sk-`
- A folder on your Mac that contains at least one text or markdown file
- A document open in any app (Pages, Word, Safari, etc.)

**For a clean test run (no terminal required):**
1. Open Jeff from the tray icon
2. Go to tray → **Privacy Center** → **Clear all Jeff data** — this resets all tasks, messages, and settings
3. To reset the API key: go through onboarding again (tray → **Set up Jeff again**) and enter the key fresh

---

### Scenario A — Already Present (Felt Property 1)

> Jeff is not launched. Jeff is already there.

**A1.** Enable Launch at Login: click the Jeff tray icon → **Launch at Login** (checkmark appears).

**A2.** Log out of macOS: Apple menu → **Log Out**. Log back in.

**A3.** Wait 5 seconds after login.

Expected: Jeff's tray icon appears in the menu bar. **No window opens**, no alert, no focus steal. Jeff is silently present.

**A4.** Press the hotkey: **Cmd+Shift+J**.

Expected: The companion bar appears in the top-right corner within 200ms. It is dark, minimal, and positioned near the corner.

**A5.** Press the hotkey again.

Expected: The companion bar hides. The tray icon remains. Jeff is still running.

---

### Scenario B — Already Knows Your Task (Felt Property 2)

> When you return, Jeff knows what you're working on. No briefing. No context pasting.

**B1.** Open any app — Pages, Word, Safari — and create or open a document. Give it a recognizable title like "My Vision Document".

**B2.** Wait 3–5 seconds, then press **Cmd+Shift+J** to open Jeff.

Expected: Below the task label in the companion bar, you see a context line showing the app name and document title — e.g., `Pages — My Vision Document`. Jeff already knows what you have open.

**B3.** Type in the companion: `What am I currently working on?` and press Enter.

Expected: Jeff's response references "My Vision Document" (or the document name you opened). Jeff read your active window without any pasting or briefing.

**B4.** (Requires folder connection) Connect a folder via tray → **Set up Jeff again** → Step 3. Pick a folder that has at least one file in it.

Expected: Within a few seconds, the companion bar briefly shows `indexed: [filename]` in green for each file in the folder. Jeff is reading your work in real time.

**B5.** Ask: `What's in my files?`

Expected: Jeff references actual content from your files.

---

### Scenario C — Interruption (Felt Property 3)

> Mid-sentence, either direction. Not via button presses.

**C1.** Send a long message: "Write me a five-paragraph essay about the history of artificial intelligence." Jeff will start speaking the response via TTS.

**C2.** While Jeff is speaking, start typing a new message — don't click anything, just type.

Expected: TTS stops before you finish your first word. Your characters appear in the input field. Jeff is no longer speaking.

**C3.** Complete and send your new message.

Expected: Jeff responds to the new message without referencing the interrupted one.

**C4.** Send another long message and wait for TTS to start.

**C5.** Press the hotkey **Cmd+Shift+J** while Jeff is speaking.

Expected: TTS stops immediately. The input field is focused. You can type.

**C6.** Send another long message and wait for TTS to start.

**C7.** Click the microphone button while Jeff is speaking.

Expected: TTS stops. Recording starts.

**C8.** While typing a message, wait for a previous Jeff response to arrive.

Expected: TTS does not begin playing while you are actively typing in the input box.

---

### Scenario D — Parallel Work (Felt Property 4)

> You say "handle the intro while I keep going" and Jeff does it in parallel. You never stopped.

**D1.** Type (or say): "Draft a short intro paragraph for this task while I keep chatting." Press Enter.

**D2.** Immediately send another message: "How's my day looking?"

Expected: Jeff responds to the second message ("How's my day looking?") AND a status row appears in the companion bar showing `jeff is working on: [intro draft]` with a small purple spinner.

**D3.** Keep chatting. Watch the companion bar.

Expected: While the companion bar shows the subtask running, you can continue sending messages without any delay or blockage.

**D4.** When the subtask completes, an offer card appears in the companion message stream.

Expected: You see the offer card without having opened the workspace. The card shows what Jeff drafted and has **keep it** and **dismiss** buttons. Approve or dismiss from the companion. The workspace was never opened.

**D5.** Say (or type): "Draft some notes and save them to a file." Wait for Jeff to propose a file write.

Expected: A file write approval card appears in the companion message stream with the file path, a content excerpt, and **approve** / **reject** buttons. Approve from the companion.

**D6.** While a subtask is running, click the **cancel** button in the companion subtask row.

Expected: The subtask stops. The spinner row disappears. No file write goes through.

---

### Scenario E — Jeff Initiates (Felt Property 5)

> When you return to a task Jeff orients you. Jeff speaks first, sometimes.

**E1.** Set an active task: type "I'm working on my history essay" and send it.

**E2.** Chat with Jeff for a minute. Then close the companion bar (press the hotkey or click collapse).

**E3.** Switch to another app. Work in it for 6 minutes without opening Jeff.

Expected: A native macOS notification appears from Jeff. It tells you specifically where you left off on your task — not "you have been away," but a sentence about the actual content.

**E4.** Click the notification.

Expected: The companion bar expands and shows the reorientation context — what Jeff remembered about your task. This is not a generic "opened from notification" message; it is the actual orientation content.

**E5.** Enable quiet mode (click **quiet off** in the companion header).

**E6.** Wait 6 more minutes.

Expected: No notification fires. Jeff monitored your session but suppressed output because quiet mode is on.

---

### Scenario F — Complete Session Without the Workspace

> Everything a user needs is in the companion bar.

**F1.** Open Jeff. Do not open the workspace (no tray → Open Full Workspace).

**F2.** Send 5 messages. Verify Jeff responds normally.

**F3.** Trigger a subtask (say "draft something for me in the background").

**F4.** Approve or reject the subtask result from the companion.

**F5.** If you have more than one task: click the `·` button next to the task label. A dropdown appears showing your recent tasks. Switch to a different task.

**F6.** Switch back to the original task. Messages are correct for each task.

Expected throughout: The "open full workspace" button does not appear anywhere in the companion bar. The workspace was never needed.

---

## Part 2 — Developer Verification Appendix

> These steps require developer tools. Run them before a release, not as part of regular user testing.

### Part 0 — Full Reset (Developer)

**0-A. Kill any running Jeff process**

```
pkill -f "jeff-desktop" 2>/dev/null; pkill -f "tauri" 2>/dev/null
```

**0-B. Delete app data**

```
rm -rf ~/Library/Application\ Support/com.jeff.desktop
```

**0-C. Delete stored API key from Keychain**

```
security delete-generic-password -s "com.jeff.desktop" -a "openai_api_key" 2>/dev/null; echo "done"
```

**0-D. Confirm reset is clean**

```
ls ~/Library/Application\ Support/com.jeff.desktop 2>/dev/null || echo "clean"
```

Expected: `clean`

---

### Part 1 — Build and Launch (Developer)

**Step 1: Open a terminal in the project root**

```
cd /Users/krishmalik/Desktop/Continuum/jeff/desktop
```

**Step 2: Install dependencies (first run only)**

```
npm install
```

**Step 3: Start the dev build**

```
npm run tauri dev
```

Expected: Compiles, tray icon appears, companion bar opens.

---

### Part 2 — Onboarding (Technical Regression)

**Step 4–8:** Same as the user-facing test but verify the exact API responses:
- Step 6: `validate_openai_api_key` returns `{ is_valid: true }`
- Step 7: `set_preferred_workspace_folder` is called and watcher starts
- Step 8: accessibility permission prompt shows system dialog

---

### Part 3 — Core Message Loop

**Step 9:** Send `What can you help me with?`

Expected: streaming response completes, `ambient_set_tray_status` idle is called.

**Step 10:** Send `Give me three tips for staying focused while writing.`

Expected: 4 total messages in overlay (2 user, 2 assistant). Auto-scroll works.

---

### Part 4 — Folder Connection

**Step 11:** Verify watcher status shows `watching [folder]`.

**Step 12:** Add a file programmatically to verify initial scan fires file-indexed events:

```
echo "My test note for Jeff. The project deadline is next Friday." > /path/to/your/folder/test_jeff_note.txt
```

Expected: `indexed: test_jeff_note.txt` appears in green within 2 seconds.

**Step 13:** Ask `What does my test note say about the deadline?`

Expected: Jeff references "next Friday".

---

### Part 5 — Active Window Context

**Step 14:** Open Pages with a document named "Research Paper Draft".

Expected: Companion shows `Pages — Research Paper Draft`.

**Step 15:** Ask `What am I working on?`

Expected: Response references "Research Paper Draft".

---

### Part 6 — Voice Input

**Step 16:** Confirm microphone permission.

**Step 17–18:** Click mic button, speak, click stop.

Expected: Transcription appears as user message, Jeff responds.

**Step 19:** Voice naturalness test:
- Ask a short question via voice (under 15 words)
- Expected: TTS response begins with a natural interjection ("got it," "here you go," etc.) — NOT a filler phrase ("Certainly,", "Of course,").

**Step 19b:** Interruption via typing:
- Send a long message, wait for TTS to start
- Without clicking anything, start typing
- Expected: TTS stops before you finish your first word.

---

### Part 7 — Error Handling (UI path)

**Step 20:** Test bad API key via the UI (not keychain manipulation):
- Go to tray → **Set up Jeff again** → Step 2
- Enter an intentionally invalid key (`sk-bad`) and click Validate
- Expected: Red error message: "Your API key isn't working."

Restore your real key by going through Step 2 again with the correct key.

**Step 21:** Test offline error:
- Turn off Wi-Fi, then send a message
- Expected: Error banner: "Jeff couldn't reach OpenAI — check your network connection."
- Re-enable Wi-Fi before continuing.

---

### Part 8 — Quiet Mode and Tray Status

**Step 22:** Click **quiet off** in the companion header.

Expected: Label changes to **quiet** (quiet mode on). TTS is suppressed.

**Step 23:** Send a short message, watch the status dot.

Expected: Gray/idle → working → idle. Correct colors.

---

### Part 9 — Multi-Task Switching (Companion Only)

**Step 24:** Click the `·` button next to the task label (appears when >1 task exists).

Expected: Dropdown shows up to 5 recent tasks.

**Step 25:** Select a different task. Verify messages update. Switch back. Verify original messages restore.

Expected: Task isolation is correct — no message mixing.

---

### Part 10 — Final Sanity Pass

With folder connected and a document open:

Ask: `Based on my files and what I have open right now, what should I focus on?`

Expected: Response references both file content and document name. Streams without errors.

---

### Part 11 — Phase Check Scripts

Run the automated phase verification (from project root):

```
bash scripts/phase16_check.sh   # subtask chain
bash scripts/phase19_check.sh   # single window + session restore
bash scripts/phase20_check.sh   # active window context
```

Expected: All checks pass.

---

## Reporting Issues

When reporting, always include:
- The scenario or step number
- Exactly what you did
- Exactly what you expected
- Exactly what happened (paste error messages verbatim)
- Output of: `git log --oneline -5`
