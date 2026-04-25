const tokenInput = document.getElementById("token");
const portInput = document.getElementById("port");
const statusEl = document.getElementById("status");

async function load() {
  const stored = await chrome.storage.local.get(["jeffBridgeToken", "jeffBridgePort"]);
  tokenInput.value = stored.jeffBridgeToken || "";
  portInput.value = String(stored.jeffBridgePort || 47832);
}

async function save() {
  await chrome.storage.local.set({
    jeffBridgeToken: tokenInput.value.trim(),
    jeffBridgePort: Number(portInput.value || 47832)
  });
  statusEl.textContent = "Saved.";
}

document.getElementById("save").addEventListener("click", () => {
  save().catch((error) => {
    statusEl.textContent = error.message || String(error);
  });
});

load().catch((error) => {
  statusEl.textContent = error.message || String(error);
});
