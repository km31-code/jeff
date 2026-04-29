# Milestone C — Intent Routing in the Overlay

Goal: when the user sends a message in the companion window, classify intent and
dispatch parallel work (subtask chains) the same way the full workspace does.

---

## Checklist

- [x] 1. Add imports: `classifyMessageIntent`, `IntentSlotsDto`, `startSubtaskChain`
- [x] 2. Add `OverlayRoutedIntent` type
- [x] 3. Add pure helper: `containsIntentPhrase`
- [x] 4. Add pure helper: `inferOverlayMessageIntentKeyword`
- [x] 5. Add pure helper: `inferOverlaySubtaskExecutionType`
- [x] 6. Add pure helper: `inferOverlaySubtaskExecutionTypeFromDraftType`
- [x] 7. Add pure helper: `deriveOverlaySubtaskTitle`
- [x] 8. Add pure helper: `classifyOverlayMessageIntentWithFallback`
- [x] 9. Add state: `infoNotice`
- [x] 10. Modify `handleSubmit`: classify intent, route subtask/revision/unknown
- [x] 11. Modify `submitVoiceMessage`: same routing (unknown → answer for voice)
- [x] 12. Add `infoNotice` banner to JSX (dismissible, info style)
- [x] 13. Verify: build passes, no TS errors

---

## Design decisions recorded

- `classifyOverlayMessageIntentWithFallback` uses same 300ms timeout + keyword fallback
  as App.tsx. Falls back silently and logs to console.
- For text input: "unknown" intent bails with a clarification message (no message sent,
  no task created).
- For voice input: "unknown" is coerced to "answer" — discarding a transcribed voice
  message would feel broken.
- `startSubtaskChain` is called AFTER `sendMessageStreaming` so the stream turn ID is
  registered first. The chain start is fast (spawns a Rust thread, returns immediately).
  The `companion-started` Tauri event wires the spinner once the thread transitions to
  "running".
- Revision intent: send the chat message (LLM can discuss the revision), then surface an
  info notice nudging the user to open the full workspace. No silent failure.
- Suggestion intent: just send the chat message (LLM already handles suggestions in its
  system prompt context). No special routing needed in the overlay.
- `infoNotice` is separate from `errorMessage` so a revision nudge does not look like an
  error. Dismissed by button or cleared at the next submit.
- No new deps needed in `handleSubmit` or `submitVoiceMessage` — helpers are pure
  module-level functions and `startSubtaskChain` is a stable import.
