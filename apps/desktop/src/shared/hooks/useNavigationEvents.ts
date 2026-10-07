import { useNavigate } from "@tanstack/react-router";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { useEffect } from "react";

import { events as windowsEvents } from "@hypr/plugin-windows";

import { createAsyncListenerScope } from "../async-listener-scope";
import { useNewNote } from "../useNewNote";

import { useAboutDialog } from "~/store/zustand/about-dialog";
import { isTabInputSupported, useTabs } from "~/store/zustand/tabs";

export const useNavigationEvents = () => {
  const navigate = useNavigate();
  const openNew = useTabs((state) => state.openNew);
  const openNewNote = useNewNote({ behavior: "new" });

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
              openNewNote();
            } else if (payload.path === "/app/about") {
              useAboutDialog.getState().setOpen(true);
            } else if (payload.path === "/app/settings") {
              const tab = (payload.search?.tab as string) ?? "app";
              openNew({ type: "settings", state: { tab } });
            } else {
              void navigate({
                to: payload.path,
                search: payload.search ?? undefined,
              });
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
              openNewNote();
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
  }, [navigate, openNew, openNewNote]);
};
