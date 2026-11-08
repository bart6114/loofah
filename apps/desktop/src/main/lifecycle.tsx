import { useRouteContext } from "@tanstack/react-router";
import { useCallback, useEffect, useRef } from "react";

import { useMainHost, type MainCaptureActivity } from "./host-context";

import { useLanguageModel } from "~/ai/hooks";
import { DevtoolsFloatingPanelHost } from "~/devtools-panel/host";
import { initEnhancerService } from "~/services/enhancer";
import { EventListeners } from "~/services/event-listeners";
import { useRemoteSessionDeletionUndoListener } from "~/session/hooks/useDeleteSession";
import { useVaultHost } from "~/shared/application/vault-provider";
import type { VaultScope } from "~/shared/application/vault-scope";
import {
  createSessionTabCloseHandler,
  useDesktopTabLifecycle,
} from "~/shared/desktop-tab-lifecycle";
import { UndoDeleteToast } from "~/sidebar/toast/undo-delete-toast";
import type { ListenerStore } from "~/store/zustand/listener";
import { useTabs, useTabsStore } from "~/store/zustand/tabs";
import { MainListenerControlBridge } from "~/stt/window-control";

export function createLocalMainCaptureActivity(
  scope: VaultScope,
  capture: ListenerStore,
): MainCaptureActivity {
  const captureState = (id: string) => {
    const current = capture.getState();
    if (current.live.loading) return "unknown";
    const mode = current.getSessionMode(id);
    return mode === "inactive" ? "idle" : mode;
  };
  return {
    scope,
    captureState,
    subscribeCapture: (id, changed) => {
      let previous = captureState(id);
      return capture.subscribe(() => {
        const next = captureState(id);
        if (next !== previous) {
          previous = next;
          changed();
        }
      });
    },
  };
}

export function useClassicMainLifecycle() {
  const store = useTabsStore();
  const { scope } = useVaultHost();
  const host = useMainHost();
  const activity =
    host?.scope === scope && host.activity?.scope === scope
      ? host.activity
      : undefined;
  const takeInitialSession =
    host?.scope === scope ? host.takeInitialSession : undefined;
  const openNew = useTabs((state) => state.openNew);
  const createClose = useCallback(() => {
    if (!scope) return null;
    return createSessionTabCloseHandler({
      scope,
      store,
      captureState: activity?.captureState,
      subscribeCapture: activity?.subscribeCapture,
    });
  }, [scope, store, activity]);

  const openDefaultEmptyTab = useCallback(() => {
    openNew({ type: "empty" });
  }, [openNew]);

  const openPendingWelcomeTab = useCallback(() => {
    const welcomeSessionId = takeInitialSession?.();
    if (welcomeSessionId) {
      openNew({ type: "sessions", id: welcomeSessionId });
    }
  }, [openNew, takeInitialSession]);

  useDesktopTabLifecycle({
    onEmpty: openDefaultEmptyTab,
    onInitialized: openPendingWelcomeTab,
    onZeroTabs: openDefaultEmptyTab,
    createOnClose: createClose,
  });
}

export function ClassicMainServices() {
  return (
    <>
      <MainListenerControlBridge />
      <EnhancerInit />
      <SessionDeletionUndoListener />
      <EventListeners />
      <UndoDeleteToast />
      <DevtoolsFloatingPanelHost />
    </>
  );
}

function SessionDeletionUndoListener() {
  useRemoteSessionDeletionUndoListener(true);
  return null;
}

function EnhancerInit() {
  const { aiTaskStore } = useRouteContext({
    from: "__root__",
  });

  const model = useLanguageModel();

  const modelRef = useRef(model);
  modelRef.current = model;

  useEffect(() => {
    if (!aiTaskStore) return;

    const service = initEnhancerService({
      aiTaskStore,
      getModel: () => modelRef.current,
    });

    return () => service.dispose();
  }, [aiTaskStore]);

  return null;
}
