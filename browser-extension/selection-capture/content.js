const JEFF_CONTENT_OBSERVATION_POLL_MS = 10_000;
const JEFF_CONTENT_OBSERVATION_ALLOWED_ORIGINS = new Set(["https://docs.google.com"]);

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

let jeffContentObservationEnabled = false;
let jeffContentObservationInFlight = false;

function isJeffContentObservationAllowedHere() {
  return JEFF_CONTENT_OBSERVATION_ALLOWED_ORIGINS.has(window.location.origin);
}

async function getJeffBridgeConfig() {
  const stored = await chrome.storage.local.get([
    "jeffBridgeToken",
    "jeffBridgePort",
    "jeffContentObservationSites"
  ]);
  const sites = stored.jeffContentObservationSites || {};
  return {
    token: String(stored.jeffBridgeToken || "").trim(),
    port: Number(stored.jeffBridgePort || 47832),
    siteEnabled: Boolean(sites[window.location.origin])
  };
}

async function refreshJeffContentObservationEnabled() {
  if (!isJeffContentObservationAllowedHere()) {
    jeffContentObservationEnabled = false;
    return;
  }
  const config = await getJeffBridgeConfig();
  jeffContentObservationEnabled = config.siteEnabled;
}

async function setJeffContentObservationSiteEnabled(enabled) {
  const stored = await chrome.storage.local.get(["jeffContentObservationSites"]);
  const sites = stored.jeffContentObservationSites || {};
  sites[window.location.origin] = Boolean(enabled);
  await chrome.storage.local.set({ jeffContentObservationSites: sites });
  jeffContentObservationEnabled = Boolean(enabled);
}

function extractGoogleDocsText() {
  const candidates = [
    ...document.querySelectorAll(".kix-lineview-text-block"),
    ...document.querySelectorAll(".kix-wordhtmlgenerator-word-node"),
    ...document.querySelectorAll('[role="textbox"]'),
    ...document.querySelectorAll('[contenteditable="true"]')
  ];
  const candidateText = candidates
    .map((node) => node.innerText || node.textContent || "")
    .map((text) => text.trim())
    .filter(Boolean)
    .join("\n");
  if (candidateText.trim()) {
    return candidateText;
  }

  const editor =
    document.querySelector(".kix-appview-editor") ||
    document.querySelector(".docs-texteventtarget-iframe") ||
    document.body;
  return String(editor?.innerText || editor?.textContent || "");
}

function extractActiveDocumentText() {
  if (window.location.origin === "https://docs.google.com") {
    return extractGoogleDocsText();
  }
  return "";
}

async function postJeffContentObservation() {
  if (!jeffContentObservationEnabled || jeffContentObservationInFlight) {
    return;
  }
  if (document.visibilityState !== "visible" || !document.hasFocus()) {
    return;
  }
  const config = await getJeffBridgeConfig();
  if (!config.siteEnabled || !config.token) {
    jeffContentObservationEnabled = config.siteEnabled;
    return;
  }

  const text = extractActiveDocumentText();
  jeffContentObservationInFlight = true;
  try {
    const response = await fetch(`http://127.0.0.1:${config.port}/content-observation`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        token: config.token,
        text,
        provenance: {
          origin: window.location.origin,
          title: document.title || "",
          captured_at: Math.floor(Date.now() / 1000)
        }
      })
    });
    if (!response.ok && response.status === 403) {
      // backend privacy toggle is the source of truth; keep polling dormant
      // until the next site-toggle refresh.
      await setJeffContentObservationSiteEnabled(false);
    }
  } catch (_) {
    // Jeff may be closed or the bridge may be unavailable. Try again next poll.
  } finally {
    jeffContentObservationInFlight = false;
  }
}

if (isJeffContentObservationAllowedHere()) {
  refreshJeffContentObservationEnabled().catch(() => {});
  chrome.storage.onChanged.addListener((changes, area) => {
    if (
      area === "local" &&
      (changes.jeffContentObservationSites ||
        changes.jeffBridgeToken ||
        changes.jeffBridgePort)
    ) {
      refreshJeffContentObservationEnabled().catch(() => {});
    }
  });
  setInterval(() => {
    refreshJeffContentObservationEnabled()
      .then(postJeffContentObservation)
      .catch(() => {});
  }, JEFF_CONTENT_OBSERVATION_POLL_MS);
}

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
