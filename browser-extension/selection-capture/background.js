const DEFAULT_PORT = 47832;

async function getConfig() {
  const stored = await chrome.storage.local.get(["jeffBridgeToken", "jeffBridgePort"]);
  return {
    token: String(stored.jeffBridgeToken || "").trim(),
    port: Number(stored.jeffBridgePort || DEFAULT_PORT)
  };
}

async function sha256Hex(text) {
  const bytes = new TextEncoder().encode(String(text || ""));
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

async function captureFromActiveTab() {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (!tab || !tab.id) return;

  const [{ result }] = await chrome.scripting.executeScript({
    target: { tabId: tab.id },
    func: () => {
      const text = window.getSelection ? window.getSelection().toString() : "";
      return {
        text,
        title: document.title || "",
        url: window.location.href
      };
    }
  });

  const config = await getConfig();
  if (!config.token) {
    throw new Error("Jeff bridge token is not configured.");
  }

  const payload = {
    token: config.token,
    text: String(result?.text || ""),
    app_name: "Browser",
    document_title: String(result?.title || tab.title || ""),
    source_url: String(result?.url || tab.url || ""),
    captured_at: Math.floor(Date.now() / 1000)
  };

  if (!payload.text.trim()) {
    throw new Error("No text is selected.");
  }

  const response = await fetch(`http://127.0.0.1:${config.port}/selection-capture`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload)
  });

  if (!response.ok) {
    const body = await response.text();
    throw new Error(body || `Jeff bridge returned ${response.status}`);
  }
}

function originFromUrl(url) {
  try {
    return new URL(url || "").origin;
  } catch (_) {
    return "";
  }
}

function googleDocsDocumentId(url) {
  try {
    const parsed = new URL(url || "");
    if (parsed.origin !== "https://docs.google.com") return "";
    const match = parsed.pathname.match(/\/document\/(?:u\/\d+\/)?d\/([^/?#]+)/);
    return match ? decodeURIComponent(match[1]) : "";
  } catch (_) {
    return "";
  }
}

function normalizedGoogleDocsTitle(title) {
  return String(title || "")
    .replace(/\s+-\s+Google Docs\s*$/i, "")
    .replace(/\s+/g, " ")
    .trim();
}

async function postBridgeJson(port, path, payload) {
  let lastError = null;
  for (let attempt = 0; attempt < 4; attempt += 1) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}${path}`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload)
      });
      if (response.ok) return;
      const body = await response.text();
      lastError = new Error(body || `Jeff bridge returned ${response.status}`);
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolve) => setTimeout(resolve, 150 * (attempt + 1)));
  }
  throw lastError || new Error("Jeff bridge did not acknowledge the action result");
}

async function reportApplyOutcome(result, receiptId, port, token) {
  if (!result || result.guided || result.anchorMismatch) {
    await postBridgeJson(port, "/apply-fallback", {
      token,
      receipt_id: receiptId,
      reason: result?.reason || "guided_apply_required"
    });
    return;
  }
  await postBridgeJson(port, "/apply-result", {
    token,
    receipt_id: receiptId,
    status: result.ok ? "applied" : "failed",
    error: result.ok && result.mode
      ? `mode:${result.mode}`
      : (result.reason || null)
  });
}

async function dispatchApprovedEdit(
  receiptId,
  beforeText,
  afterText,
  anchorHash,
  tabId,
  port,
  token,
  options
) {
  if (!Number.isInteger(tabId)) {
    await reportApplyOutcome(
      { ok: false, guided: true, reason: "source_tab_unavailable" },
      receiptId,
      port,
      token
    );
    return;
  }
  const tab = await chrome.tabs.get(tabId);
  const googleDocs = options.editorSurface === "google_docs" || options.editorSurface === "Google Docs";
  const currentUrl = String(tab?.url || "");
  const currentTitle = String(tab?.title || "");
  const identityChanged = googleDocs
    ? (
      !options.expectedDocumentId ||
      googleDocsDocumentId(currentUrl) !== options.expectedDocumentId ||
      normalizedGoogleDocsTitle(currentTitle) !== options.expectedDocumentTitle
    )
    : currentUrl !== options.expectedUrl;
  if (identityChanged) {
    await reportApplyOutcome(
      { ok: false, guided: true, reason: "document_identity_mismatch" },
      receiptId,
      port,
      token
    );
    return;
  }

  let result;
  try {
    result = await chrome.tabs.sendMessage(tabId, {
      type: googleDocs ? "JEFF_APPLY_GOOGLE_DOCS_ACTION" : "JEFF_APPLY_EDIT",
      receiptId,
      beforeText,
      afterText,
      anchorHash,
      anchorBefore: options.anchorBefore || "",
      anchorAfter: options.anchorAfter || "",
      preferSuggesting: options.preferSuggesting !== false,
      expectedDocumentId: options.expectedDocumentId || "",
      expectedDocumentTitle: options.expectedDocumentTitle || ""
    });
  } catch (error) {
    result = {
      ok: false,
      guided: true,
      reason: `content_script_unavailable:${error && error.message ? error.message : String(error)}`
    };
  }
  await reportApplyOutcome(result, receiptId, port, token);
}

function isContentObservationOriginAllowed(origin) {
  return origin === "https://docs.google.com";
}

async function getActiveSiteObservationStatus() {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  const origin = originFromUrl(tab?.url || "");
  const stored = await chrome.storage.local.get(["jeffContentObservationSites"]);
  const sites = stored.jeffContentObservationSites || {};
  return {
    origin,
    title: tab?.title || "",
    supported: isContentObservationOriginAllowed(origin),
    enabled: Boolean(sites[origin])
  };
}

async function setActiveSiteObservationEnabled(enabled) {
  const status = await getActiveSiteObservationStatus();
  if (!status.supported) {
    return { ...status, enabled: false };
  }
  const stored = await chrome.storage.local.get(["jeffContentObservationSites"]);
  const sites = stored.jeffContentObservationSites || {};
  sites[status.origin] = Boolean(enabled);
  await chrome.storage.local.set({ jeffContentObservationSites: sites });
  return { ...status, enabled: Boolean(enabled) };
}

// phase 23: poll for approval of a pending live edit and dispatch to the
// active content script when approved. falls back to guided apply on rejection
// or anchor mismatch.
async function pollForLiveEditApproval(receiptId, beforeText, afterText, anchorHash, tabId, port, token, options = {}) {
  const MAX_POLLS = 40; // 20 seconds at 500ms intervals
  for (let i = 0; i < MAX_POLLS; i++) {
    await new Promise(resolve => setTimeout(resolve, 500));
    try {
      const resp = await fetch(`http://127.0.0.1:${port}/pending-approval/${encodeURIComponent(token)}/${receiptId}`);
      if (!resp.ok) continue;
      const data = await resp.json();
      if (data.status === "approved") {
        // Await both the content-script result and the backend acknowledgment.
        // Never re-dispatch an edit just because result reporting was slow.
        try {
          await dispatchApprovedEdit(
            receiptId,
            beforeText,
            afterText,
            anchorHash,
            tabId,
            port,
            token,
            options
          );
        } catch (error) {
          console.warn("[jeff-live-edit-result]", error);
        }
        return;
      }
      if (data.status === "rejected") {
        // user declined — no action needed
        return;
      }
      if (data.status === "fallback") {
        // anchor drifted — guided apply fallback already handled by backend
        return;
      }
    } catch (_) {
      // network error, keep polling
    }
  }
}

