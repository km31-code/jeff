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
          body: JSON.stringify({ receipt_id: message.receiptId })
        }).catch(() => {});
      }
    });
    return false;
  }

  return false;
});

// compute a simple djb2-style hash of a string for anchor validation.
// matches the backend SHA-256 only conceptually; the full SHA-256 comparison
// happens in the backend. here we compare the current selection against the
// before_text directly (string equality after normalization) since we have
// the full text, not just the hash.
function normalizeText(text) {
  return text.replace(/\s+/g, " ").trim();
}

async function applyEditInPlace(message) {
  const { beforeText, afterText, receiptId } = message;

  // get the current selection
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0) {
    return { ok: false, reason: "no selection" };
  }

  const currentText = sel.toString();

  // anchor validation: compare current selected text to before_text
  if (normalizeText(currentText) !== normalizeText(beforeText)) {
    return { ok: false, anchorMismatch: true, reason: "anchor text no longer matches" };
  }

  // apply the replacement using execCommand (supported in content editable areas)
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
}
