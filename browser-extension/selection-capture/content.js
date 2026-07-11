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
    applyEditInPlace(message)
      .then((result) => sendResponse(result))
      .catch((error) => sendResponse({
        ok: false,
        reason: error && error.message ? error.message : String(error)
      }));
    return true;
  }

  // apex d2: Google Docs tracked/suggested write-back. This path anchors by
  // 50-char surrounding context and reports guided fallback before touching the
  // document when the anchor has drifted.
  if (message.type === "JEFF_APPLY_GOOGLE_DOCS_ACTION") {
    applyGoogleDocsAction(message)
      .then((result) => sendResponse(result))
      .catch((error) => sendResponse({
        ok: false,
        reason: error && error.message ? error.message : String(error)
      }));
    return true;
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
  // Selector families overlap in Docs. Joining all of them duplicates the
  // document and can turn one target into an apparent ambiguous match. Use the
  // first rendered family that yields text, preserving DOM order within it.
  const selectorFamilies = [
    ".kix-lineview-text-block",
    ".kix-wordhtmlgenerator-word-node",
    '[role="textbox"]',
    '[contenteditable="true"]'
  ];
  for (const selector of selectorFamilies) {
    const text = [...document.querySelectorAll(selector)]
      .map((node) => node.innerText || node.textContent || "")
      .map((value) => value.trim())
      .filter(Boolean)
      .join("\n");
    if (text.trim()) return text;
  }

  const editor =
    document.querySelector(".kix-appview-editor") ||
    document.querySelector(".docs-texteventtarget-iframe") ||
    document.body;
  return String(editor?.innerText || editor?.textContent || "");
}

