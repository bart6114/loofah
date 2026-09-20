import { Outlet } from "@tanstack/react-router";
import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect } from "react";

import { useNavigationEvents } from "./hooks/useNavigationEvents";

import { DevtoolsFloatingPanelHost } from "~/devtools-panel/host";
import { UndoDeleteToast } from "~/sidebar/toast/undo-delete-toast";

export default function MainAppLayout() {
  useNavigationEvents();
  useFullscreenAttribute();

  return <MainAppContent />;
}

function MainAppContent() {
  return (
    <>
      <Outlet />
      <UndoDeleteToast />
      <DevtoolsFloatingPanelHost />
    </>
  );
}

const useFullscreenAttribute = () => {
  useEffect(() => {
    if (!isTauri()) {
      return;
    }

    const appWindow = getCurrentWindow();
    let cancelled = false;

    const sync = () => {
      void appWindow
        .isFullscreen()
        .then((fullscreen) => {
          if (!cancelled) {
            document.documentElement.toggleAttribute(
              "data-fullscreen",
              fullscreen,
            );
          }
        })
        .catch(() => {});
    };

    sync();
    const unlisten = appWindow.onResized(sync);

    return () => {
      cancelled = true;
      void unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);
};
