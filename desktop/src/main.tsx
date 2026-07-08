import React from "react";
import ReactDOM from "react-dom/client";
import Root from "./Root";
import { isOverlayWindow } from "./ambientClient";
import "./styles.css";

const rootElement = document.getElementById("root") as HTMLElement;
if (isOverlayWindow()) {
  document.body.classList.add("overlay-body");
}

// Root owns the overlay/workspace switch inside the overlay window. the hidden
// main window stays available as a close-to-hide workspace escape hatch.
ReactDOM.createRoot(rootElement).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>
);
