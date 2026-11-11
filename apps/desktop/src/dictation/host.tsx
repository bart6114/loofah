import { getName } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef } from "react";

import { commands as permissionsCommands } from "@hypr/plugin-permissions";
import { commands as windowsCommands } from "@hypr/plugin-windows";
import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { useConfigValues } from "~/shared/config";
import type { DictationStatus } from "~/types/tauri.gen";

export function DictationHost() {
  const { dictation_shortcut, dictation_trigger_mode, floating_bar_opacity } =
    useConfigValues([
      "dictation_shortcut",
      "dictation_trigger_mode",
      "floating_bar_opacity",
    ]);
  const recording = useRef(false);
  const processing = useRef(false);
  const starting = useRef<Promise<void> | null>(null);

  useEffect(() => {
    const register = () => {
      void invoke("plugin:windows|dictation_shortcut_configure", {
        shortcut: dictation_shortcut,
      }).catch((error) => {
        sonnerToast.error(`Dictation shortcut unavailable: ${error}`);
      });
    };
    register();
    window.addEventListener("focus", register);
    return () => window.removeEventListener("focus", register);
  }, [dictation_shortcut]);

  useEffect(() => {
    const start = () => {
      if (recording.current || processing.current || starting.current) return;
      starting.current = (async () => {
        if (navigator.platform.includes("Mac")) {
          const accessibility =
            await permissionsCommands.checkPermission("accessibility");
          if (
            accessibility.status !== "ok" ||
            accessibility.data !== "authorized"
          ) {
            const appName = await getName();
            await permissionsCommands.requestPermission("accessibility");
            await permissionsCommands.openPermission("accessibility");
            sonnerToast.warning(
              `Allow ${appName} in Privacy & Security → Accessibility, then try dictation again.`,
              {
                id: "dictation-accessibility",
                duration: Infinity,
                action: {
                  label: "Open Settings",
                  onClick: () => {
                    void permissionsCommands.openPermission("accessibility");
                  },
                },
              },
            );
            return;
          }
          sonnerToast.dismiss("dictation-accessibility");
        }
        await invoke("dictation_start");
        recording.current = true;
      })()
        .catch((error) => {
          sonnerToast.error(`Could not start dictation: ${error}`);
        })
        .finally(() => {
          starting.current = null;
        });
    };
    const stop = async () => {
      await starting.current;
      if (!recording.current || processing.current) return;
      recording.current = false;
      processing.current = true;
      try {
        await invoke("dictation_stop");
      } catch (error) {
        sonnerToast.error(`Dictation failed: ${error}`);
        void windowsCommands.floatingBarHide();
      } finally {
        processing.current = false;
      }
    };
    const unlisteners = [
      listen("plugin:windows:dictation-shortcut-down", () => {
        if (dictation_trigger_mode === "toggle" && recording.current)
          void stop();
        else start();
      }),
      listen("plugin:windows:dictation-shortcut-up", () => {
        if (dictation_trigger_mode === "hold") void stop();
      }),
      listen("plugin:windows:floating-bar-stop", () => {
        if (recording.current) void stop();
      }),
    ];
    return () => {
      void Promise.all(unlisteners).then((fns) => fns.forEach((fn) => fn()));
    };
  }, [dictation_trigger_mode]);

  useEffect(() => {
    let hideTimer: ReturnType<typeof setTimeout> | null = null;
    let disposed = false;
    let shown = false;
    const listener = listen<DictationStatus>(
      "dictation-status",
      ({ payload }) => {
        if (disposed) return;
        if (hideTimer) clearTimeout(hideTimer);
        if (payload.phase === "done") {
          hideTimer = setTimeout(() => {
            shown = false;
            void windowsCommands.floatingBarHide();
          }, 800);
          return;
        }
        const state = {
          amplitude: payload.amplitude,
          title:
            payload.message ??
            (payload.phase === "recording"
              ? "Dictating"
              : payload.phase === "inserting"
                ? "Inserting"
                : "Transcribing"),
          status:
            payload.phase === "error"
              ? ("error" as const)
              : payload.phase === "recording"
                ? ("recording" as const)
                : ("processing" as const),
          colorScheme: matchMedia("(prefers-color-scheme: dark)").matches
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
        if (shown) void windowsCommands.floatingBarUpdate(state);
        else {
          shown = true;
          void windowsCommands
            .floatingBarShow()
            .then(() => windowsCommands.floatingBarUpdate(state));
        }
        if (payload.phase === "error") {
          hideTimer = setTimeout(() => {
            shown = false;
            void windowsCommands.floatingBarHide();
          }, 4000);
        }
      },
    );
    return () => {
      disposed = true;
      if (hideTimer) clearTimeout(hideTimer);
      void listener.then((unlisten) => unlisten());
    };
  }, [floating_bar_opacity]);

  return null;
}
