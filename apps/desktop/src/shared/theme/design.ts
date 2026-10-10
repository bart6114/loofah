import { designThemeTokens, getDesignTheme } from "./catalog";

export const DESIGN_THEME_STORAGE_KEY = "loofah-design-theme";

export function readStoredDesignTheme(): string {
  try {
    return getDesignTheme(localStorage.getItem(DESIGN_THEME_STORAGE_KEY)).id;
  } catch {
    return getDesignTheme(null).id;
  }
}

export function applyDocumentDesignTheme(
  id: unknown,
  isDark = document.documentElement.classList.contains("dark"),
): void {
  const theme = getDesignTheme(id);
  isDark =
    theme.appearance === "adaptive" ? isDark : theme.appearance === "dark";
  const root = document.documentElement;
  root.classList.toggle("dark", isDark);
  root.dataset.designTheme = theme.id;
  root.style.colorScheme = isDark ? "dark" : "light";
  for (const [key, value] of Object.entries(designThemeTokens(theme, isDark))) {
    root.style.setProperty(key, value);
  }
}

export function writeStoredDesignTheme(id: string): void {
  try {
    localStorage.setItem(DESIGN_THEME_STORAGE_KEY, getDesignTheme(id).id);
  } catch {
    // Boot caching is optional; config.json remains authoritative.
  }
}
