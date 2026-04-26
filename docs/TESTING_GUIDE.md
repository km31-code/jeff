# Jeff — End-to-End Testing Guide

Use the step numbers to report issues precisely ("Step 7: X doesn't work").

---

## Part 0 — Full Reset

Do this before a clean test run. Clears all data Jeff has ever stored.

**0-A. Kill any running Jeff process**

```
pkill -f "jeff-desktop" 2>/dev/null; pkill -f "tauri" 2>/dev/null
```

**0-B. Delete app data (all tasks, messages, artifacts, settings)**

```
rm -rf ~/Library/Application\ Support/com.jeff.desktop
```

**0-C. Delete stored API key from Keychain**

```
security delete-generic-password -s "com.jeff.desktop" -a "openai_api_key" 2>/dev/null; echo "done"
```

You should see `done` (not an error — "not found" is also fine, it just means nothing was stored).

**0-D. Confirm reset is clean**

```
ls ~/Library/Application\ Support/com.jeff.desktop 2>/dev/null || echo "clean"
```

Expected: `clean`

---

## Part 1 — Build and Launch

**Step 1: Open a terminal in the project root**

```
cd /Users/krishmalik/Desktop/Continuum/jeff/desktop
```

**Step 2: Install dependencies (first run only, skip if node_modules exists)**

```
npm install
```

**Step 3: Start the dev build**

```
npm run tauri dev
```

This compiles Rust (~30–60 seconds first time, ~10 seconds after) then opens the app.

**Expected at Step 3:**
- Terminal prints `Compiling jeff-desktop...` then `Finished`
- A small window or tray icon appears
- No red compile errors in the terminal
- The overlay companion window appears (small bar near top of screen)

**If Step 3 fails:** paste the full terminal error. Common causes: missing Rust toolchain (`rustup show`), missing npm packages (`npm install`), port 1420 in use.

---

## Part 2 — Onboarding (First-Run Flow)

**Step 4: Trigger onboarding**

The onboarding wizard should appear automatically because no API key is stored. If the overlay is in collapsed mode, click **expand**.

**Expected at Step 4:**
- Overlay shows "Step 1 of 5"
- Text: "What Jeff is — Jeff is your task-focused coworker..."
- A **Continue** button and a **Cancel** button

**Step 5: Advance through Step 1**

Click **Continue**.

**Expected at Step 5:**
- Jumps to "Step 2 of 5 — API key setup"
- A password input field (`sk-...` placeholder)

**Step 6: Enter your OpenAI API key**

Paste your `sk-...` key into the field. Click **Validate key**.

**Expected at Step 6:**
- Brief "working" state while key is validated against the OpenAI API
- Green confirmation message: "API key is valid" or similar
- Automatically advances to Step 3

**If Step 6 fails with "API key isn't working":** The key itself may be invalid. Try validating it at platform.openai.com. If the key is definitely valid, report "Step 6: key rejected with message: [paste message]".

**Step 7: Connect a workspace folder (Step 3 of onboarding)**

Click **Choose folder**. A native file picker opens. Navigate to any folder that has files in it (e.g. your Desktop, a project folder, or a folder with a few documents).

**Expected at Step 7:**
- Picker opens
- After picking, the folder path appears in the onboarding panel
- A **Continue** button becomes available

**Step 8: Complete the remaining onboarding steps (Steps 4–5)**

Click through. Step 4 covers accessibility permission, Step 5 finishes setup.

**Expected at Step 8:**
- Step 4 asks to enable Accessibility permission. Click **Enable** — macOS opens System Settings > Privacy > Accessibility. Toggle Jeff on. Return to Jeff.
- Step 5 shows a "You're ready" summary. Click **Finish**.
- Onboarding panel disappears. A text input box appears in the overlay.

---

## Part 3 — Sending a Message (Core Loop)

**Step 9: Type a message and send**

In the overlay input box, type: `What can you help me with?` and press Enter.

**Expected at Step 9:**
- Your message appears immediately as a "user" bubble
- Status dot changes to "working"
- An "assistant" bubble appears within 1–3 seconds, starting with "thinking..." then filling in with a response
- Status dot returns to "idle" when the response finishes
- No error banner

**If Step 9 shows no response and no error:** Report "Step 9: message sent, no response, no error shown". This is the silent streaming error bug — confirm the fix is deployed (`git log --oneline -3`).

**If Step 9 shows an error banner:**
- "Your API key isn't working" → API key issue (Step 6)
- "Jeff couldn't reach OpenAI" → network issue
- Any other text → paste it exactly

**Step 10: Send a follow-up message**

Type: `Give me three tips for staying focused while writing.` and press Enter.

**Expected at Step 10:**
- Conversation grows — now 4 bubbles (2 user, 2 assistant)
- Messages scroll so the latest response is visible
- Auto-scroll keeps the bottom of the conversation in view as tokens stream in

---

## Part 4 — Folder Connection (Workspace File Watcher)

**Step 11: Verify the watcher is running**

In the overlay (expanded), look for the watcher status line directly below the task name. It should say "watching [folder name]".

**Expected at Step 11:**
- Overlay shows "watching [your-folder-name]" below the task title
- If you open the full workspace, the watcher section also shows the folder path

**If Step 11 shows "no folder connected":** The watcher did not start. Go through onboarding again and pick a folder, or report "Step 11: watcher not running after onboarding".

**Step 12: Add a file to the connected folder**

Create a plain text file **inside the exact folder you chose in Step 7**. Replace `/path/to/your/folder` with that folder's path:

```
echo "My test note for Jeff. The project deadline is next Friday." > /path/to/your/folder/test_jeff_note.txt
```

Do NOT use `~/Desktop` unless that is literally the folder you connected.

