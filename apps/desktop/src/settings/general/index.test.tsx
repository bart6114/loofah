import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  useStoredSettingValuesQuery: vi.fn(),
}));

vi.mock("~/settings/queries", () => ({
  setSettingValue: vi.fn(),
  useSetSettingValues: vi.fn(),
  useStoredSettingValuesQuery: mocks.useStoredSettingValuesQuery,
}));

vi.mock("./app-settings", () => ({
  AppSettingsView: ({ autostart }: { autostart: { value: boolean } }) => (
    <span data-testid="autostart">{String(autostart.value)}</span>
  ),
  RecordingSettingsView: () => null,
}));
vi.mock("./main-language", () => ({
  MainLanguageView: ({ value }: { value: string }) => (
    <span data-testid="main-language">{value}</span>
  ),
}));
vi.mock("./notification", () => ({ NotificationSettingsView: () => null }));
vi.mock("./permissions", () => ({ Permissions: () => null }));
vi.mock("./spoken-languages", () => ({ SpokenLanguagesView: () => null }));
vi.mock("./storage", () => ({ StorageSettingsView: () => null }));
vi.mock("./theme", () => ({ ThemeSelector: () => null }));
vi.mock("./timezone", () => ({ TimezoneSelector: () => null }));

import { SettingsApp } from "./index";

describe("SettingsApp", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("waits for settings before constructing the form", () => {
    mocks.useStoredSettingValuesQuery.mockReturnValue({
      data: undefined,
      isLoading: true,
      error: null,
    });

    render(<SettingsApp />);

    expect(screen.getByLabelText("Loading settings")).toBeTruthy();
  });

  it("constructs the form from the hydrated config values", () => {
    mocks.useStoredSettingValuesQuery.mockReturnValue({
      data: {
        values: {
          autostart: true,
          spoken_languages: JSON.stringify(["en"]),
        },
        hasValues: new Set(["autostart", "spoken_languages"]),
      },
      isLoading: false,
      error: null,
    });

    render(<SettingsApp />);

    expect(screen.getByTestId("autostart").textContent).toBe("true");
  });
});
