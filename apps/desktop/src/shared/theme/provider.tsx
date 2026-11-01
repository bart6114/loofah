import { getCurrentWindow, type Theme } from "@tauri-apps/api/window";
import type { ReactNode } from "react";

import { commands as iconCommands } from "@hypr/plugin-icon";

import { applyDocumentTheme, writeStoredThemePreference } from "./apply";
import { getDesignTheme } from "./catalog";
import { applyDocumentDesignTheme, writeStoredDesignTheme } from "./design";
import type { ThemePreference } from "./resolve";
import { useSettingsThemeReady } from "./use-settings-theme-ready";

import { useConfigValue } from "~/shared/config";
import { useMountEffect } from "~/shared/hooks/useMountEffect";

let appearanceRevision = 0;

export function AppThemeProvider({ children }: { children: ReactNode }) {
  const theme = useConfigValue("theme") as ThemePreference;
  const designTheme = getDesignTheme(useConfigValue("design_theme")).id;
  const settingsReady = useSettingsThemeReady();

  return (
    <>
      {settingsReady ? (
        <ThemeSync
          key={`${theme}:${designTheme}`}
          theme={theme}
          designTheme={designTheme}
        />
      ) : null}
      {children}
    </>
  );
}

function ThemeSync({
  theme,
  designTheme,
}: {
  theme: ThemePreference;
  designTheme: string;
}) {
  useMountEffect(() => {
    applyDesignThemePreference(designTheme);
    if (
      theme !== "system" ||
      getDesignTheme(designTheme).appearance !== "adaptive"
    ) {
      applyAppTheme(theme);
      return;
    }

    const appWindow = getCurrentWindow();
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    const applySystemTheme = (systemTheme: Theme | null) => {
      if (cancelled) {
        return;
      }

      applyAppTheme(theme, systemTheme === "dark");
    };

    void (async () => {
      unlisten = await appWindow.onThemeChanged(({ payload }) => {
        applySystemTheme(payload);
      });

      if (cancelled) {
        unlisten();
        return;
      }

      applySystemTheme(await appWindow.theme());
    })().catch((error) => {
      if (!cancelled) {
        console.error("[theme] failed to read system appearance", error);
        applyAppTheme(theme);
      }
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  });

  return null;
}

export function applyDesignThemePreference(id: string) {
  applyDocumentDesignTheme(id);
  writeStoredDesignTheme(id);
}

export async function applyThemePreference(theme: ThemePreference) {
  const revision = ++appearanceRevision;
  if (
    theme !== "system" ||
    getDesignTheme(document.documentElement.dataset.designTheme).appearance !==
      "adaptive"
  ) {
    applyAppTheme(theme);
    return;
  }

  try {
    const systemTheme = await getCurrentWindow().theme();
    if (revision !== appearanceRevision) return;
    applyAppTheme(theme, systemTheme === "dark");
  } catch (error) {
    if (revision !== appearanceRevision) return;
    console.error("[theme] failed to read system appearance", error);
    applyAppTheme(theme);
  }
}

function applyAppTheme(theme: ThemePreference, prefersDark?: boolean) {
  appearanceRevision += 1;
  const isDark =
    prefersDark === undefined
      ? applyDocumentTheme(theme)
      : applyDocumentTheme(theme, prefersDark);
  writeStoredThemePreference(theme);

  void iconCommands
    .setDockIcon(isDark ? "stable-dark" : "stable")
    .then((result) => {
      if (result.status === "error") {
        console.error("[theme] failed to update Dock icon", result.error);
      }
    })
    .catch((error) => {
      console.error("[theme] failed to update Dock icon", error);
    });
}
