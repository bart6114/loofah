import { useQuery } from "@tanstack/react-query";
import { useCallback, useRef, useState } from "react";

import { commands as notificationCommands } from "@hypr/plugin-notification";
import {
  commands as windowsCommands,
  events as windowsEvents,
  getCurrentWebviewWindowLabel,
} from "@hypr/plugin-windows";

import { useMountEffect } from "~/shared/hooks/useMountEffect";
import {
  type DevtoolsOtaPreviewStatus,
  useDevtoolsOtaPreview,
} from "~/store/zustand/devtools-ota-preview";
import {
  type DevtoolsToastPreview,
  useDevtoolsToastPreview,
} from "~/store/zustand/devtools-toast-preview";
import { showBatchCompletedNotification } from "~/store/zustand/listener/general-batch";
import { listenerStore } from "~/store/zustand/listener/instance";
import { useTabs } from "~/store/zustand/tabs";
import {
  AUTO_STOP_CONFIRM_TIMEOUT_SECONDS,
  createAutoStopEndedNotificationKey,
} from "~/stt/auto-stop-notification";
import { commands } from "~/types/tauri.gen";

const canResolveDevtoolsPanel = import.meta.env.MODE !== "test";

type DevtoolsPanelAction =
  | "navigation:onboarding"
  | `toasts:preview:${DevtoolsToastPreview}`
  | "toasts:clear"
  | "ota:available"
  | "ota:downloading"
  | "ota:ready"
  | "ota:failed"
  | "ota:clear"
  | "notifications:mic-detected"
  | "notifications:mic-options"
  | "notifications:auto-stop"
  | "notifications:batch-done"
  | "notifications:clear"
  | "panel:opened"
  | "panel:closed"
  | "error:trigger";

export function DevtoolsFloatingPanelHost() {
  const isMainWindow = getCurrentWebviewWindowLabel() === "main";
  const shouldShow = useShouldShowDevtoolsPanel(isMainWindow);

  if (!isMainWindow) {
    return null;
  }

  if (!shouldShow) {
    return <DevtoolsFloatingPanelDisabled />;
  }

  return <DevtoolsFloatingPanelSync />;
}

function useShouldShowDevtoolsPanel(isMainWindow: boolean) {
  const enabledQuery = useQuery({
    queryKey: ["devtools-panel", "enabled"],
    queryFn: commands.showDevtool,
    enabled: isMainWindow && canResolveDevtoolsPanel,
    staleTime: Infinity,
  });

  return enabledQuery.data ?? false;
}

function DevtoolsFloatingPanelDisabled() {
  useMountEffect(() => {
    void hideDevtoolsPanel();
  });

  return null;
}

function DevtoolsFloatingPanelSync() {
  const { dialogs, handleAction, shouldThrow } = useDevtoolsPanelActions();
  const actionHandlerRef = useRef(handleAction);
  actionHandlerRef.current = handleAction;

  useMountEffect(() => {
    let cancelled = false;
    let unlistenAction: (() => void) | undefined;

    windowsEvents.devtoolsPanelAction
      .listen(({ payload }) => {
        actionHandlerRef.current(payload.action);
      })
      .then((unlisten) => {
        if (cancelled) {
          unlisten();
          return;
        }

        unlistenAction = unlisten;
      });

    return () => {
      cancelled = true;
      unlistenAction?.();
      void hideDevtoolsPanel();
    };
  });

  if (shouldThrow) {
    throw new Error("Test error triggered from devtools");
  }

  return dialogs;
}

