import { resolveResource } from "@tauri-apps/api/path";
import React, { createContext, useContext, useRef } from "react";
import { useStore } from "zustand";
import { useShallow } from "zustand/shallow";

import {
  commands as detectCommands,
  events as detectEvents,
} from "@hypr/plugin-detect";
import {
  commands as notificationCommands,
  type NotificationIcon,
} from "@hypr/plugin-notification";

import { useConfigValue } from "~/shared/config";
import { useMountEffect } from "~/shared/hooks/useMountEffect";
import {
  createListenerStore,
  type ListenerStore,
} from "~/store/zustand/listener";

const ListenerContext = createContext<ListenerStore | null>(null);
export const AUTO_STOP_CONFIRM_DELAY_MS = 5_000;

const MAX_TIMEOUT_DELAY_MS = 2_147_483_647;

const UNRELIABLE_AUTO_STOP_APP_IDS = new Set(["com.kakao.KakaoTalkMac"]);

type MicApp = { id: string; name: string };
type PendingAutoStop = {
  timeout?: ReturnType<typeof setTimeout>;
  requireMicSnapshot: boolean;
  sessionId: string | null;
  networkInterrupted: boolean;
};

const NOTIFICATION_ICON_RESOURCES = {
  discord: "notification-icons/discord.png",
  kakaotalk: "notification-icons/kakaotalk.png",
  line: "notification-icons/line.png",
  messenger: "notification-icons/messenger.png",
  microsoftTeams: "notification-icons/microsoft-teams.svg",
  phone: "notification-icons/phone.png",
  signal: "notification-icons/signal.png",
  slack: "notification-icons/slack.svg",
  telegram: "notification-icons/telegram.png",
  webex: "notification-icons/webex.svg",
  whatsapp: "notification-icons/whatsapp.png",
  zoom: "notification-icons/zoom.svg",
} as const;

type NotificationIconResource = keyof typeof NOTIFICATION_ICON_RESOURCES;

const notificationIconResourceCache = new Map<
  NotificationIconResource,
  Promise<NotificationIcon | null>
>();

type MicAppNotificationOverride = {
  ids: Set<string>;
  names: Set<string>;
  displayName: string;
  icon?: NotificationIcon;
  iconResource?: NotificationIconResource;
};

const MIC_APP_NOTIFICATION_OVERRIDES = [
  {
    ids: new Set([
      "/usr/libexec/avconferenced",
      "com.apple.avconferenced",
      "com.apple.TelephonyUtilities",
      "com.apple.TelephonyUtilities.callservicesd",
    ]),
    names: new Set(["av capture", "avcapture", "avconferenced", "iphone call"]),
    displayName: "iPhone Call",
    iconResource: "phone",
  },
  {
    ids: new Set(["com.apple.FaceTime"]),
    names: new Set(["facetime"]),
    displayName: "FaceTime",
    icon: {
      type: "bundle_id",
      bundle_id: "com.apple.FaceTime",
    } satisfies NotificationIcon,
  },
  {
    ids: new Set(["us.zoom.xos"]),
    names: new Set(["zoom", "zoom helper", "zoom workplace"]),
    displayName: "Zoom",
    iconResource: "zoom",
  },
  {
    ids: new Set(["com.microsoft.teams", "com.microsoft.teams2"]),
    names: new Set([
      "microsoft teams",
      "microsoft teams helper",
      "teams",
      "teams helper",
    ]),
    displayName: "Microsoft Teams",
    iconResource: "microsoftTeams",
  },
  {
    ids: new Set([
      "Cisco-Systems.Spark",
      "com.cisco.webex",
      "com.cisco.webexmeetingsapp",
    ]),
    names: new Set(["cisco webex", "webex", "webex helper", "webex meetings"]),
    displayName: "Webex",
    iconResource: "webex",
  },
  {
    ids: new Set(["com.slack.Slack", "com.tinyspeck.slackmacgap"]),
    names: new Set(["slack", "slack helper"]),
    displayName: "Slack",
    iconResource: "slack",
  },
  {
    ids: new Set(["com.kakao.KakaoTalkMac"]),
    names: new Set(["kakaotalk", "kakaotalk helper"]),
    displayName: "KakaoTalk",
    iconResource: "kakaotalk",
  },
  {
    ids: new Set(["net.whatsapp.WhatsApp"]),
    names: new Set(["whatsapp", "whatsapp helper"]),
    displayName: "WhatsApp",
    iconResource: "whatsapp",
  },
  {
    ids: new Set(["com.hnc.Discord", "com.discordapp.Discord"]),
    names: new Set(["discord", "discord helper"]),
    displayName: "Discord",
    iconResource: "discord",
  },
  {
    ids: new Set(["org.whispersystems.signal-desktop"]),
    names: new Set(["signal", "signal helper"]),
    displayName: "Signal",
    iconResource: "signal",
  },
  {
    ids: new Set(["ru.keepcoder.Telegram", "ru.keepcoder.TelegramLite"]),
    names: new Set(["telegram", "telegram helper", "telegram lite"]),
    displayName: "Telegram",
    iconResource: "telegram",
  },
  {
    ids: new Set(["jp.naver.line.mac"]),
    names: new Set(["line", "line helper"]),
    displayName: "LINE",
    iconResource: "line",
  },
  {
    ids: new Set(["com.facebook.archon"]),
    names: new Set(["messenger", "messenger helper"]),
    displayName: "Messenger",
    iconResource: "messenger",
  },
] satisfies MicAppNotificationOverride[];

