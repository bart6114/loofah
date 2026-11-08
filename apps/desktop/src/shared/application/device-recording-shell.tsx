import { useLingui } from "@lingui/react/macro";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { getIdentifier } from "@tauri-apps/api/app";
import { useEffect, useRef } from "react";

import {
  commands as detect,
  events as detectEvents,
} from "@hypr/plugin-detect";
import {
  commands as notifications,
  events as notificationEvents,
} from "@hypr/plugin-notification";
import {
  getCurrentWebviewWindowLabel,
  commands as windows,
  events as windowEvents,
} from "@hypr/plugin-windows";
import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { deviceCaptionPreview } from "./device-caption-preview";
import {
  activeDeviceCapture,
  useDeviceStartActions,
} from "./device-recording-controls";
import type { createDeviceRecordingMonitor } from "./device-recordings";

import {
  getFloatingOverlaySettings,
  getLiveCaptionDisplayText,
} from "~/meeting-float/overlay-settings";
import { getSettingsValuesFromNativeChange } from "~/meeting-float/settings-change";
import {
  initializeDeviceSettings,
  setSettingValue,
  setSettingValues,
  useSettingsReady,
} from "~/settings/queries";
import { createAsyncListenerScope } from "~/shared/async-listener-scope";
import { useConfigValues } from "~/shared/config";
import type { CaptureIdentity, DeviceRecordingInfo } from "~/types/tauri.gen";

