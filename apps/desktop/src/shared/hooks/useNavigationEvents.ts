import { useNavigate } from "@tanstack/react-router";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { useCallback, useEffect } from "react";

import { events as windowsEvents, type Navigate } from "@hypr/plugin-windows";

import { createAsyncListenerScope } from "../async-listener-scope";
import { useNewNote, useNewNoteAndListen } from "../useNewNote";

import { useAboutDialog } from "~/store/zustand/about-dialog";
import { isTabInputSupported, useTabs } from "~/store/zustand/tabs";

export const useNavigationEvents = () => {
  const navigate = useNavigate();
  const fallback = useCallback(
    (path: string, search: Navigate["search"]) => {
      void navigate({ to: path, search: search ?? undefined });
    },
    [navigate],
  );
  useCapturedNavigationEvents(fallback);
};

export function useCapturedNavigationEvents(
  navigate?: (path: string, search: Navigate["search"]) => void,
) {
  const openNew = useTabs((state) => state.openNew);
  const openNewNote = useNewNote({ behavior: "new" });
  const recordNew = useNewNoteAndListen();

  useEffect(() => {
    const scope = createAsyncListenerScope();
    (window as any).__HYPR_NAVIGATE__ = scope.guard((path: string) => {
      const match = path.match(/^\/app\/([^/]+)\/(.+)$/);
      if (!match) return;
      const [, type, id] = match;
      if (type === "session") {
        openNew({ type: "sessions", id });
      }
    });

    const webview = getCurrentWebviewWindow();

    void scope
      .add(() =>
        windowsEvents.navigate(webview).listen(
          scope.guard(({ payload }) => {
            if (payload.path === "/app/new") {
              if (payload.search?.record === "true") recordNew();
              else openNewNote();
            } else if (payload.path === "/app/about") {
              useAboutDialog.getState().setOpen(true);
            } else if (payload.path === "/app/settings") {
              const tab = (payload.search?.tab as string) ?? "app";
              openNew({ type: "settings", state: { tab } });
            } else if (navigate) {
              navigate(payload.path, payload.search);
            } else if (payload.path === "/app") {
              openNew({ type: "empty" });
            } else {
              const match = payload.path.match(/^\/app\/session\/([^/]+)$/);
              if (match) openNew({ type: "sessions", id: match[1] });
            }
          }),
        ),
      )
      .catch((error) =>
        console.error("[navigation] listener setup failed", error),
      );

    void scope
      .add(() =>
        windowsEvents.openTab(webview).listen(
          scope.guard(({ payload }) => {
            if (payload.tab.type === "sessions" && payload.tab.id === "new") {
              if (payload.tab.state?.autoStart) recordNew();
              else openNewNote();
            } else if (!isTabInputSupported(payload.tab)) {
              return;
            } else {
              openNew(payload.tab);
            }
          }),
        ),
      )
      .catch((error) =>
        console.error("[navigation] listener setup failed", error),
      );

    return () => {
      delete (window as any).__HYPR_NAVIGATE__;
      scope.dispose();
    };
  }, [navigate, openNew, openNewNote, recordNew]);
}
