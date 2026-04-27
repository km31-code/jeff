import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import App from "./App";
import Overlay from "./Overlay";
import { AmbientStateDto, setWorkspaceMode } from "./ambientClient";

export default function Root(): JSX.Element {
  const [workspaceOpen, setWorkspaceOpen] = useState(false);

  const openWorkspace = useCallback(async () => {
    try {
      await setWorkspaceMode(true);
      setWorkspaceOpen(true);
    } catch {
      // fallback: still show workspace ui even if backend resize fails
      setWorkspaceOpen(true);
    }
  }, []);

  const closeWorkspace = useCallback(async () => {
    try {
      await setWorkspaceMode(false);
    } catch {
      // continue regardless
    }
    setWorkspaceOpen(false);
  }, []);

  // stay in sync with backend-initiated mode changes (tray menu, etc.)
  useEffect(() => {
    const unsub = listen<AmbientStateDto>("ambient://state-changed", (event) => {
      const mode: string = event.payload.overlay_mode;
      if (mode === "workspace") {
        setWorkspaceOpen(true);
      } else if (workspaceOpen && mode !== "workspace") {
        setWorkspaceOpen(false);
      }
    });
    return () => {
      unsub.then((fn) => fn()).catch(() => undefined);
    };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspaceOpen]);

  if (workspaceOpen) {
    return <App onCloseWorkspace={closeWorkspace} />;
  }
  return <Overlay onOpenWorkspace={openWorkspace} />;
}
