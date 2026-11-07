import { useRouteContext } from "@tanstack/react-router";
import { useCallback, useEffect, useRef, useMemo } from "react";

import { useLanguageModel } from "~/ai/hooks";
import { DevtoolsFloatingPanelHost } from "~/devtools-panel/host";
import { takePendingWelcomeSession } from "~/onboarding/welcome-note";
import { initEnhancerService } from "~/services/enhancer";
import { EventListeners } from "~/services/event-listeners";
import { useRemoteSessionDeletionUndoListener } from "~/session/hooks/useDeleteSession";
import { useVaultHost } from "~/shared/application/vault-provider";
import {
  createSessionTabCloseHandler,
  useDesktopTabLifecycle,
} from "~/shared/desktop-tab-lifecycle";
import { UndoDeleteToast } from "~/sidebar/toast/undo-delete-toast";
import { useTabs, useTabsStore } from "~/store/zustand/tabs";
import { MainListenerControlBridge } from "~/stt/window-control";

export function useClassicMainLifecycle() {
  const store = useTabsStore();
  const { scope } = useVaultHost();
  const openNew = useTabs((state) => state.openNew);
  const close = useMemo(
    () =>
      createSessionTabCloseHandler({
        isActive: () => scope?.getSnapshot().state === "connected",
        invalidateSessionResource: (id) =>
          store.getState().invalidateResource("sessions", id),
      }),
    [scope, store],
  );

  const openDefaultEmptyTab = useCallback(() => {
    openNew({ type: "empty" });
  }, [openNew]);

  const openPendingWelcomeTab = useCallback(() => {
    const welcomeSessionId = takePendingWelcomeSession();
    if (welcomeSessionId) {
      openNew({ type: "sessions", id: welcomeSessionId });
    }
  }, [openNew]);

  useDesktopTabLifecycle({
    onEmpty: openDefaultEmptyTab,
    onInitialized: openPendingWelcomeTab,
    onZeroTabs: openDefaultEmptyTab,
    onClose: close,
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
