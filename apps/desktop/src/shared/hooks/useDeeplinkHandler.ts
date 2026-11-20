import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { platform } from "@tauri-apps/plugin-os";

import {
  type DeepLink,
  commands as deeplink2Commands,
  events as deeplink2Events,
} from "@hypr/plugin-deeplink2";
import {
  dismissInstruction,
  commands as windowCommands,
} from "@hypr/plugin-windows";
import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { subscribeThenDrainDeepLinks } from "~/shared/deeplink";
import { useMountEffect } from "~/shared/hooks/useMountEffect";
import { useTabs } from "~/store/zustand/tabs";
import { commands } from "~/types/tauri.gen";

export function useDeeplinkHandler() {
  useMountEffect(() => {
    if (!isTauri()) {
      return;
    }
    const webview = getCurrentWebviewWindow();
    const isMac = platform() === "macos";
    if (isMac && webview.label !== "main") {
      return;
    }

    const handleDeepLink = (payload: DeepLink) => {
      if (payload.to === "/billing/refresh") {
        void dismissInstruction();
      } else if (payload.to === "/meetings/open") {
        void openMeeting(payload.search.id).catch((error) => {
          console.error("Could not open meeting link", error);
          sonnerToast.error("Could not open meeting");
        });
      }
    };
    const deepLinkEvent = isMac
      ? deeplink2Events.deepLinkEvent(webview)
      : deeplink2Events.deepLinkEvent;
    const deepLinkSubscription = subscribeThenDrainDeepLinks({
      listen: (handler) =>
        deepLinkEvent.listen(({ payload }) => {
          handler(payload);
        }),
      takePendingDeepLinks: deeplink2Commands.takePendingDeepLinks,
      handle: handleDeepLink,
    });

    return () => {
      void deepLinkSubscription.then((fn) => fn()).catch(() => {});
    };
  });
}

async function openMeeting(id: string) {
  const window = await windowCommands.windowShow({ type: "main" });
  if (window.status === "error") throw new Error(window.error);

  const session = await commands.sessionGet(id);
  if (session.status === "error") throw new Error(session.error);
  if (!session.data) {
    sonnerToast.error("Meeting not found in this vault");
    return;
  }

  useTabs.getState().openNew({ type: "sessions", id });
}