function googleDocsEditorRoot() {
  return (
    document.querySelector(".kix-appview-editor") ||
    document.querySelector('[role="textbox"][contenteditable="true"]') ||
    document.querySelector('[contenteditable="true"]') ||
    document.body
  );
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

function normalizeAnchorText(text) {
  return String(text || "").replace(/\s+/g, " ").trim();
}

function googleDocsDocumentId(url = window.location.href) {
  try {
    const parsed = new URL(url);
    if (parsed.origin !== "https://docs.google.com") return "";
    const match = parsed.pathname.match(/\/document\/(?:u\/\d+\/)?d\/([^/?#]+)/);
    return match ? decodeURIComponent(match[1]) : "";
  } catch (_) {
    return "";
  }
}

function normalizedGoogleDocsTitle(title) {
  return normalizeAnchorText(String(title || "").replace(/\s+-\s+Google Docs\s*$/i, ""));
}

function googleDocsIdentityMatches(message) {
  const expectedId = String(message.expectedDocumentId || "").trim();
  const expectedTitle = normalizedGoogleDocsTitle(message.expectedDocumentTitle || "");
  const currentId = googleDocsDocumentId();
  const currentTitle = normalizedGoogleDocsTitle(document.title || "");
  return Boolean(
    expectedId &&
    currentId === expectedId &&
    expectedTitle &&
    currentTitle === expectedTitle
  );
}

function buildAnchorContext50(fullText, beforeText) {
  const text = String(fullText || "");
  const target = String(beforeText || "");
  const index = text.indexOf(target);
  if (index < 0) {
    return { anchorBefore: "", anchorAfter: "" };
  }
  return {
    anchorBefore: text.slice(Math.max(0, index - 50), index),
    anchorAfter: text.slice(index + target.length, index + target.length + 50)
  };
}

function countOccurrences(haystack, needle) {
  if (!needle) return 0;
  let count = 0;
  let offset = 0;
  while (offset <= haystack.length - needle.length) {
    const next = haystack.indexOf(needle, offset);
    if (next < 0) break;
    count += 1;
    offset = next + 1;
  }
  return count;
}

function normalizedAnchorNeedle(beforeText, anchorBefore, anchorAfter) {
  return normalizeAnchorText(`${anchorBefore || ""}${beforeText || ""}${anchorAfter || ""}`);
}

function anchoredMatchCount(documentText, beforeText, anchorBefore, anchorAfter) {
  const before = normalizeAnchorText(beforeText);
  const leading = normalizeAnchorText(anchorBefore);
  const trailing = normalizeAnchorText(anchorAfter);
  // A target without any surrounding context is not an anchored action. For an
  // insertion, at least one side is required as the actual insertion point.
  if (!leading && !trailing) return 0;
  if (!before && !leading && !trailing) return 0;
  return countOccurrences(
    normalizeAnchorText(documentText),
    normalizedAnchorNeedle(beforeText, anchorBefore, anchorAfter)
  );
}

function anchorMatchesDocument(documentText, beforeText, anchorBefore, anchorAfter) {
  return anchoredMatchCount(documentText, beforeText, anchorBefore, anchorAfter) === 1;
}

function modeText(node) {
  return normalizeAnchorText([
    node.getAttribute("aria-label") || "",
    node.getAttribute("data-tooltip") || "",
    node.getAttribute("title") || "",
    node.innerText || node.textContent || ""
  ].join(" ")).toLowerCase();
}

function isVisibleControl(node) {
  const style = window.getComputedStyle ? window.getComputedStyle(node) : null;
  return !style || (style.display !== "none" && style.visibility !== "hidden");
}

function detectGoogleDocsEditMode() {
  const controls = [
    ...document.querySelectorAll('[role="button"][aria-label], [role="button"][data-tooltip], [aria-pressed]')
  ];
  for (const node of controls) {
    if (!isVisibleControl(node)) continue;
    const label = modeText(node);
    if (node.getAttribute("aria-pressed") === "true" && /\bsuggest(?:ing)?\b/.test(label)) {
      return { mode: "suggesting", control: node };
    }
    if (/\bsuggesting mode\b/.test(label) || label === "suggesting") {
      return { mode: "suggesting", control: node };
    }
    if (/\bediting mode\b/.test(label) || label === "editing") {
      return { mode: "direct", control: node };
    }
  }
  return { mode: "unknown", control: null };
}

function selectionContext(root, selection) {
  if (!root || !selection || selection.rangeCount !== 1) return null;
  const range = selection.getRangeAt(0);
  if (!root.contains(range.startContainer) || !root.contains(range.endContainer)) return null;
  try {
    const leading = document.createRange();
    leading.selectNodeContents(root);
    leading.setEnd(range.startContainer, range.startOffset);
    const trailing = document.createRange();
    trailing.selectNodeContents(root);
    trailing.setStart(range.endContainer, range.endOffset);
    return { leading: leading.toString(), trailing: trailing.toString(), range };
  } catch (_) {
    return null;
  }
}

function selectionHasAnchors(selection, beforeText, anchorBefore, anchorAfter, insertionSearch) {
  const context = selectionContext(googleDocsEditorRoot(), selection);
  if (!context) return null;
  const leading = normalizeAnchorText(context.leading);
  const trailing = normalizeAnchorText(context.trailing);
  const expectedBefore = normalizeAnchorText(anchorBefore);
  const expectedAfter = normalizeAnchorText(anchorAfter);

  if (beforeText) {
    if (selection.toString() !== beforeText) return null;
    if (expectedBefore && !leading.endsWith(expectedBefore)) return null;
    if (expectedAfter && !trailing.startsWith(expectedAfter)) return null;
    return context.range.cloneRange();
  }

  if (insertionSearch === "before") {
    if (selection.toString() !== anchorBefore) return null;
    if (expectedAfter && !trailing.startsWith(expectedAfter)) return null;
    const insertion = context.range.cloneRange();
    insertion.collapse(false);
    return insertion;
  }
  if (selection.toString() !== anchorAfter) return null;
  if (expectedBefore && !leading.endsWith(expectedBefore)) return null;
  const insertion = context.range.cloneRange();
  insertion.collapse(true);
  return insertion;
}

function selectStrictAnchoredTarget(beforeText, anchorBefore, anchorAfter) {
  if (typeof window.find !== "function") return false;
  const root = googleDocsEditorRoot();
  const selection = window.getSelection();
  if (!root || !selection) return false;
  const insertionSearch = beforeText ? "target" : (anchorBefore ? "before" : "after");
  const searchText = beforeText || anchorBefore || anchorAfter;
  if (!searchText) return false;

  const start = document.createRange();
  start.selectNodeContents(root);
  start.collapse(true);
  selection.removeAllRanges();
  selection.addRange(start);

  let uniqueRange = null;
  let matches = 0;
  for (let attempts = 0; attempts < 1000; attempts += 1) {
    const found = window.find(searchText, false, false, false, false, false, false);
    if (!found) break;
    const candidate = selectionHasAnchors(
      selection,
      beforeText,
      anchorBefore,
      anchorAfter,
      insertionSearch
    );
    if (candidate) {
      matches += 1;
      uniqueRange = candidate;
      if (matches > 1) break;
    }
    const current = selection.getRangeAt(0).cloneRange();
    current.collapse(false);
    selection.removeAllRanges();
    selection.addRange(current);
  }
  if (matches !== 1 || !uniqueRange) {
    selection.removeAllRanges();
    return false;
  }
  selection.removeAllRanges();
  selection.addRange(uniqueRange);
  return true;
}

async function resolveGoogleDocsEditMode(preferSuggesting) {
  const initial = detectGoogleDocsEditMode();
  if (initial.mode === "unknown" || !preferSuggesting || initial.mode === "suggesting") {
    return initial.mode;
  }

  // Switching is best-effort; a known Editing mode is still safe as a direct
  // edit, but the result must report that truthfully. Never infer the mode from
  // a failed click.
  try {
    initial.control?.click();
    await new Promise((resolve) => setTimeout(resolve, 150));
    const suggestingOption = [...document.querySelectorAll('[role="menuitem"], [role="option"]')]
      .find((node) => isVisibleControl(node) && /^suggesting\b/.test(modeText(node)));
    if (suggestingOption) {
      suggestingOption.click();
      await new Promise((resolve) => setTimeout(resolve, 200));
      const updated = detectGoogleDocsEditMode();
      return updated.mode === "suggesting" ? "suggesting" : "direct";
    }
    initial.control?.click();
  } catch (_) {
    // A verified direct mode remains a valid, explicitly reported fallback.
  }
  return "direct";
}

async function applyGoogleDocsAction(message) {
  if (window.location.origin !== "https://docs.google.com") {
    return { ok: false, guided: true, reason: "unsupported_origin" };
  }
  if (!googleDocsIdentityMatches(message)) {
    return { ok: false, guided: true, reason: "document_identity_mismatch" };
  }

  const beforeText = String(message.beforeText || "");
  const afterText = String(message.afterText || "");
  const anchorBefore = String(message.anchorBefore || "");
  const anchorAfter = String(message.anchorAfter || "");
  if (!afterText && !beforeText) {
    return { ok: false, guided: true, reason: "empty_edit" };
  }
  const documentText = extractGoogleDocsText();
  const matchCount = anchoredMatchCount(documentText, beforeText, anchorBefore, anchorAfter);
  if (matchCount !== 1) {
    return {
      ok: false,
      guided: true,
      anchorMismatch: true,
      reason: matchCount > 1 ? "anchor_ambiguous" : "anchor_miss"
    };
  }

  const mode = await resolveGoogleDocsEditMode(message.preferSuggesting !== false);
  if (mode === "unknown") {
    return { ok: false, guided: true, reason: "google_docs_edit_mode_unknown" };
  }
  if (!googleDocsIdentityMatches(message)) {
    return { ok: false, guided: true, reason: "document_identity_mismatch" };
  }
  if (!selectStrictAnchoredTarget(beforeText, anchorBefore, anchorAfter)) {
    return { ok: false, guided: true, anchorMismatch: true, reason: "anchor_not_selectable" };
  }

  const result = await applyEditInPlace({
    beforeText,
    afterText,
    anchorHash: message.anchorHash
  });
  if (!result.ok) return result;

  await new Promise((resolve) => setTimeout(resolve, 50));
  const verified = anchorMatchesDocument(
    extractGoogleDocsText(),
    afterText,
    anchorBefore,
    anchorAfter
  );
  if (!verified) {
    return { ok: false, mutationAttempted: true, reason: "post_apply_verification_failed", mode };
  }
  const finalMode = detectGoogleDocsEditMode().mode;
  return { ok: true, mode: finalMode === "unknown" ? mode : finalMode };
}