// phase 23: handle a proposed live edit from the content script:
// send it to the backend /apply-edit endpoint and start the approval poll.
async function handleLiveEditProposal(proposal) {
  const config = await getConfig();
  if (!config.token) return;
  const anchorHash = proposal.anchorHash || await sha256Hex(proposal.beforeText);

  const payload = {
    token: config.token,
    editor_surface: proposal.editorSurface || "unknown",
    selection_anchor_hash: anchorHash,
    before_text: proposal.beforeText,
    after_text: proposal.afterText,
    document_title: proposal.documentTitle || proposal.expectedDocumentTitle || ""
  };

  const response = await fetch(`http://127.0.0.1:${config.port}/apply-edit`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload)
  });

  if (!response.ok) return;

  const data = await response.json();
  if (data.status === "pending_approval" && data.receipt_id) {
    pollForLiveEditApproval(
      data.receipt_id,
      proposal.beforeText,
      proposal.afterText,
      anchorHash,
      proposal.tabId,
      config.port,
      config.token,
      {
        editorSurface: proposal.editorSurface || "",
        anchorBefore: proposal.anchorBefore || "",
        anchorAfter: proposal.anchorAfter || "",
        preferSuggesting: proposal.preferSuggesting !== false,
        expectedUrl: proposal.expectedUrl || "",
        expectedDocumentId: proposal.expectedDocumentId || "",
        expectedDocumentTitle: proposal.expectedDocumentTitle || ""
      }
    );
  }
}

chrome.action.onClicked.addListener(() => {
  captureFromActiveTab().catch((error) => {
    console.warn("[jeff-selection-capture]", error);
  });
});

chrome.commands.onCommand.addListener((command) => {
  if (command === "capture-selection-for-jeff") {
    captureFromActiveTab().catch((error) => {
      console.warn("[jeff-selection-capture]", error);
    });
  }
});

// phase 23: listen for live edit proposals from content scripts
chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (!message) return false;
  if (message.type === "JEFF_CAPTURE_ACTIVE_SELECTION") {
    captureFromActiveTab().catch((error) => {
      console.warn("[jeff-selection-capture]", error);
    });
    return false;
  }
  if (message.type === "JEFF_GET_SITE_OBSERVATION_STATUS") {
    getActiveSiteObservationStatus()
      .then((status) => sendResponse({ ok: true, status }))
      .catch((error) => {
        console.warn("[jeff-content-observation]", error);
        sendResponse({ ok: false, error: error.message || String(error) });
      });
    return true;
  }
  if (message.type === "JEFF_SET_SITE_OBSERVATION_ENABLED") {
    setActiveSiteObservationEnabled(Boolean(message.enabled))
      .then((status) => sendResponse({ ok: true, status }))
      .catch((error) => {
        console.warn("[jeff-content-observation]", error);
        sendResponse({ ok: false, error: error.message || String(error) });
      });
    return true;
  }
  if (message.type === "JEFF_PROPOSE_LIVE_EDIT") {
    const sourceUrl = String(sender.tab?.url || "");
    const sourceTitle = String(sender.tab?.title || "");
    handleLiveEditProposal({
      ...message,
      tabId: sender.tab?.id,
      documentTitle: sourceTitle || message.documentTitle || "",
      expectedUrl: sourceUrl,
      expectedDocumentId: googleDocsDocumentId(sourceUrl),
      expectedDocumentTitle: normalizedGoogleDocsTitle(sourceTitle)
    }).catch((error) => {
      console.warn("[jeff-live-edit]", error);
    });
  }
  return false;
});
