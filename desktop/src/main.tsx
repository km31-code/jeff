import React from "react";
import ReactDOM from "react-dom/client";
import Root from "./Root";
import "./styles.css";

// single-window design: the overlay window is the only window. Root switches
// between Overlay (companion bar) and App (workspace mode) by resizing the
// same os window — no second window is created or managed.
const rootElement = document.getElementById("root") as HTMLElement;
document.body.classList.add("overlay-body");
ReactDOM.createRoot(rootElement).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>
);