export function DeviceRecordingShell(props: {
  profileId: string;
  monitor: ReturnType<typeof createDeviceRecordingMonitor>;
}) {
  return getCurrentWebviewWindowLabel() === "main" ? (
    <MainDeviceRecordingShell {...props} />
  ) : null;
}
function MainDeviceRecordingShell({
  profileId,
  monitor,
}: {
  profileId: string;
  monitor: ReturnType<typeof createDeviceRecordingMonitor>;
}) {
  const actions = useDeviceStartActions();
  const ready = useSettingsReady();
  const cache = useQueryClient();
  const options = useConfigValues([
    "notification_detect",
    "auto_stop_meetings",
    "floating_bar_enabled",
    "floating_bar_opacity",
    "ignored_platforms",
    "live_caption_opacity",
    "live_caption_width",
    "live_caption_line_count",
    "live_caption_position",
    "live_caption_minimized",
  ] as const);
  const key = ["device", "recordings", profileId];
  const inventory = useQuery({
    queryKey: key,
    queryFn: () => monitor.recoveryInventory(),
    networkMode: "always",
    staleTime: Infinity,
    retry: false,
  });
  const active = inventory.data?.find(activeDeviceCapture) ?? null;
  const latest = useRef({ actions, active, ready, options });
  latest.current = { actions, active, ready, options };
  const triggers = useRef(new Map<string, Set<string>>());
  useEffect(() => {
    void initializeDeviceSettings().catch((error) =>
      console.error("[device] preferences unavailable", error),
    );
  }, []);
  useEffect(() => {
    const lifetime = createAsyncListenerScope();
    let closed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const prompts = new Map<
      string,
      { start: NonNullable<typeof actions>["startPending"]; apps: string[] }
    >();
    const report = (error: unknown) => {
      if (!closed)
        sonnerToast.error(
          error instanceof Error ? error.message : "Device recording failed",
        );
    };
    const identityKey = (item: DeviceRecordingInfo) =>
      JSON.stringify(item.identity);
    const remember = (item: DeviceRecordingInfo, apps: string[]) => {
      const id = identityKey(item);
      if (triggers.current.size >= 8 && !triggers.current.has(id))
        triggers.current.delete(triggers.current.keys().next().value!);
      const known = triggers.current.get(id) ?? new Set<string>();
      for (const app of apps) known.add(app);
      triggers.current.set(id, known);
    };
    const clear = () => {
      if (timer) clearTimeout(timer);
      timer = undefined;
    };
    window.addEventListener("offline", clear);
    void lifetime
      .add(() =>
        detectEvents.detectEvent.listen(
          lifetime.guard(({ payload }) => {
            if (!latest.current.ready) return;
            if (payload.type === "micDetected") {
              clear();
              const apps = [
                ...new Set(
                  payload.apps
                    .map((app) => app.id)
                    .filter((id) => !!id && !id.startsWith("pid:")),
                ),
              ];
              const current = latest.current.active;
              if (current) {
                remember(current, apps);
                return;
              }
              const start = latest.current.actions?.startPending;
              if (
                !start ||
                !latest.current.options.notification_detect ||
                prompts.has(payload.key)
              )
                return;
              if (prompts.size >= 128)
                prompts.delete(prompts.keys().next().value!);
              prompts.set(payload.key, { start, apps });
              void notifications
                .showNotification({
                  key: payload.key,
                  title: "Are you in a meeting?",
                  message: "",
                  timeout: { secs: 30, nanos: 0 },
                  source: {
                    type: "mic_detected",
                    app_names: payload.apps.map((app) => app.name),
                    app_ids: apps,
                  },
                  start_time: null,
                  participants: null,
                  action_label: "Start recording",
                  action_variant: null,
                  options: null,
                  footer: {
                    text: "Ignore these apps",
                    actionLabel: "Always ignore",
                    icon: null,
                  },
                  icon: null,
                })
                .catch(report);
            } else if (payload.type === "micStopped") {
              clear();
              const item = latest.current.active;
              if (
                !item ||
                latest.current.options.auto_stop_meetings === false ||
                !navigator.onLine
              )
                return;
              const original = structuredClone(item.identity);
              const id = identityKey(item);
              const apps = triggers.current.get(id);
              const candidates = payload.apps
                .map((app) => app.id)
                .filter(
                  (app) => apps?.has(app) && app !== "com.kakao.KakaoTalkMac",
                );
              if (!candidates.length) return;
              timer = setTimeout(() => {
                void (async () => {
                  const unchanged = () =>
                    !closed &&
                    latest.current.options.auto_stop_meetings !== false &&
                    JSON.stringify(latest.current.active?.identity) === id;
                  if (!unchanged() || !navigator.onLine) return;
                  const mic = await detect.listMicUsingApplications();
                  if (
                    mic.status !== "ok" ||
                    mic.data.some((app) => apps!.has(app.id)) ||
                    !unchanged()
                  )
                    return;
                  await monitor.stop(original);
                  void cache.invalidateQueries({ queryKey: key });
                })().catch(report);
              }, 5000);
            } else if (payload.type === "sleepStateChanged") {
              clear();
              const item = latest.current.active;
              if (payload.value && item) {
                const original = structuredClone(item.identity);
                void monitor
                  .stop(original)
                  .then(() => cache.invalidateQueries({ queryKey: key }))
                  .catch(report);
              }
            }
          }),
        ),
      )
      .catch(report);
    void lifetime
      .add(() =>
        notificationEvents.notificationEvent.listen(
          lifetime.guard(({ payload }) => {
            const prompt = prompts.get(payload.key);
            if (!prompt) return;
            if (
              (payload.type === "notification_confirm" ||
                payload.type === "notification_accept") &&
              payload.source?.type === "mic_detected"
            ) {
              prompts.delete(payload.key);
              void prompt
                .start({ triggerAppIds: prompt.apps })
                .then((item) => remember(item, prompt.apps))
                .catch(report);
            } else if (payload.type === "notification_footer_action") {
              prompts.delete(payload.key);
              const ignored = latest.current.options.ignored_platforms ?? [];
              void setSettingValue(
                "ignored_platforms",
                JSON.stringify([...new Set([...ignored, ...prompt.apps])]),
              ).catch(report);
            } else if (payload.type === "notification_timeout")
              prompts.delete(payload.key);
          }),
        ),
      )
      .catch(report);
    return () => {
      closed = true;
      clear();
      window.removeEventListener("offline", clear);
      lifetime.dispose();
      prompts.clear();
    };
  }, [monitor, profileId, cache]);
  const activeKey = active ? JSON.stringify(active.identity) : null;
  useEffect(() => {
    if (!activeKey) return;
    let closed = false;
    void Promise.all([detect.listMicUsingApplications(), getIdentifier()])
      .then(([result, self]) => {
        if (closed || result.status !== "ok") return;
        const known = triggers.current.get(activeKey) ?? new Set<string>();
        for (const app of result.data)
          if (app.id && app.id !== self && !app.id.startsWith("pid:"))
            known.add(app.id);
        triggers.current.set(activeKey, known);
      })
      .catch(() => {});
    return () => {
      closed = true;
    };
  }, [activeKey]);
  return (
    <DeviceRecordingOverlay
      monitor={monitor}
      item={active}
      enabled={ready && options.floating_bar_enabled !== false}
      settings={getFloatingOverlaySettings(options)}
    />
  );
}

