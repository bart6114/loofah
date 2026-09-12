import { type UnlistenFn } from "@tauri-apps/api/event";

import { events as notificationEvents } from "@hypr/plugin-notification";
import {
  commands as updaterCommands,
  events as updaterEvents,
} from "@hypr/plugin-updater2";
import { getCurrentWebviewWindowLabel } from "@hypr/plugin-windows";

import { createSession } from "~/session/queries";
import { setSettingValue } from "~/settings/queries";
import { useConfigValue, useConfigValues } from "~/shared/config";
import { useLatestRef } from "~/shared/hooks/useLatestRef";
import { useMountEffect } from "~/shared/hooks/useMountEffect";
import { subscribeIndexChanged } from "~/shared/index-query";
import { DEFAULT_USER_ID } from "~/shared/utils";
import { useAudioImport } from "~/store/zustand/audio-import";
import { listenerStore } from "~/store/zustand/listener/instance";
import { useTabs } from "~/store/zustand/tabs";
import { AUDIO_IMPORT_COMPLETED_NOTIFICATION_KEY } from "~/stt/audio-import-notification";
import { parseAutoStopEndedNotificationKey } from "~/stt/auto-stop-notification";
import { parseBatchCompletedNotificationKey } from "~/stt/batch-completed-notification";
import {
  getLiveTranscriptionConfig,
  getTranscriptionLanguages,
} from "~/stt/capabilities";
import { commands } from "~/types/tauri.gen";

const LIVE_CAPTURE_CONFIG_DEBOUNCE_MS = 750;

async function createNotificationSession(
  triggerAppIds: string[] | null,
): Promise<{ sessionId: string; autoStart: boolean }> {
  const sessionId = await createSession();

  if (triggerAppIds && triggerAppIds.length > 0) {
    listenerStore.getState().setTriggerAppIds(triggerAppIds);
  }

  return {
    sessionId,
    autoStart: true,
  };
}

function handleAutoStopEndedNotification(
  type: "notification_confirm" | "notification_accept" | "notification_timeout",
  key: string,
): boolean {
  const sessionId = parseAutoStopEndedNotificationKey(key);
  if (!sessionId) {
    return false;
  }

  if (type === "notification_confirm") {
    return true;
  }

  const listenerState = listenerStore.getState();
  if (
    listenerState.live.status === "active" &&
    listenerState.live.sessionId === sessionId
  ) {
    listenerState.stop();
  }

  return true;
}

function createCaptureConfigSignature(config: {
  session_id: string;
  languages: string[];
  participant_human_ids: string[];
  self_human_id: string | null;
}) {
  return JSON.stringify(config);
}

function parseStringArray(value: unknown, fallback: string[]) {
  if (Array.isArray(value)) {
    return value.filter((item): item is string => typeof item === "string");
  }

  if (typeof value !== "string") {
    return fallback;
  }

  try {
    const parsed = JSON.parse(value);
    return Array.isArray(parsed)
      ? parsed.filter((item): item is string => typeof item === "string")
      : fallback;
  } catch {
    return fallback;
  }
}

function getLiveConfigLanguages(
  aiLanguage: string,
  spokenLanguages: string[],
  meetingLanguages?: string[],
) {
  return getTranscriptionLanguages(
    aiLanguage || undefined,
    parseStringArray(spokenLanguages, []),
    meetingLanguages,
  );
}

function LiveCaptureConfigSync() {
  const settingsValues = useConfigValues([
    "ai_language",
    "spoken_languages",
    "meeting_languages",
    "current_stt_provider",
    "current_stt_model",
  ] as const);

  const settingsSignature = JSON.stringify(settingsValues);
  return (
    <LiveCaptureConfigSyncReady
      key={settingsSignature}
      settingsValues={settingsValues}
    />
  );
}

function LiveCaptureConfigSyncReady({
  settingsValues,
}: {
  settingsValues: {
    ai_language: string;
    spoken_languages: string[];
    meeting_languages: string[] | undefined;
    current_stt_provider: string | undefined;
    current_stt_model: string | undefined;
  };
}) {
  useMountEffect(() => {
    let timeoutId: ReturnType<typeof setTimeout> | null = null;
    let lastSignature: string | null = null;
    let sessionIds = new Set<string>();
    let hasSnapshot = false;

    const pushConfig = async () => {
      if (!hasSnapshot) {
        return;
      }

      const live = listenerStore.getState().live;
      if (live.status !== "active" || !live.sessionId) {
        return;
      }

      const languages = getLiveConfigLanguages(
        settingsValues.ai_language,
        settingsValues.spoken_languages,
        settingsValues.meeting_languages,
      );
      const liveConfig = await getLiveTranscriptionConfig({
        provider: settingsValues.current_stt_provider,
        model: settingsValues.current_stt_model,
        languages,
      });

      if (liveConfig.transcriptionMode === "batch") {
        return;
      }

      const nextConfig = {
        session_id: live.sessionId,
        languages: liveConfig.languages,
        participant_human_ids: [],
        // The owner concept died with the workspaces removal (D10); the indexed
        // session's existence is all that's left to check.
        self_human_id: sessionIds.has(live.sessionId) ? DEFAULT_USER_ID : null,
      };
      const signature = createCaptureConfigSignature(nextConfig);
      if (signature === lastSignature) {
        return;
      }

      lastSignature = signature;
      await listenerStore.getState().updateCaptureConfig(nextConfig);
    };

    const schedulePush = () => {
      if (timeoutId) {
        clearTimeout(timeoutId);
      }

      timeoutId = setTimeout(() => {
        void pushConfig().catch((error) => {
          console.error(
            "[listener] failed to update live capture config",
            error,
          );
        });
      }, LIVE_CAPTURE_CONFIG_DEBOUNCE_MS);
    };

    const refreshSessionIds = async () => {
      const result = await commands.sessionIds();
      if (result.status === "error") {
        throw new Error(result.error);
      }
      sessionIds = new Set(result.data);
      hasSnapshot = true;
      schedulePush();
    };

    const handleRefreshError = (error: unknown) => {
      console.error("[listener] failed to read live capture identities", error);
    };

    const unsubscribeListener = listenerStore.subscribe(schedulePush);
    const unsubscribeIndex = subscribeIndexChanged("sessions", () => {
      void refreshSessionIds().catch(handleRefreshError);
    });
    void refreshSessionIds().catch(handleRefreshError);

    return () => {
      if (timeoutId) {
        clearTimeout(timeoutId);
      }
      unsubscribeListener();
      unsubscribeIndex();
    };
  });

  return null;
}

