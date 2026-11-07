import { useEffect, useRef, useSyncExternalStore } from "react";

import { isSessionEmpty, softDeleteSession } from "~/session/queries";
import { listenerStore } from "~/store/zustand/listener/instance";
import { type Tab, useTabs, useTabsStore } from "~/store/zustand/tabs";
import { useScopedTabsController } from "~/store/zustand/tabs/scoped-provider";

type InitializeDesktopTabsOptions = {
  getTabs: () => Tab[];
  restore: () => Promise<void>;
  isActive: () => boolean;
  onInitialized?: (() => void) | null;
  onZeroTabs?: (() => void) | null;
};

type SessionTabCloseHandlerOptions = {
  invalidateSessionResource: (sessionId: string) => void;
  getSessionMode?: (sessionId: string) => string | null | undefined;
  isSessionEmptyFn?: typeof isSessionEmpty;
  deleteSessionFn?: typeof softDeleteSession;
  isActive?: () => boolean;
};

export async function initializeDesktopTabs({
  getTabs,
  restore,
  isActive,
  onInitialized,
  onZeroTabs,
}: InitializeDesktopTabsOptions) {
  await restore();
  if (!isActive()) return;
  onInitialized?.();
  if (!isActive() || getTabs().length > 0) {
    return;
  }

  onZeroTabs?.();
}

export function createSessionTabCloseHandler({
  invalidateSessionResource,
  getSessionMode = (sessionId) =>
    listenerStore.getState().getSessionMode(sessionId),
  isSessionEmptyFn = isSessionEmpty,
  deleteSessionFn = softDeleteSession,
  isActive = () => true,
}: SessionTabCloseHandlerOptions) {
  return (tab: Tab) => {
    if (!isActive() || tab.type !== "sessions") {
      return;
    }

    const sessionId = tab.id;
    const sessionMode = getSessionMode(sessionId);
    if (
      sessionMode === "active" ||
      sessionMode === "finalizing" ||
      sessionMode === "running_batch"
    ) {
      return;
    }

    void (async () => {
      if (!(await isSessionEmptyFn(sessionId)) || !isActive()) return;

      const deleted = await deleteSessionFn(sessionId);
      if (deleted && isActive()) invalidateSessionResource(sessionId);
    })().catch((error) => {
      console.error("session close cleanup", error);
    });
  };
}

export function useDesktopTabLifecycle({
  onEmpty,
  onInitialized,
  onZeroTabs,
  onClose,
}: {
  onEmpty?: (() => void) | null;
  onInitialized?: (() => void) | null;
  onZeroTabs?: (() => void) | null;
  onClose: (tab: Tab) => void;
}) {
  const { registerOnEmpty, registerCanClose, registerOnClose } = useTabs();
  const store = useTabsStore();
  const controller = useScopedTabsController();
  const connection = useSyncExternalStore(
    controller.scope.subscribe,
    controller.scope.getSnapshot,
  );
  const hasOpenedInitialTab = useRef(false);

  useEffect(() => {
    if (hasOpenedInitialTab.current || connection.state !== "connected") {
      return;
    }

    let active = true;
    const isActive = () =>
      active && controller.scope.getSnapshot().state === "connected";
    const complete = (callback?: (() => void) | null) => {
      hasOpenedInitialTab.current = true;
      callback?.();
    };
    void initializeDesktopTabs({
      getTabs: () => store.getState().tabs,
      restore: () => controller.start(),
      isActive,
      onInitialized: () => complete(onInitialized),
      onZeroTabs: () => complete(onZeroTabs),
    }).catch((error) => {
      console.error("tab restoration", error);
      if (isActive()) complete(onZeroTabs);
    });
    return () => {
      active = false;
    };
  }, [onInitialized, onZeroTabs, store, controller, connection.state]);

  useEffect(() => {
    registerOnEmpty(onEmpty ?? null);
    return () => registerOnEmpty(null);
  }, [onEmpty, registerOnEmpty]);

  useEffect(() => {
    registerCanClose(() => true);
    return () => registerCanClose(null);
  }, [registerCanClose]);

  useEffect(() => {
    registerOnClose(onClose);
    return () => registerOnClose(null);
  }, [registerOnClose, onClose]);
}
