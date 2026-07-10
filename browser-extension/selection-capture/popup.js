const toggle = document.getElementById("docs-toggle");
const site = document.getElementById("site");
const statusEl = document.getElementById("status");
const captureButton = document.getElementById("capture-selection");

function setStatus(text) {
  statusEl.textContent = text || "";
}

async function send(message) {
  return chrome.runtime.sendMessage(message);
}

async function loadStatus() {
  const response = await send({ type: "JEFF_GET_SITE_OBSERVATION_STATUS" });
  if (!response?.ok) {
    throw new Error(response?.error || "Could not read site status.");
  }
  const current = response.status;
  site.textContent = current.origin || "No supported tab is active.";
  toggle.checked = Boolean(current.enabled);
  toggle.disabled = !current.supported;
  setStatus(current.supported ? "" : "Google Docs tabs are supported first.");
}

toggle.addEventListener("change", () => {
  send({
    type: "JEFF_SET_SITE_OBSERVATION_ENABLED",
    enabled: toggle.checked
  })
    .then((response) => {
      if (!response?.ok) {
        throw new Error(response?.error || "Could not update site setting.");
      }
      toggle.checked = Boolean(response.status.enabled);
      setStatus(toggle.checked ? "Reading enabled for this tab." : "Reading disabled.");
    })
    .catch((error) => {
      toggle.checked = false;
      setStatus(error.message || String(error));
    });
});

captureButton.addEventListener("click", () => {
  send({ type: "JEFF_CAPTURE_ACTIVE_SELECTION" })
    .then(() => setStatus("Selection capture requested."))
    .catch((error) => setStatus(error.message || String(error)));
});

loadStatus().catch((error) => {
  setStatus(error.message || String(error));
});
