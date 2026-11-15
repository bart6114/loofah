import { useLingui } from "@lingui/react/macro";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef } from "react";

import { commands as notificationCommands } from "@hypr/plugin-notification";
import { commands as permissionsCommands } from "@hypr/plugin-permissions";
import { commands as windowsCommands } from "@hypr/plugin-windows";

import {
  acceptDictationStatus,
  configureDictationShortcut,
  dictationQueryKey,
  isDictationActive,
  isDictationCapturing,
  useDictationShortcut,
  useDictationStatus,
} from "./state";

import { useConfigValues } from "~/shared/config";
import { listenerStore } from "~/store/zustand/listener/instance";
import { useTabs } from "~/store/zustand/tabs";
import type { DictationStatus } from "~/types/tauri.gen";

export function DictationHost() {
  const { t } = useLingui();
  const client = useQueryClient();
  useEffect(() => {
    void invoke("dictation_set_copy_label", {
      label: t`Copy Last Dictation`,
    }).catch(console.error);
  }, [t]);
  const { data: status } = useDictationStatus();
  const capturing = useDictationShortcut((state) => state.capturing);
  const {
    dictation_enabled,
    dictation_shortcut,
    dictation_trigger_mode,
    floating_bar_opacity,
  } = useConfigValues([
    "dictation_enabled",
    "dictation_shortcut",
    "dictation_trigger_mode",
    "floating_bar_opacity",
  ]);
  const latest = useRef({
    enabled: dictation_enabled,
    mode: dictation_trigger_mode,
    capturing,
  });
  latest.current = {
    enabled: dictation_enabled,
    mode: dictation_trigger_mode,
    capturing,
  };
  const queue = useRef(Promise.resolve());
  const meetingRequestRevision = useRef<number | null>(null);
  const notifiedMeetingAttempt = useRef<number | null>(null);
  const configure = useMutation({
    mutationFn: configureDictationShortcut,
    onSuccess: () => useDictationShortcut.setState({ unavailable: false }),
    onError: () => useDictationShortcut.setState({ unavailable: true }),
  });
  const configureRef = useRef(configure.mutate);
  configureRef.current = configure.mutate;

  useEffect(() => {
    const register = () =>
      configureRef.current(
        dictation_enabled && !capturing ? dictation_shortcut : "",
      );
    register();
    window.addEventListener("focus", register);
    return () => window.removeEventListener("focus", register);
  }, [dictation_enabled, dictation_shortcut, capturing]);

  useEffect(() => {
    let disposed = false;
    const run = (action: string, attemptId?: number) => {
      queue.current = queue.current
        .then(async () => {
          if (disposed) return;
          const current =
            client.getQueryData<DictationStatus>(dictationQueryKey);
          if (attemptId !== undefined && current?.attemptId !== attemptId)
            return;
          if (action === "keyDown" || action === "keyUp") {
            if (!latest.current.enabled || latest.current.capturing) return;
            if (action === "keyUp") {
              if (
                latest.current.mode !== "hold" ||
                !isDictationCapturing(current)
              )
                return;
              action = "stop";
            } else {
              action =
                latest.current.mode === "toggle" &&
                isDictationCapturing(current)
                  ? "stop"
                  : "start";
            }
          }
          if (action === "stop" && !isDictationCapturing(current)) return;
          if (action === "cancel" && !isDictationActive(current)) return;
          if (action === "settings") {
            useTabs.getState().openNew({
              type: "settings",
              state: {
                tab:
                  current?.problem === "modelRequired"
                    ? "transcription"
                    : "app",
              },
            });
            await windowsCommands.windowShow({ type: "main" });
            return;
          }
          if (action === "permission") {
            const permission =
              current?.problem === "accessibilityRequired"
                ? "accessibility"
                : "microphone";
            const checked =
              await permissionsCommands.checkPermission(permission);
            if (checked.status === "ok" && checked.data === "neverRequested")
              await permissionsCommands.requestPermission(permission);
            else await permissionsCommands.openPermission(permission);
            return;
          }
          if (
            !["start", "stop", "cancel", "retry", "copy", "discard"].includes(
              action,
            )
          )
            return;
          if (action === "start") {
            meetingRequestRevision.current = meetingOwnsBar()
              ? (current?.revision ?? -1)
              : null;
          }
          const next = await invoke<DictationStatus>(
            `dictation_${action}`,
            action === "start" ? {} : { attemptId: current?.attemptId },
          );
          acceptDictationStatus(client, next);
        })
        .catch((error) => {
          console.error("Dictation command failed", error);
          void client.invalidateQueries({ queryKey: dictationQueryKey });
        });
    };
    const subscriptions = [
      listen<DictationStatus>("dictation-status", ({ payload }) => {
        if (!disposed) acceptDictationStatus(client, payload);
      }),
      listen("plugin:windows:dictation-shortcut-down", () => run("keyDown")),
      listen("plugin:windows:dictation-shortcut-up", () => run("keyUp")),
      listen("plugin:windows:dictation-cancel", () => run("cancel")),
      listen("plugin:windows:floating-bar-stop", () => run("stop")),
      listen<{ attemptId: number; action: string }>(
        "plugin:windows:dictation-action",
        ({ payload }) => run(payload.action, payload.attemptId),
      ),
    ];
    return () => {
      disposed = true;
      void Promise.all(subscriptions).then((unlisteners) =>
        unlisteners.forEach((unlisten) => unlisten()),
      );
    };
  }, [client]);

  const shown = useRef(false);
  const renderQueue = useRef(Promise.resolve());
  const meetingOwnsBar = () => {
    const { live } = listenerStore.getState();
    return (
      live.loading ||
      live.status !== "inactive" ||
      Object.keys(live.finalizingBySession).length > 0
    );
  };
  useEffect(
    () =>
      listenerStore.subscribe(() => {
        if (meetingOwnsBar()) shown.current = false;
      }),
    [],
  );
  useEffect(() => {
    if (!status) return;
    if (meetingOwnsBar()) {
      if (
        status.phase === "error" &&
        meetingRequestRevision.current !== null &&
        status.revision > meetingRequestRevision.current &&
        notifiedMeetingAttempt.current !== status.attemptId
      ) {
        meetingRequestRevision.current = null;
        notifiedMeetingAttempt.current = status.attemptId;
        const notify = async () => {
          const result = await notificationCommands.showNotification({
            key: "dictation-meeting-active",
            title: t`A meeting is recording`,
            message: t`Try dictation after the meeting finishes.`,
            timeout: { secs: 4, nanos: 0 },
            source: null,
            start_time: null,
            participants: null,
            action_label: null,
            action_variant: null,
            options: null,
            footer: null,
            icon: null,
          });
          if (result.status === "error") console.error(result.error);
        };
        void notify().catch(console.error);
      }
      return;
    }
    const currentStatus = status;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const hide = () => {
      renderQueue.current = renderQueue.current
        .then(async () => {
          const current =
            client.getQueryData<DictationStatus>(dictationQueryKey);
          if (
            meetingOwnsBar() ||
            current?.revision !== currentStatus.revision ||
            !shown.current
          )
            return;
          shown.current = false;
          await windowsCommands.floatingBarHide();
        })
        .catch(console.error);
    };
    if (status.phase === "idle") {
      hide();
      return;
    }
    const actions: Array<{ id: string; label: string }> = [];
    let detail: string | null = null;
    let title = t`Transcribing…`;
    switch (status.phase) {
      case "starting":
        title = t`Starting microphone…`;
        break;
      case "recording":
        title = t`Listening…`;
        actions.push({ id: "stop", label: t`Stop` });
        break;
      case "loadingModel":
        title = t`Loading model…`;
        break;
      case "inserting":
        title = t`Inserting…`;
        break;
      case "copied":
        title = t`Copied`;
        break;
      case "done":
        title = t`Done`;
        break;
      case "cancelled":
        title = t`Cancelled`;
        break;
      case "ready":
        title = t`Your text is ready`;
        break;
      case "error":
        title = t`Dictation couldn’t finish`;
        break;
    }
    if (isDictationActive(status))
      actions.push({ id: "cancel", label: t`Cancel` });
    switch (status.problem) {
      case "busy":
        title = t`Finishing your previous dictation…`;
        break;
      case "disabled":
        title = t`Dictation is turned off`;
        actions.push({ id: "settings", label: t`Settings` });
        break;
      case "meetingActive":
        title = t`A meeting is recording`;
        detail = t`Try dictation after the meeting finishes.`;
        break;
      case "modelRequired":
        title = t`Choose a transcription model`;
        actions.push({ id: "settings", label: t`Choose model` });
        break;
      case "microphone":
        title = t`Couldn’t use the microphone`;
        detail = t`Check microphone access and your input device.`;
        actions.push({ id: "permission", label: t`Microphone settings` });
        break;
      case "accessibilityRequired":
        title = t`Allow Loofah to insert text`;
        detail = t`Enable Loofah in Accessibility, then try again.`;
        actions.push({ id: "permission", label: t`Open Settings` });
        break;
      case "targetUnavailable":
        title = t`Click where you want to type`;
        detail = t`Then use your dictation shortcut again.`;
        break;
      case "transcriptionFailed":
        title = t`Couldn’t transcribe`;
        detail = t`Your recording is available to retry.`;
        break;
      case "noSpeech":
        title = t`No speech detected. Try again.`;
        break;
      case "insertionFailed":
        title = t`Couldn’t insert your text`;
        break;
      case "copyFailed":
        title = t`Couldn’t copy your text`;
        detail = t`Try copying again.`;
        break;
    }
    if (status.canRetry) actions.push({ id: "retry", label: t`Retry` });
    if (
      status.canCopy &&
      (status.phase === "error" || status.phase === "ready")
    )
      actions.push({ id: "copy", label: t`Copy text` });
    if (
      !isDictationActive(status) &&
      (status.phase === "error" || status.phase === "ready") &&
      status.problem !== "noSpeech"
    ) {
      actions.push({
        id: "discard",
        label: status.canRetry ? t`Discard` : t`Dismiss`,
      });
    }
    const state = {
      amplitude: status.amplitude,
      title,
      status:
        status.phase === "recording"
          ? ("recording" as const)
          : status.phase === "error" && status.problem !== "noSpeech"
            ? ("error" as const)
            : ("processing" as const),
      showStatusText: true,
      dictation: {
        attemptId: status.attemptId,
        phase: status.phase,
        actions,
        detail,
      },
      colorScheme: document.documentElement.classList.contains("dark")
        ? ("dark" as const)
        : ("light" as const),
      opacity: floating_bar_opacity,
      liveCaptionOpacity: 0.3,
      liveCaptionWidth: 440,
      liveCaptionLineCount: 1,
      liveCaptionPosition: "topCenter" as const,
      liveCaptionMinimized: true,
      liveCaptionToggleVisible: false,
      transcriptBubbles: [],
    };
    renderQueue.current = renderQueue.current
      .then(async () => {
        if (
          meetingOwnsBar() ||
          client.getQueryData<DictationStatus>(dictationQueryKey)?.revision !==
            currentStatus.revision
        )
          return;
        const updated = await windowsCommands.floatingBarUpdate(state);
        if (updated.status === "error") throw new Error(String(updated.error));
        if (!shown.current && !meetingOwnsBar()) {
          const result = await windowsCommands.floatingBarShow();
          if (result.status === "error") throw new Error(String(result.error));
          shown.current = true;
        }
      })
      .catch(console.error);
    if (
      status.phase === "done" ||
      status.phase === "copied" ||
      status.phase === "cancelled" ||
      status.problem === "noSpeech"
    ) {
      timer = setTimeout(hide, status.problem === "noSpeech" ? 2500 : 700);
    }
    return () => {
      if (timer) clearTimeout(timer);
    };
  }, [status, floating_bar_opacity, client, t]);

  return null;
}