function DeviceRecordingOverlay({
  monitor,
  item,
  enabled,
  settings,
}: {
  monitor: ReturnType<typeof createDeviceRecordingMonitor>;
  item: DeviceRecordingInfo | null;
  enabled: boolean;
  settings: ReturnType<typeof getFloatingOverlaySettings>;
}) {
  const { t } = useLingui();
  const labels = {
    microphone: t`Microphone`,
    system_audio: t`System audio`,
    mixed: t`Mixed audio`,
  };
  const cache = useQueryClient();
  const latest = useRef({ item, enabled, settings, labels });
  latest.current = { item, enabled, settings, labels };
  const bindings = useRef(new Map<string, CaptureIdentity>());
  const binding = useRef<{
    key: string;
    identity: CaptureIdentity;
    fingerprint: string;
  } | null>(null);
  const fingerprint = item ? JSON.stringify(item.identity) : null;
  const captionGeneration = deviceCaptionPreview(item)?.generation ?? null;
  const operations = useRef(Promise.resolve());
  const enqueue = (work: () => Promise<void>) => {
    const next = operations.current.then(work, work);
    operations.current = next.catch(() => {});
    return next;
  };
  if (fingerprint && binding.current?.fingerprint !== fingerprint) {
    const key = crypto.randomUUID();
    const identity = structuredClone(item!.identity);
    binding.current = { key, identity, fingerprint };
    if (bindings.current.size >= 8)
      bindings.current.delete(bindings.current.keys().next().value!);
    bindings.current.set(key, identity);
  }
  useEffect(() => {
    const scope = createAsyncListenerScope();
    void scope
      .add(() =>
        windowEvents.floatingBarStop.listen(
          scope.guard(({ payload }) => {
            if (!payload.capture_key) return;
            const original = bindings.current.get(payload.capture_key);
            if (!original) return;
            void monitor
              .stop(structuredClone(original))
              .then(() =>
                cache.invalidateQueries({ queryKey: ["device", "recordings"] }),
              )
              .catch((error) => sonnerToast.error(error.message));
          }),
        ),
      )
      .catch(() => {});
    void scope
      .add(() =>
        windowEvents.floatingBarOpenMain.listen(
          scope.guard(() => {
            void windows.windowShow({ type: "main" });
          }),
        ),
      )
      .catch(() => {});
    void scope
      .add(() =>
        windowEvents.floatingBarSettingsChange.listen(
          scope.guard(({ payload }) => {
            void setSettingValues(
              getSettingsValuesFromNativeChange(payload),
            ).catch((error) => sonnerToast.error(error.message));
          }),
        ),
      )
      .catch(() => {});
    return scope.dispose;
  }, [monitor, cache]);
  useEffect(() => {
    if (!enabled || !fingerprint || !item || !activeDeviceCapture(item)) {
      void enqueue(async () => {
        await windows.floatingBarHide();
        await windows.liveCaptionHide();
      });
      return;
    }
    const original = binding.current!;
    let closed = false;
    let shown = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const update = async () => {
      try {
        const telemetry = await monitor.telemetry(original.identity);
        if (closed) return;
        await enqueue(async () => {
          if (closed) return;
          const value = latest.current;
          if (JSON.stringify(value.item?.identity) !== original.fingerprint)
            return;
          const captions = deviceCaptionPreview(value.item, value.labels);
          const settings = value.settings;
          await windows.floatingBarUpdate({
            captureKey: original.key,
            amplitude: Math.max(
              0,
              Math.min(
                1,
                Math.max(telemetry?.mic ?? 0, telemetry?.speaker ?? 0) / 1000,
              ),
            ),
            title: "Recording",
            status: value.item?.failure ? "error" : "recording",
            colorScheme: document.documentElement.classList.contains("dark")
              ? "dark"
              : "light",
            opacity: settings.floatingBarOpacity,
            liveCaptionOpacity: settings.liveCaptionOpacity,
            liveCaptionWidth: settings.liveCaptionWidth,
            liveCaptionLineCount: settings.liveCaptionLineCount,
            liveCaptionPosition: settings.liveCaptionPosition,
            liveCaptionMinimized: settings.liveCaptionMinimized,
            liveCaptionToggleVisible: !!captions,
            transcriptBubbles: captions?.bubbles ?? [],
          });
          if (
            closed ||
            JSON.stringify(latest.current.item?.identity) !==
              original.fingerprint
          )
            return;
          if (captions && !settings.liveCaptionMinimized) {
            await windows.liveCaptionUpdate({
              text: getLiveCaptionDisplayText(captions.text, settings),
              opacity: settings.liveCaptionOpacity,
              width: settings.liveCaptionWidth,
              lineCount: settings.liveCaptionLineCount,
              position: settings.liveCaptionPosition,
              minimized: false,
            });
            if (!closed) await windows.liveCaptionShow();
          } else await windows.liveCaptionHide();
          if (!closed && !shown) {
            await windows.floatingBarShow();
            shown = true;
          }
        });
      } finally {
        if (!closed)
          timer = setTimeout(() => {
            void update().catch(() => {});
          }, 100);
      }
    };
    void update().catch(() => {});
    return () => {
      closed = true;
      if (timer) clearTimeout(timer);
      void enqueue(async () => {
        await windows.floatingBarHide();
        await windows.liveCaptionHide();
      });
    };
  }, [monitor, enabled, fingerprint, captionGeneration]);
  return null;
}
