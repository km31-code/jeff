chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (!message || message.type !== "JEFF_CAPTURE_SELECTION") {
    return false;
  }

  sendResponse({
    text: window.getSelection ? window.getSelection().toString() : "",
    title: document.title || "",
    url: window.location.href
  });
  return false;
});
