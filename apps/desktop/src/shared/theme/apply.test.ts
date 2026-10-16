import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const getStoredSettingValues = vi.hoisted(() => vi.fn());

vi.mock("~/settings/queries", () => ({
  getStoredSettingValues,
}));

import {
  applyDocumentTheme,
  bootstrapThemeFromSettings,
  normalizeThemePreference,
  resolveBootIsDark,
} from "./apply";
import { applyDocumentDesignTheme } from "./design";

function mockSystemTheme(prefersDark: boolean) {
  window.matchMedia = vi.fn().mockReturnValue({
    matches: prefersDark,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  });
}

beforeEach(() => {
  const values = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    clear: () => values.clear(),
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
  });
  getStoredSettingValues.mockReset();
  localStorage.clear();
  document.documentElement.className = "";
  document.documentElement.removeAttribute("data-design-theme");
  document.documentElement.removeAttribute("style");
  mockSystemTheme(false);
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("normalizeThemePreference", () => {
  it("returns stored theme values", () => {
    expect(normalizeThemePreference("light")).toBe("light");
    expect(normalizeThemePreference("dark")).toBe("dark");
    expect(normalizeThemePreference("system")).toBe("system");
  });

  it("falls back to system for missing or invalid values", () => {
    expect(normalizeThemePreference(null)).toBe("system");
    expect(normalizeThemePreference("invalid")).toBe("system");
  });
});

describe("resolveBootIsDark", () => {
  it("honors explicit light and dark preferences", () => {
    expect(resolveBootIsDark("light", true)).toBe(false);
    expect(resolveBootIsDark("dark", false)).toBe(true);
  });

  it("follows system preference when stored theme is system or missing", () => {
    expect(resolveBootIsDark("system", true)).toBe(true);
    expect(resolveBootIsDark("system", false)).toBe(false);
    expect(resolveBootIsDark(null, true)).toBe(true);
    expect(resolveBootIsDark(null, false)).toBe(false);
  });

  it("treats invalid boot values like system to avoid theme flashes", () => {
    expect(resolveBootIsDark("legacy-value", true)).toBe(true);
    expect(resolveBootIsDark("legacy-value", false)).toBe(false);
  });
});

describe("bootstrapThemeFromSettings", () => {
  it("keeps fixed themes fixed and restores the Default appearance when returning", () => {
    applyDocumentDesignTheme("signal");
    expect(applyDocumentTheme("dark", true)).toBe(false);
    applyDocumentDesignTheme("midnight");
    expect(applyDocumentTheme("light", false)).toBe(true);
    applyDocumentDesignTheme("default");
    expect(applyDocumentTheme("system", true)).toBe(true);
    expect(applyDocumentTheme("system", false)).toBe(false);
  });
  it("hydrates the design from config rather than trusting an old boot cache", async () => {
    localStorage.setItem("loofah-design-theme", "forma");
    getStoredSettingValues.mockResolvedValue({
      values: { theme: "dark", design_theme: "signal" },
      hasValues: new Set(["theme", "design_theme"]),
    });
    await bootstrapThemeFromSettings({ timeoutMs: 100 });
    expect(document.documentElement.dataset.designTheme).toBe("signal");
    expect(document.documentElement.classList.contains("dark")).toBe(false);
    expect(localStorage.getItem("hypr-theme")).toBe("dark");
    expect(localStorage.getItem("loofah-design-theme")).toBe("signal");
  });
  it("applies persisted settings before resolving when load is prompt", async () => {
    getStoredSettingValues.mockResolvedValue({
      values: { theme: "dark" },
      hasValues: new Set(["theme"]),
    });

    await bootstrapThemeFromSettings({ timeoutMs: 100 });

    expect(document.documentElement.classList.contains("dark")).toBe(true);
    expect(localStorage.getItem("hypr-theme")).toBe("dark");
  });

  it("does not hold startup past the deadline when settings load stalls", async () => {
    vi.useFakeTimers();

    let resolveLoad!: (value: {
      values: { theme: string };
      hasValues: Set<string>;
    }) => void;
    getStoredSettingValues.mockReturnValue(
      new Promise((resolve) => {
        resolveLoad = resolve;
      }),
    );

    const bootstrap = bootstrapThemeFromSettings({ timeoutMs: 20 });
    let resolved = false;
    void bootstrap.then(() => {
      resolved = true;
    });

    await vi.advanceTimersByTimeAsync(20);

    expect(resolved).toBe(true);
    expect(localStorage.getItem("hypr-theme")).toBe(null);

    resolveLoad({
      values: { theme: "dark" },
      hasValues: new Set(["theme"]),
    });
    await Promise.resolve();

    expect(document.documentElement.classList.contains("dark")).toBe(true);
    expect(localStorage.getItem("hypr-theme")).toBe("dark");
  });
});
