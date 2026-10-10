import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  setSettingValues: vi.fn(async (_values: unknown) => {}),
  applyDesignThemePreference: vi.fn(),
  applyThemePreference: vi.fn(async (_theme: string) => {}),
  error: vi.fn(),
  designTheme: "signal",
}));

vi.mock("~/settings/queries", () => ({
  setSettingValues: mocks.setSettingValues,
}));
vi.mock("~/shared/config", () => ({
  useConfigValue: (key: string) =>
    key === "theme" ? "light" : mocks.designTheme,
}));
vi.mock("~/shared/theme/provider", () => ({
  applyDesignThemePreference: mocks.applyDesignThemePreference,
  applyThemePreference: mocks.applyThemePreference,
}));
vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: { error: mocks.error },
}));

import { ThemeSelector } from "./theme";

function renderPicker() {
  return render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { mutations: { retry: false } } })
      }
    >
      <ThemeSelector />
    </QueryClientProvider>,
  );
}

describe("theme picker", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.designTheme = "signal";
  });
  afterEach(cleanup);

  it("saves a fixed dark theme without overwriting Default appearance", async () => {
    renderPicker();
    fireEvent.click(screen.getByRole("button", { name: "Midnight" }));
    await waitFor(() =>
      expect(mocks.setSettingValues).toHaveBeenCalledWith({
        design_theme: "midnight",
      }),
    );
    expect(mocks.applyDesignThemePreference).toHaveBeenCalledWith("midnight");
    expect(mocks.applyThemePreference).toHaveBeenCalledWith("light");
  });

  it("keeps the chosen appearance when selecting one of the five directions", async () => {
    renderPicker();
    fireEvent.click(screen.getByRole("button", { name: "Workshop" }));
    await waitFor(() =>
      expect(mocks.setSettingValues).toHaveBeenCalledWith({
        design_theme: "workshop",
      }),
    );
    expect(mocks.applyThemePreference).toHaveBeenCalledWith("light");
  });

  it("only exposes Light, Dark, and System for Default", () => {
    const view = renderPicker();
    expect(screen.queryByRole("combobox", { name: "Appearance" })).toBeNull();
    mocks.designTheme = "default";
    view.rerender(
      <QueryClientProvider client={new QueryClient()}>
        <ThemeSelector />
      </QueryClientProvider>,
    );
    expect(screen.getByRole("combobox", { name: "Appearance" })).toBeTruthy();
  });

  it("restores the saved theme after a failed config write", async () => {
    mocks.setSettingValues.mockRejectedValueOnce(new Error("disk full"));
    renderPicker();
    fireEvent.click(screen.getByRole("button", { name: "Ember" }));
    await waitFor(() => expect(mocks.error).toHaveBeenCalledOnce());
    expect(mocks.applyDesignThemePreference).toHaveBeenLastCalledWith("signal");
    expect(mocks.applyThemePreference).toHaveBeenLastCalledWith("light");
    expect(
      screen
        .getByRole("button", { name: "Signal" })
        .getAttribute("aria-pressed"),
    ).toBe("true");
  });
});