function useUpdaterEvents() {
  const openNew = useTabs((state) => state.openNew);
  const openNewRef = useLatestRef(openNew);

  useMountEffect(() => {
    if (getCurrentWebviewWindowLabel() !== "main") {
      return;
    }

    let unlisten: UnlistenFn | null = null;

    void updaterEvents.updatedEvent
      .listen(({ payload: { previous, current } }) => {
        openNewRef.current({
          type: "changelog",
          state: { previous, current },
        });
      })
      .then(async (f) => {
        unlisten = f;
        await updaterCommands.maybeEmitUpdated();
      });

    return () => {
      unlisten?.();
    };
  });
}

function useNotificationEvents() {
  const ignoredPlatforms = useConfigValue("ignored_platforms");
  const openNew = useTabs((state) => state.openNew);
  const ignoredPlatformsRef = useLatestRef(ignoredPlatforms);
  const openNewRef = useLatestRef(openNew);

  useMountEffect(() => {
    if (getCurrentWebviewWindowLabel() !== "main") {
      return;
    }

    let unlisten: UnlistenFn | null = null;
    let cancelled = false;

    void notificationEvents.notificationEvent
      .listen(({ payload }) => {
        if (
          payload.type === "notification_confirm" ||
          payload.type === "notification_accept" ||
          payload.type === "notification_timeout"
        ) {
          if (handleAutoStopEndedNotification(payload.type, payload.key)) {
            return;
          }

          if (payload.type === "notification_timeout") {
            return;
          }

          const sourceSessionId =
            payload.source?.type === "session"
              ? payload.source.session_id
              : parseBatchCompletedNotificationKey(payload.key);
          if (sourceSessionId) {
            openNewRef.current({
              type: "sessions",
              id: sourceSessionId,
              state: { view: null, autoStart: null },
            });
            return;
          }

          if (payload.key === AUDIO_IMPORT_COMPLETED_NOTIFICATION_KEY) {
            useAudioImport.getState().setDialogOpen(true);
            return;
          }

          // Only meeting-detection notifications may start a recording;
          // anything unrecognized just focuses the app (the Rust handler
          // already showed the main window).
          if (payload.source?.type !== "mic_detected") {
            return;
          }

          void createNotificationSession(payload.source.app_ids ?? null)
            .then(({ sessionId, autoStart }) => {
              openNewRef.current({
                type: "sessions",
                id: sessionId,
                state: { view: null, autoStart: autoStart ? true : null },
              });
            })
            .catch((error) => {
              console.error(
                "[notification] failed to open notification session",
                error,
              );
            });
        } else if (payload.type === "notification_option_selected") {
          const sessionPromise = createSession();

          if (payload.source?.type === "mic_detected") {
            const triggerAppIds = payload.source.app_ids ?? [];
            listenerStore
              .getState()
              .setTriggerAppIds(
                triggerAppIds.length > 0 ? triggerAppIds : null,
              );
          }

          void sessionPromise
            .then((sessionId) => {
              openNewRef.current({
                type: "sessions",
                id: sessionId,
                state: { view: null, autoStart: true },
              });
            })
            .catch((error) => {
              console.error(
                "[notification] failed to open selected event",
                error,
              );
            });
        } else if (payload.type === "notification_footer_action") {
          if (payload.source?.type !== "mic_detected") {
            return;
          }

          const appIds = payload.source.app_ids ?? [];
          if (appIds.length === 0) {
            return;
          }

          const ignoredPlatforms = ignoredPlatformsRef.current;
          const nextIgnoredPlatforms = [
            ...new Set([...ignoredPlatforms, ...appIds]),
          ];

          if (nextIgnoredPlatforms.length === ignoredPlatforms.length) {
            return;
          }

          void setSettingValue(
            "ignored_platforms",
            JSON.stringify(nextIgnoredPlatforms),
          ).catch((error) => {
            console.error("[notification] failed to ignore platforms", error);
          });
        }
      })
      .then((f) => {
        if (cancelled) {
          f();
        } else {
          unlisten = f;
        }
      });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  });
}

export function EventListeners() {
  return (
    <>
      <EventListenersInner />
      <LiveCaptureConfigSync />
    </>
  );
}

function EventListenersInner() {
  useUpdaterEvents();
  useNotificationEvents();

  return null;
}