function getMicAppNotificationOverride(app: MicApp) {
  const normalizedName = app.name.trim().toLowerCase();
  return MIC_APP_NOTIFICATION_OVERRIDES.find(
    (override) =>
      override.ids.has(app.id) || override.names.has(normalizedName),
  );
}

function getNotificationResourceIcon(
  resource: NotificationIconResource,
): Promise<NotificationIcon | null> {
  const cached = notificationIconResourceCache.get(resource);
  if (cached) {
    return cached;
  }

  const promise = resolveResource(NOTIFICATION_ICON_RESOURCES[resource])
    .then((path): NotificationIcon => ({ type: "path", path }))
    .catch(() => null);

  notificationIconResourceCache.set(resource, promise);
  return promise;
}

function getNotificationIconForAppId(appId: string): NotificationIcon | null {
  if (!appId || appId.startsWith("pid:")) {
    return null;
  }

  if (appId.startsWith("/") || appId.startsWith("~/")) {
    return { type: "path", path: appId };
  }

  return { type: "bundle_id", bundle_id: appId };
}

async function getNotificationIconForApp(
  app: MicApp,
): Promise<NotificationIcon | null> {
  const override = getMicAppNotificationOverride(app);
  if (override?.iconResource) {
    const icon = await getNotificationResourceIcon(override.iconResource);
    if (icon) {
      return icon;
    }
  }

  return override?.icon ?? getNotificationIconForAppId(app.id);
}

function getNotificationAppName(app: MicApp) {
  return getMicAppNotificationOverride(app)?.displayName ?? app.name;
}

function getIgnorableApps(apps: MicApp[]) {
  const seen = new Set<string>();

  return apps.filter((app) => {
    if (!app.id || app.id.startsWith("pid:") || seen.has(app.id)) {
      return false;
    }

    seen.add(app.id);
    return true;
  });
}

function getIgnoreAppsFooterText(apps: MicApp[]) {
  const firstName = apps[0] ? getNotificationAppName(apps[0]).trim() : "";

  if (apps.length === 1) {
    return firstName || "This app";
  }

  if (!firstName) {
    return "These apps";
  }

  const secondName = apps[1] ? getNotificationAppName(apps[1]).trim() : "";
  if (apps.length === 2 && secondName) {
    return `${firstName} and ${secondName}`;
  }

  const otherAppCount = apps.length - 1;
  return `${firstName} and ${otherAppCount} other app${otherAppCount === 1 ? "" : "s"}`;
}

function getAutoStopCandidateAppIds(
  triggerAppIds: string[] | null | undefined,
  stoppedApps: { id: string }[],
) {
  const trigger = triggerAppIds ?? [];
  const stoppedIds = new Set(stoppedApps.map((app) => app.id));
  const stoppedTriggerAppIds = trigger.filter((id) => stoppedIds.has(id));
  const candidateAppIds =
    stoppedTriggerAppIds.length > 0 ? stoppedTriggerAppIds : trigger;

  return candidateAppIds.filter((id) => !UNRELIABLE_AUTO_STOP_APP_IDS.has(id));
}

