import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import Overlay from "./Overlay";
import { isOverlayWindow } from "./ambientClient";
import "./styles.css";

// phase 11: same frontend bundle serves two windows. the overlay window is
// loaded with url `index.html#overlay`; the main window has no hash.
const rootElement = document.getElementById("root") as HTMLElement;
const root = ReactDOM.createRoot(rootElement);

if (isOverlayWindow()) {
  document.body.classList.add("overlay-body");
  root.render(
    <React.StrictMode>
      <Overlay />
    </React.StrictMode>
  );
} else {
  root.render(
    <React.StrictMode>
      <App />
    </React.StrictMode>
  );
}
