chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (!message) return false;

  if (message.type === "JEFF_CAPTURE_SELECTION") {
    sendResponse({
      text: window.getSelection ? window.getSelection().toString() : "",
      title: document.title || "",
      url: window.location.href
    });
    return false;
  }

  // phase 23: apply an approved live edit in place.
  // verifies the anchor hash before replacing to prevent silent corruption.
  if (message.type === "JEFF_APPLY_EDIT") {
    applyEditInPlace(message).then(result => {
      if (!result.ok && result.anchorMismatch) {
        // anchor drifted — report fallback to backend
        fetch(`http://127.0.0.1:${message.port}/apply-fallback`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ token: message.token, receipt_id: message.receiptId })
        }).catch(() => {});
      } else {
        fetch(`http://127.0.0.1:${message.port}/apply-result`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            token: message.token,
            receipt_id: message.receiptId,
            status: result.ok ? "applied" : "failed",
            error: result.reason || null
          })
        }).catch(() => {});
      }
    });
    return false;
  }

  return false;
});

async function sha256Hex(text) {
  const bytes = new TextEncoder().encode(String(text || ""));
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

async function applyEditInPlace(message) {
  const { beforeText, afterText, anchorHash } = message;

  // get the current selection
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0) {
    return { ok: false, reason: "no selection" };
  }

  const currentText = sel.toString();

  // anchor validation: compare current selected text's SHA-256 to the
  // original anchor, preventing silent writes to shifted selections.
  const currentHash = await sha256Hex(currentText);
  const expectedHash = String(anchorHash || await sha256Hex(beforeText)).toLowerCase();
  if (currentHash !== expectedHash) {
    return { ok: false, anchorMismatch: true, reason: "anchor text no longer matches" };
  }

  if (document.queryCommandSupported && document.queryCommandSupported("insertText")) {
    const inserted = document.execCommand("insertText", false, afterText);
    if (inserted) {
      return { ok: true };
    }
  }

  try {
    const range = sel.getRangeAt(0);
    range.deleteContents();
    const textNode = document.createTextNode(afterText);
    range.insertNode(textNode);

    // move cursor to end of inserted text
    range.setStartAfter(textNode);
    range.setEndAfter(textNode);
    sel.removeAllRanges();
    sel.addRange(range);
    return { ok: true };
  } catch (error) {
    return { ok: false, reason: String(error && error.message ? error.message : error) };
  }
}