function getAutoStopActiveCheckAppIds(
  triggerAppIds: string[] | null | undefined,
  candidateAppIds: string[],
) {
  const unreliableTriggerAppIds =
    triggerAppIds?.filter((id) => UNRELIABLE_AUTO_STOP_APP_IDS.has(id)) ?? [];

  return [...new Set([...candidateAppIds, ...unreliableTriggerAppIds])];
}

export const ListenerProvider = ({
  children,
  store,
}: {
  children: React.ReactNode;
  store: ListenerStore;
}) => {
  useHandleDetectEvents(store);

  const storeRef = useRef<ListenerStore | null>(null);
  if (!storeRef.current) {
    storeRef.current = store;
  }

  return (
    <ListenerContext.Provider value={storeRef.current}>
      {children}
    </ListenerContext.Provider>
  );
};

export const useListener = <T,>(
  selector: Parameters<
    typeof useStore<ReturnType<typeof createListenerStore>, T>
  >[1],
) => {
  const store = useContext(ListenerContext);

  if (!store) {
    throw new Error("'useListener' must be used within a 'ListenerProvider'");
  }

  return useStore(store, useShallow(selector));
};

const useHandleDetectEvents = (store: ListenerStore) => {
  const stop = useStore(store, (state) => state.stop);
  const autoStopMeetings = useConfigValue("auto_stop_meetings");
  const notificationDetect = useConfigValue("notification_detect");

  const autoStopMeetingsRef = useRef(autoStopMeetings);
  autoStopMeetingsRef.current = autoStopMeetings;
  const notificationDetectRef = useRef(notificationDetect);
  notificationDetectRef.current = notificationDetect;
  const isOnlineRef = useRef(true);
  const pendingAutoStopRef = useRef<PendingAutoStop | null>(null);
  const pendingMicDetectedPromptRef = useRef(false);

  useMountEffect(() => {
    let unlistenDetect: (() => void) | undefined;
    let cancelled = false;
    isOnlineRef.current = navigator.onLine;
    const clearPendingAutoStop = () => {
      if (pendingAutoStopRef.current) {
        if (pendingAutoStopRef.current.timeout) {
          clearTimeout(pendingAutoStopRef.current.timeout);
        }
        pendingAutoStopRef.current = null;
      }
    };
    const shouldCaptureMicDetectedTriggerApps = () => {
      const live = store.getState().live;
      return (
        live.status === "active" ||
        (live.status === "inactive" && live.loading && !!live.sessionId)
      );
    };
    const captureTriggerAppIds = (appIds: string[]) => {
      if (appIds.length === 0) {
        return;
      }

      const currentTrigger = store.getState().live.triggerAppIds ?? [];
      if (appIds.some((id) => currentTrigger.includes(id))) {
        clearPendingAutoStop();
      }
      store
        .getState()
        .setTriggerAppIds([...new Set([...currentTrigger, ...appIds])]);
    };

    function scheduleAutoStop(
      delayMs: number,
      candidateAppIds: string[],
      requireMicSnapshot: boolean,
      sessionId: string | null,
      networkInterrupted: boolean,
    ) {
      clearPendingAutoStop();

      const pending: PendingAutoStop = {
        requireMicSnapshot,
        sessionId,
        networkInterrupted,
      };
      pending.timeout = setTimeout(
        () => {
          void confirmAutoStop(candidateAppIds, pending).finally(() => {
            if (pendingAutoStopRef.current === pending) {
              pendingAutoStopRef.current = null;
            }
          });
        },
        Math.min(Math.max(delayMs, 0), MAX_TIMEOUT_DELAY_MS),
      );
      pendingAutoStopRef.current = pending;
    }

    async function confirmAutoStop(
      candidateAppIds: string[],
      pending: PendingAutoStop,
    ) {
      const live = store.getState().live;
      if (
        pendingAutoStopRef.current !== pending ||
        live.status !== "active" ||
        live.sessionId !== pending.sessionId
      ) {
        return;
      }

      const currentTrigger = live.triggerAppIds;
      if (
        !currentTrigger ||
        !candidateAppIds.some((id) => currentTrigger.includes(id))
      ) {
        return;
      }

      const activeCheckAppIds = getAutoStopActiveCheckAppIds(
        currentTrigger,
        candidateAppIds,
      );
      const hasUnreliableActiveCheckApp = activeCheckAppIds.some(
        (id) => !candidateAppIds.includes(id),
      );
      const result = await detectCommands.listMicUsingApplications();
      if (result.status === "ok") {
        const activeAppIds = new Set(result.data.map((app) => app.id));
        if (activeCheckAppIds.some((id) => activeAppIds.has(id))) {
          return;
        }
      } else if (pending.requireMicSnapshot || hasUnreliableActiveCheckApp) {
        return;
      }

      if (pendingAutoStopRef.current !== pending) {
        return;
      }

      const currentLive = store.getState().live;
      if (
        pendingAutoStopRef.current !== pending ||
        currentLive.status !== "active" ||
        currentLive.sessionId !== pending.sessionId
      ) {
        return;
      }

      stop();
    }

    const handleOffline = () => {
      isOnlineRef.current = false;
      if (pendingAutoStopRef.current) {
        pendingAutoStopRef.current.networkInterrupted = true;
      }
    };
    const handleOnline = () => {
      isOnlineRef.current = true;
    };
    window.addEventListener("offline", handleOffline);
    window.addEventListener("online", handleOnline);

    detectEvents.detectEvent
      .listen(({ payload }) => {
        if (payload.type === "micDetected") {
          const ignorableApps = getIgnorableApps(payload.apps);
          const appIds = ignorableApps.map((app) => app.id);

          if (shouldCaptureMicDetectedTriggerApps()) {
            captureTriggerAppIds(appIds);
            return;
          }

          if (!notificationDetectRef.current) {
            return;
          }

          if (pendingMicDetectedPromptRef.current) {
            return;
          }
          pendingMicDetectedPromptRef.current = true;

          void (async () => {
            try {
              const footerIcon =
                ignorableApps.length > 0
                  ? await getNotificationIconForApp(ignorableApps[0]!)
                  : null;
              const footer =
                ignorableApps.length > 0
                  ? {
                      text: getIgnoreAppsFooterText(ignorableApps),
                      actionLabel: "Always ignore",
                      icon: footerIcon,
                    }
                  : null;

              if (shouldCaptureMicDetectedTriggerApps()) {
                captureTriggerAppIds(appIds);
                return;
              }

              await notificationCommands.showNotification({
                key: payload.key,
                title: "Are you in a meeting?",
                message: "",
                timeout: { secs: 30, nanos: 0 },
                source: {
                  type: "mic_detected",
                  app_names: payload.apps.map((app) =>
                    getNotificationAppName(app),
                  ),
                  app_ids: appIds,
                },
                start_time: null,
                participants: null,
                action_label: "Start recording",
                action_variant: null,
                options: null,
                footer,
                icon: null,
              });
            } finally {
              pendingMicDetectedPromptRef.current = false;
            }
          })();
        } else if (payload.type === "micStopped") {
          const autoStopEnabled = autoStopMeetingsRef.current !== false;
          if (!autoStopEnabled) {
            return;
          }

          const trigger = store.getState().live.triggerAppIds;
          const stoppedTriggerAppIds =
            trigger?.filter((id) =>
              payload.apps.some((app) => app.id === id),
            ) ?? [];
          const candidateAppIds = getAutoStopCandidateAppIds(
            trigger,
            payload.apps,
          );
          if (candidateAppIds.length > 0) {
            const requireMicSnapshot = stoppedTriggerAppIds.length === 0;
            if (
              pendingAutoStopRef.current &&
              !pendingAutoStopRef.current.requireMicSnapshot &&
              requireMicSnapshot
            ) {
              return;
            }

            scheduleAutoStop(
              AUTO_STOP_CONFIRM_DELAY_MS,
              candidateAppIds,
              requireMicSnapshot,
              store.getState().live.sessionId,
              !isOnlineRef.current,
            );
          }
        } else if (payload.type === "sleepStateChanged") {
          if (payload.value) {
            clearPendingAutoStop();
            stop();
          }
        }
      })
      .then((fn) => {
        if (cancelled) {
          fn();
        } else {
          unlistenDetect = fn;
        }
      })
      .catch((err) => {
        console.error("Failed to setup detect event listener:", err);
      });

    return () => {
      cancelled = true;
      clearPendingAutoStop();
      window.removeEventListener("offline", handleOffline);
      window.removeEventListener("online", handleOnline);
      unlistenDetect?.();
    };
  });
};
