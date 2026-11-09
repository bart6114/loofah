import { createFileRoute } from "@tanstack/react-router";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useLayoutEffect, useMemo } from "react";

import { getCurrentWebviewWindowLabel } from "@hypr/plugin-windows";

import { ClassicMainLayout } from "~/main/layout";
import { TabContentNote } from "~/session";
import { RemoteNoteWindow } from "~/shared/application/remote-note-window";
import { StandaloneWindowShell } from "~/shared/window-shell";
import { type Tab, useTabs, useTabsStore } from "~/store/zustand/tabs";
import { useListener } from "~/stt/contexts";

export const Route = createFileRoute("/app/note/$sessionId")({
  component: Component,
});

function Component() {
  const { sessionId } = Route.useParams();
  useCloseStandaloneNoteWindowOnEscape();

  if (getCurrentWebviewWindowLabel().startsWith("remote-note-"))
    return <RemoteNoteWindow />;

  return (
    <ClassicMainLayout includeServices={false}>
      <StandaloneNote sessionId={sessionId} />
    </ClassicMainLayout>
  );
}

function StandaloneNote({ sessionId }: { sessionId: string }) {
  useAttachStandaloneNoteToLiveSession(sessionId);
  return <StandaloneNoteContent sessionId={sessionId} />;
}

export function StandaloneNoteContent({ sessionId }: { sessionId: string }) {
  const tab = useStandaloneNoteTab(sessionId);
  return (
    <StandaloneWindowShell topDragRegion={false}>
      <div className="bg-background h-screen w-screen">
        <TabContentNote tab={tab} standaloneWindow />
      </div>
    </StandaloneWindowShell>
  );
}

export function useAttachStandaloneNoteToLiveSession(sessionId: string) {
  const attachLiveSession = useListener((state) => state.attachLiveSession);
  const hasLiveSessionEvents = useListener((state) =>
    Boolean(state.live.eventUnlistenersBySession[sessionId]),
  );

  useEffect(() => {
    if (hasLiveSessionEvents) {
      return;
    }

    void attachLiveSession(sessionId);
  }, [attachLiveSession, hasLiveSessionEvents, sessionId]);
}

export function useStandaloneNoteTab(sessionId: string) {
  const tabsStore = useTabsStore();
  const tab = useMemo(
    () =>
      ({
        active: true,
        id: sessionId,
        pinned: false,
        slotId: `note-window-${sessionId}`,
        state: { view: null, autoStart: null },
        type: "sessions",
      }) satisfies Extract<Tab, { type: "sessions" }>,
    [sessionId],
  );

  const storeTab = useTabs((state) =>
    state.tabs.find(
      (candidate): candidate is Extract<Tab, { type: "sessions" }> =>
        candidate.type === "sessions" && candidate.id === sessionId,
    ),
  );

  useLayoutEffect(() => {
    const state = tabsStore.getState();
    const existingTab = state.tabs.find(
      (candidate): candidate is Extract<Tab, { type: "sessions" }> =>
        candidate.type === "sessions" && candidate.id === sessionId,
    );

    if (existingTab) {
      state.select(existingTab);
      return;
    }

    state.openCurrent(tab);
  }, [sessionId, tab, tabsStore]);

  return storeTab ?? tab;
}

export function useCloseStandaloneNoteWindowOnEscape() {
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") {
        return;
      }

      event.preventDefault();
      void getCurrentWindow().close();
    };

    window.addEventListener("keydown", handleKeyDown, { capture: true });
    return () => {
      window.removeEventListener("keydown", handleKeyDown, { capture: true });
    };
  }, []);
}
