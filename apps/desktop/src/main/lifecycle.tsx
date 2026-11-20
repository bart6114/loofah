import { useRouteContext } from "@tanstack/react-router";
import { platform } from "@tauri-apps/plugin-os";
import { useCallback, useEffect, useMemo, useRef } from "react";

import {
  createLanguageModel,
  useLLMConnection,
} from "~/ai/hooks/useLLMConnection";
import { takePendingWelcomeSession } from "~/onboarding/welcome-note";
import { initEnhancerService } from "~/services/enhancer";
import { useDesktopTabLifecycle } from "~/shared/desktop-tab-lifecycle";
import { useTabs } from "~/store/zustand/tabs";
import { IphoneCaptureWorker } from "~/stt/iphone-capture-worker";
import { MainListenerControlBridge } from "~/stt/window-control";

export function useClassicMainLifecycle() {
  const openNew = useTabs((state) => state.openNew);

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
  });
}

export function ClassicMainServices() {
  const isOnboarding = useTabs(
    (state) => state.currentTab?.type === "onboarding",
  );

  return (
    <>
      <MainListenerControlBridge />
      <EnhancerInit />
      {!isOnboarding && platform() === "macos" && <IphoneCaptureWorker />}
    </>
  );
}

function EnhancerInit() {
  const { aiTaskStore } = useRouteContext({
    from: "__root__",
  });

  const { conn } = useLLMConnection();
  const model = useMemo(() => conn && createLanguageModel(conn), [conn]);
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
