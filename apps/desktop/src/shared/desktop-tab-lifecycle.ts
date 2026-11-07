import { onlineManager } from "@tanstack/react-query";
import { useEffect, useRef, useSyncExternalStore } from "react";

import { captureSessionCleanupGuard } from "~/shared/application/session-cleanup-guard";
import type { VaultScope } from "~/shared/application/vault-scope";
import {
  type Tab,
  type ScopedTabsStore,
  useTabs,
  useTabsStore,
} from "~/store/zustand/tabs";
import { useScopedTabsController } from "~/store/zustand/tabs/scoped-provider";

type InitializeDesktopTabsOptions = {
  getTabs: () => Tab[];
  restore: () => Promise<void>;
  isActive: () => boolean;
  onInitialized?: (() => void) | null;
  onZeroTabs?: (() => void) | null;
};

type SessionTabCloseHandlerOptions = {
  scope: VaultScope;
  store: ScopedTabsStore;
  captureState?: (sessionId: string) => string | null | undefined;
  subscribeCapture?: (sessionId: string, changed: () => void) => () => void;
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
  scope,
  store,
  captureState,
  subscribeCapture,
}: SessionTabCloseHandlerOptions) {
  const client = scope.client;
  const lifetime = new AbortController();
  const pending = new Set<string>();
  const available = () =>
    !lifetime.signal.aborted &&
    scope.client === client &&
    store.scope === client.scope &&
    scope.getSnapshot().state === "connected" &&
    client.canEdit &&
    client.features.includes("session_cleanup") &&
    (client.networkMode === "always" || onlineManager.isOnline());
  const handle = (tab: Tab) => {
    if (
      tab.type !== "sessions" ||
      !available() ||
      !captureState ||
      !subscribeCapture
    )
      return;
    const sessionId = tab.id;
    if (pending.has(sessionId) || pending.size >= 8) return;
    const isOpen = () =>
      store
        .getState()
        .tabs.some(
          (entry) => entry.type === "sessions" && entry.id === sessionId,
        );
    const idle = () => {
      try {
        return captureState(sessionId) === "idle";
      } catch {
        return false;
      }
    };
    if (isOpen() || !idle()) return;
    pending.add(sessionId);
    let invalidated = false;
    let reopened = false;
    const request = new AbortController();
    let guard: ReturnType<typeof captureSessionCleanupGuard>;
    let requestId: string;
    try {
      requestId = crypto.randomUUID();
      guard = captureSessionCleanupGuard(scope, sessionId, {
        isOpen: () => reopened || isOpen(),
      });
    } catch {
      pending.delete(sessionId);
      return;
    }
    const stops: (() => void)[] = [];
    const stop = () => {
      for (const release of stops.splice(0)) release();
    };
    const invalidate = () => {
      if (invalidated) return;
      invalidated = true;
      guard.dispose();
      request.abort();
      stop();
    };
    const own = (release: () => void) => {
      if (invalidated) release();
      else stops.push(release);
    };
    const current = () =>
      !invalidated && !reopened && !isOpen() && available() && idle();
    try {
      lifetime.signal.addEventListener("abort", invalidate, { once: true });
      own(() => lifetime.signal.removeEventListener("abort", invalidate));
      own(
        store.subscribe(() => {
          if (isOpen()) {
            reopened = true;
            invalidate();
          }
        }),
      );
      own(
        scope.subscribe(() => {
          if (!current()) invalidate();
        }),
      );
      own(
        onlineManager.subscribe(() => {
          if (!current()) invalidate();
        }),
      );
      own(
        subscribeCapture(sessionId, () => {
          if (!idle()) invalidate();
        }),
      );
    } catch {
      invalidate();
      pending.delete(sessionId);
      return;
    }
    if (!current()) {
      invalidate();
      pending.delete(sessionId);
      return;
    }
    void (async () => {
      if (!(await guard.prepare()) || !current() || !guard.canDelete()) return;
      await scope.run(() =>
        client.deleteEmptySession(
          { session_id: sessionId },
          { requestId, signal: request.signal },
        ),
      );
      // Replayed outcomes describe the original intent, so refetch current state.
      if (current() && guard.canDelete()) scope.resync();
    })()
      .catch((error) => {
        if (current()) console.error("session close cleanup", error);
      })
      .finally(() => {
        guard.dispose();
        stop();
        invalidated = true;
        pending.delete(sessionId);
      });
  };
  return Object.assign(handle, { dispose: () => lifetime.abort() });
}

export function useDesktopTabLifecycle({
  onEmpty,
  onInitialized,
  onZeroTabs,
  createOnClose,
}: {
  onEmpty?: (() => void) | null;
  onInitialized?: (() => void) | null;
  onZeroTabs?: (() => void) | null;
  createOnClose: () => ReturnType<typeof createSessionTabCloseHandler> | null;
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
    const onClose = createOnClose();
    registerOnClose(onClose);
    return () => {
      registerOnClose(null);
      onClose?.dispose();
    };
  }, [registerOnClose, createOnClose]);
}
