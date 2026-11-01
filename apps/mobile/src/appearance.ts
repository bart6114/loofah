import { invoke } from "@tauri-apps/api/core";
import { useEffect } from "react";

import {
  getDesignTheme,
  resolveThemeAppearance,
} from "@hypr/design-theme/catalog";
import { applyDocumentDesignTheme } from "@hypr/design-theme/design";
import { resolveIsDarkMode } from "@hypr/design-theme/resolve";

import type { Snapshot } from "./api";

const cacheKey = "loofah-mobile-appearance";
export const defaultAppearance: Snapshot["appearance"] = {
  theme: "system",
  design_theme: "default",
};
let current = defaultAppearance;

export function normalizeAppearance(value?: {
  theme?: unknown;
  design_theme?: unknown;
}): Snapshot["appearance"] {
  return {
    theme:
      value?.theme === "light" || value?.theme === "dark"
        ? value.theme
        : "system",
    design_theme: getDesignTheme(value?.design_theme).id,
  };
}

export function applyAppearance(value: Snapshot["appearance"]) {
  current = normalizeAppearance(value);
  const theme = getDesignTheme(current.design_theme);
  const prefersDark =
    window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false;
  const dark = resolveIsDarkMode(
    resolveThemeAppearance(theme, current.theme),
    prefersDark,
  );
  applyDocumentDesignTheme(theme.id, dark);
  const palette =
    theme.appearance === "adaptive"
      ? dark
        ? theme.dark
        : theme.light
      : theme.palette;
  document
    .querySelector('meta[name="theme-color"]')
    ?.setAttribute("content", palette.paper);
  try {
    localStorage.setItem(cacheKey, JSON.stringify(current));
  } catch {
    // This only accelerates startup; the vault configuration is authoritative.
  }
}

export function restoreAppearance() {
  try {
    const stored = JSON.parse(localStorage.getItem(cacheKey) ?? "null");
    applyAppearance(
      stored && typeof stored === "object" ? stored : defaultAppearance,
    );
  } catch {
    applyAppearance(defaultAppearance);
  }
}

export function useAppearance(value: Snapshot["appearance"] | undefined) {
  const theme = value?.theme;
  const design = value?.design_theme;
  useEffect(() => {
    if (theme && design) applyAppearance({ theme, design_theme: design });
    const preference = resolveThemeAppearance(
      getDesignTheme(current.design_theme),
      current.theme,
    );
    void invoke("mobile_apply_appearance", { appearance: preference }).catch(
      console.error,
    );
  }, [theme, design]);
  useEffect(() => {
    const media = window.matchMedia?.("(prefers-color-scheme: dark)");
    const update = () => applyAppearance(current);
    media?.addEventListener("change", update);
    return () => media?.removeEventListener("change", update);
  }, []);
}
