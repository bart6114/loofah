import { cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const themeState = vi.hoisted(() => ({
  settingsReady: false,
  theme: "system" as "light" | "dark" | "system",
  designTheme: "default",
}));

const applyDocumentTheme = vi.hoisted(() =>
  vi.fn((theme: string, prefersDark?: boolean) =>
    theme === "dark" ? true : theme === "system" && prefersDark === true,
  ),
);
const writeStoredThemePreference = vi.hoisted(() => vi.fn());
const setDockIcon = vi.hoisted(() =>
  vi.fn(async () => ({ status: "ok", data: null })),
);
const nativeTheme = vi.hoisted(() => vi.fn(async () => "light"));
const onThemeChanged = vi.hoisted(() => vi.fn(async () => vi.fn()));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    theme: nativeTheme,
    onThemeChanged,
  }),
}));

vi.mock("@hypr/plugin-icon", () => ({
  commands: { setDockIcon },
}));

vi.mock("./apply", () => ({
  applyDocumentTheme,
  writeStoredThemePreference,
}));

vi.mock("./use-settings-theme-ready", () => ({
  useSettingsThemeReady: () => themeState.settingsReady,
}));

vi.mock("~/shared/config", () => ({
  useConfigValue: (key: string) =>
    key === "design_theme" ? themeState.designTheme : themeState.theme,
}));

import { AppThemeProvider, applyThemePreference } from "./provider";

describe("AppThemeProvider", () => {
  beforeEach(() => {
    cleanup();
    document.documentElement.removeAttribute("data-design-theme");
    themeState.settingsReady = false;
    themeState.theme = "system";
    themeState.designTheme = "default";
    applyDocumentTheme.mockClear();
    writeStoredThemePreference.mockClear();
    setDockIcon.mockClear();
    nativeTheme.mockReset();
    nativeTheme.mockResolvedValue("light");
    onThemeChanged.mockReset();
    onThemeChanged.mockResolvedValue(vi.fn());
    window.matchMedia = vi.fn().mockReturnValue({
      matches: false,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    });
  });

  afterEach(() => {
    cleanup();
  });

  it("does not clobber the boot theme before settings hydrate", () => {
    render(
      <AppThemeProvider>
        <div>child</div>
      </AppThemeProvider>,
    );

    expect(applyDocumentTheme).not.toHaveBeenCalled();
    expect(writeStoredThemePreference).not.toHaveBeenCalled();
    expect(setDockIcon).not.toHaveBeenCalled();
  });

  it("applies the hydrated settings theme once the store is ready", async () => {
    themeState.settingsReady = true;
    themeState.theme = "light";

    render(
      <AppThemeProvider>
        <div>child</div>
      </AppThemeProvider>,
    );

    expect(applyDocumentTheme).toHaveBeenCalledWith("light");
    expect(writeStoredThemePreference).toHaveBeenCalledWith("light");
    await waitFor(() => expect(setDockIcon).toHaveBeenCalledWith("stable"));
  });

  it("uses the native window appearance for the system theme", async () => {
    themeState.settingsReady = true;
    themeState.theme = "system";
    nativeTheme.mockResolvedValue("dark");

    render(
      <AppThemeProvider>
        <div>child</div>
      </AppThemeProvider>,
    );

    await waitFor(() =>
      expect(applyDocumentTheme).toHaveBeenCalledWith("system", true),
    );
    expect(writeStoredThemePreference).toHaveBeenCalledWith("system");
    await waitFor(() =>
      expect(setDockIcon).toHaveBeenCalledWith("stable-dark"),
    );
  });

  it("ignores a stale native theme after leaving system appearance", async () => {
    let resolveNativeTheme!: (theme: string) => void;
    nativeTheme.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveNativeTheme = resolve;
        }),
    );
    themeState.settingsReady = true;
    themeState.theme = "system";

    const { rerender } = render(
      <AppThemeProvider>
        <div>child</div>
      </AppThemeProvider>,
    );

    await waitFor(() => expect(nativeTheme).toHaveBeenCalledOnce());

    themeState.theme = "light";
    rerender(
      <AppThemeProvider>
        <div>child</div>
      </AppThemeProvider>,
    );
    resolveNativeTheme("dark");
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(applyDocumentTheme).toHaveBeenCalledTimes(1);
    expect(applyDocumentTheme).toHaveBeenCalledWith("light");
    expect(setDockIcon).toHaveBeenCalledWith("stable");
  });

  it("applies a selected system theme and Dock icon from the native source", async () => {
    nativeTheme.mockResolvedValue("dark");

    await applyThemePreference("system");

    expect(nativeTheme).toHaveBeenCalledOnce();
    expect(applyDocumentTheme).toHaveBeenCalledWith("system", true);
    expect(writeStoredThemePreference).toHaveBeenCalledWith("system");
    expect(setDockIcon).toHaveBeenCalledWith("stable-dark");
  });

  it("applies an explicit selection and matching Dock icon immediately", async () => {
    await applyThemePreference("light");

    expect(nativeTheme).not.toHaveBeenCalled();
    expect(applyDocumentTheme).toHaveBeenCalledWith("light");
    expect(writeStoredThemePreference).toHaveBeenCalledWith("light");
    expect(setDockIcon).toHaveBeenCalledWith("stable");
  });

  it("updates the design on config changes without changing appearance", () => {
    themeState.settingsReady = true;
    themeState.theme = "dark";
    const { rerender } = render(
      <AppThemeProvider>
        <div>child</div>
      </AppThemeProvider>,
    );
    themeState.designTheme = "workshop";
    rerender(
      <AppThemeProvider>
        <div>child</div>
      </AppThemeProvider>,
    );
    expect(document.documentElement.dataset.designTheme).toBe("workshop");
    expect(applyDocumentTheme).toHaveBeenLastCalledWith("dark");
  });

  it("does not let a pending system selection overwrite a newer explicit selection", async () => {
    let resolveNative!: (theme: string) => void;
    nativeTheme.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveNative = resolve;
        }),
    );
    const pending = applyThemePreference("system");
    await applyThemePreference("light");
    resolveNative("dark");
    await pending;
    expect(applyDocumentTheme).toHaveBeenCalledTimes(1);
    expect(applyDocumentTheme).toHaveBeenCalledWith("light");
  });
});
