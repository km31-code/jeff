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

// phase 23: poll for approval of a pending live edit and dispatch to the
// active content script when approved. falls back to guided apply on rejection
// or anchor mismatch.
async function pollForLiveEditApproval(receiptId, beforeText, afterText, anchorHash, tabId, port, token) {
  const MAX_POLLS = 40; // 20 seconds at 500ms intervals
  for (let i = 0; i < MAX_POLLS; i++) {
    await new Promise(resolve => setTimeout(resolve, 500));
    try {
      const resp = await fetch(`http://127.0.0.1:${port}/pending-approval/${encodeURIComponent(token)}/${receiptId}`);
      if (!resp.ok) continue;
      const data = await resp.json();
      if (data.status === "approved") {
        // dispatch the apply command to the content script
        chrome.tabs.sendMessage(tabId, {
          type: "JEFF_APPLY_EDIT",
          receiptId,
          beforeText,
          afterText,
          anchorHash,
          token,
          port
        });
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
    document_title: proposal.documentTitle || ""
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
      config.token
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
chrome.runtime.onMessage.addListener((message, sender) => {
  if (!message) return false;
  if (message.type === "JEFF_PROPOSE_LIVE_EDIT") {
    handleLiveEditProposal({
      ...message,
      tabId: sender.tab?.id
    }).catch((error) => {
      console.warn("[jeff-live-edit]", error);
    });
  }
  return false;
});