function useDevtoolsPanelActions() {
  const openNew = useTabs((s) => s.openNew);
  const showToastPreview = useDevtoolsToastPreview(
    (state) => state.showPreview,
  );
  const clearToastPreview = useDevtoolsToastPreview(
    (state) => state.clearPreview,
  );
  const showOtaPreview = useDevtoolsOtaPreview((state) => state.showPreview);
  const clearOtaPreview = useDevtoolsOtaPreview((state) => state.clearPreview);
  const [shouldThrow, setShouldThrow] = useState(false);

  const showMainWindow = useCallback(async () => {
    await windowsCommands.windowShow({ type: "main" });
  }, []);

  const showOnboarding = useCallback(async () => {
    await showMainWindow();
    openNew({ type: "onboarding" });
  }, [openNew, showMainWindow]);

  const showToastPreviewInMainWindow = useCallback(
    async (preview: DevtoolsToastPreview) => {
      await showMainWindow();
      showToastPreview(preview);
    },
    [showMainWindow, showToastPreview],
  );

  const showOtaPreviewInMainWindow = useCallback(
    async (preview: DevtoolsOtaPreviewStatus) => {
      await showMainWindow();
      showOtaPreview(preview);
    },
    [showMainWindow, showOtaPreview],
  );

  const clearNotifications = useCallback(async () => {
    try {
      await notificationCommands.clearNotifications();
    } catch (error) {
      console.error("[devtools] failed to clear notifications", error);
    }
  }, []);

  const showMicDetectedNotification = useCallback(async () => {
    await notificationCommands.showNotification({
      key: `devtool-mic-${crypto.randomUUID()}`,
      title: "Are you in a meeting?",
      message: "",
      timeout: { secs: 30, nanos: 0 },
      source: {
        type: "mic_detected",
        app_names: ["Zoom"],
        app_ids: ["us.zoom.xos"],
      },
      start_time: null,
      participants: null,
      action_label: null,
      action_variant: null,
      options: null,
      footer: null,
      icon: null,
    });
  }, []);

  const showMicOptionsNotification = useCallback(async () => {
    await notificationCommands.showNotification({
      key: `devtool-mic-options-${crypto.randomUUID()}`,
      title: "Are you in a meeting?",
      message: "",
      timeout: { secs: 30, nanos: 0 },
      source: {
        type: "mic_detected",
        app_names: ["Zoom", "Google Chrome"],
        app_ids: ["us.zoom.xos", "com.google.Chrome"],
      },
      start_time: null,
      participants: null,
      action_label: "Yes",
      action_variant: null,
      options: null,
      footer: {
        text: "Ignore Zoom and Chrome?",
        actionLabel: "Yes",
        icon: { type: "bundle_id", bundle_id: "us.zoom.xos" },
      },
      icon: null,
    });
  }, []);

  const showAutoStopNotification = useCallback(async () => {
    const sessionId =
      listenerStore.getState().live.sessionId ??
      `devtool-${crypto.randomUUID()}`;

    await notificationCommands.showNotification({
      key: createAutoStopEndedNotificationKey(sessionId),
      title: "Did your meeting end?",
      message: `Loofah will stop listening in ${AUTO_STOP_CONFIRM_TIMEOUT_SECONDS} seconds.`,
      timeout: { secs: AUTO_STOP_CONFIRM_TIMEOUT_SECONDS, nanos: 0 },
      source: null,
      start_time: null,
      participants: null,
      action_label: "Stop",
      action_variant: "destructive",
      options: null,
      footer: null,
      icon: { type: "bundle_id", bundle_id: "com.google.Chrome" },
    });
  }, []);

  const handleAction = useCallback(
    (action: string) => {
      switch (action as DevtoolsPanelAction) {
        case "navigation:onboarding":
          void showOnboarding();
          return;
        case "toasts:preview:language-model":
          void showToastPreviewInMainWindow("language-model");
          return;
        case "toasts:preview:transcription-model":
          void showToastPreviewInMainWindow("transcription-model");
          return;
        case "toasts:preview:transcription-error":
          void showToastPreviewInMainWindow("transcription-error");
          return;
        case "toasts:preview:download":
          void showToastPreviewInMainWindow("download");
          return;
        case "toasts:clear":
          clearToastPreview();
          return;
        case "ota:available":
          void showOtaPreviewInMainWindow("available");
          return;
        case "ota:downloading":
          void showOtaPreviewInMainWindow("downloading");
          return;
        case "ota:ready":
          void showOtaPreviewInMainWindow("ready");
          return;
        case "ota:failed":
          void showOtaPreviewInMainWindow("failed");
          return;
        case "ota:clear":
          clearOtaPreview();
          return;
        case "notifications:mic-detected":
          void showMicDetectedNotification();
          return;
        case "notifications:mic-options":
          void showMicOptionsNotification();
          return;
        case "notifications:auto-stop":
          void showAutoStopNotification();
          return;
        case "notifications:batch-done":
          void showBatchCompletedNotification("devtool", { force: true });
          return;
        case "notifications:clear":
          void clearNotifications();
          return;
        case "panel:opened":
        case "panel:closed":
          return;
        case "error:trigger":
          setShouldThrow(true);
          return;
        default:
          console.warn("Unknown Devtools panel action:", action);
      }
    },
    [
      clearNotifications,
      showAutoStopNotification,
      showMicDetectedNotification,
      showMicOptionsNotification,
      showOnboarding,
      showToastPreviewInMainWindow,
      showOtaPreviewInMainWindow,
      clearToastPreview,
      clearOtaPreview,
    ],
  );

  return {
    dialogs: null,
    handleAction,
    shouldThrow,
  };
}

async function hideDevtoolsPanel() {
  const result = await windowsCommands.devtoolsPanelHide();
  if (result.status === "error") {
    console.error("Failed to hide Devtools panel:", result.error);
  }
}