**Expected at Step 12:**
- Within 1–2 seconds, the overlay watcher line briefly shows "indexed: test_jeff_note.txt" in green
- This confirms the file was detected and embedded

**If Step 12 shows no "indexed" notice:** The watcher is not watching that folder, or the file path is wrong. Double check the folder path in the watcher status line matches where you created the file.

**Step 13: Ask a question about the file**

In the overlay, type: `What does my test note say about the deadline?` and press Enter.

**Expected at Step 13:**
- Jeff's response references "next Friday" or the content of the note
- The response cites the file content

**If Step 13 returns a generic answer with no mention of the file:** The file was not indexed (Step 12 never showed the green "indexed" notice). Report "Step 13: no indexed notice appeared in Step 12, response was: [paste response]".

---

## Part 5 — Screen Reading (Active Window Context)

Requires: Accessibility permission granted in Step 8.

**Step 14: Open a document in another app**

Open a PDF, a Google Doc in Safari/Chrome, or any named document in Pages, Word, or a text editor. The document title should be something recognizable.

**Expected at Step 14:**
- In the overlay, a context line appears below the task title, e.g.:
  `Pages — My Document.pages`
  or
  `Safari — Project Brief - Google Docs`

**If Step 14 shows nothing:** Check that Accessibility is toggled ON for Jeff in System Settings > Privacy & Security > Accessibility. If it is on and still nothing shows, report "Step 14: accessibility on, no context line visible".

**Step 15: Ask about the active document**

With the document still open (Jeff in another window, or use Cmd+Tab to switch), type in Jeff: `What am I currently working on?`

**Expected at Step 15:**
- Jeff's response mentions the document name or app name
- The system prompt was injected with the active window context

---

## Part 6 — Voice Input

**Step 16: Confirm microphone permission**

Jeff needs microphone access. If not yet granted, macOS will prompt automatically when you first use voice. Grant it.

**Step 17: Record a voice message**

In the overlay, click the microphone button (bottom right of the input area).

**Expected at Step 17:**
- The mic button pulses (animated) to show recording is active
- Input placeholder changes to "Recording — click mic to send"

**Step 18: Speak and stop recording**

Say clearly: "What are three things I should know about the files in my workspace?"

Then click the mic button again to stop.

**Expected at Step 18:**
- Recording stops
- Status briefly shows "working" while audio is transcribed (Whisper via OpenAI)
- Your spoken message appears as a "user" bubble (the transcription)
- Jeff responds as usual

**If Step 18 hangs on "working" forever:** Report "Step 18: voice sends but never gets a response". This may be the voice `setSending` bug — confirm the fix is deployed.

**If Step 18 shows a transcription error:** Report "Step 18: transcription failed with: [paste message]". This may be the MIME type codec parameter bug — confirm the fix is deployed.

**Step 19: Test voice with a question**

Record yourself asking: "Can you summarize what this task is about?"

**Expected at Step 19:**
- Transcription appears correctly as text
- Jeff answers about the current task context

---

## Part 7 — Error Handling

**Step 20: Test bad API key error message**

Do a partial reset — only delete the keychain key, keep data:

```
security delete-generic-password -s "com.jeff.desktop" -a "openai_api_key"
```

Then set an invalid key via onboarding (or set one in Keychain manually with a fake value `sk-bad`):

```
security add-generic-password -s "com.jeff.desktop" -a "openai_api_key" -w "sk-bad"
```

Restart the app (`npm run tauri dev`), skip onboarding if possible, then send a message.

**Expected at Step 20:**
- An error banner appears: "Your API key isn't working. Open settings to update it."
- NOT a blank response or a hang

**Restore your real key after this test** by going through onboarding Step 6 again.

**Step 21: Test offline error message**

Turn off Wi-Fi (or disconnect network), then send a message.

**Expected at Step 21:**
- Error banner: "Jeff couldn't reach OpenAI — check your network connection."
- NOT a blank response

Re-enable Wi-Fi before continuing.

---

## Part 8 — Quiet Mode and Tray Status

**Step 22: Toggle quiet mode**

Click **quiet off** in the overlay header.

**Expected at Step 22:**
- Button label changes to **quiet** (indicating quiet mode is now on)
- TTS responses should be suppressed (no audio plays for new messages)

Click again to turn quiet mode off.

**Step 23: Watch tray status during a send**

Send a short message and watch the status dot in the overlay header.

**Expected at Step 23:**
- Dot is gray/idle when waiting
- Dot changes to "working" (different color) while response streams
- Returns to idle when done

---

## Part 9 — Multi-Task Switching

**Step 24: Create a second task**

In the full workspace window, create a new task (e.g. "Weekly Planning").

Switch to it as the active task.

**Expected at Step 24:**
- Overlay task label updates to "Weekly Planning"
- Messages panel is empty (no messages in the new task)

**Step 25: Send a message in the new task, then switch back**

Type: `This is a note for my weekly plan.` and send it.

Then switch back to the first task.

**Expected at Step 25:**
- First task's messages reappear correctly
- Second task's message is not mixed in
- Workspace watcher attaches to the correct folder for the active task

---

## Part 10 — Final Sanity Pass

After all steps above pass, do one end-to-end loop:

1. Have a folder connected with at least one file
2. Have a document open in another app
3. Ask: `Based on my files and what I have open right now, what should I focus on?`

**Expected:**
- Jeff's response references both the file content and the active document name
- Response streams token by token with no errors
- Overlay auto-scrolls to show the full response

---

## Reporting Issues

When reporting, always include:
- The step number
- Exactly what you did
- Exactly what you expected
- Exactly what happened (paste error messages verbatim)
- Output of: `git log --oneline -5` (to confirm which code is running)
