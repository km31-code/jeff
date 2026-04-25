const DEFAULT_PORT = 47832;

async function getConfig() {
  const stored = await chrome.storage.local.get(["jeffBridgeToken", "jeffBridgePort"]);
  return {
    token: String(stored.jeffBridgeToken || "").trim(),
    port: Number(stored.jeffBridgePort || DEFAULT_PORT)
  };
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
