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
  setSettingValue: vi.fn(),
  enabled: undefined as boolean | undefined,
}));

vi.mock("~/settings/queries", () => ({
  setSettingValue: mocks.setSettingValue,
}));

vi.mock("~/shared/config/store", () => ({
  useConfigSelector: (selector: (stored: unknown) => unknown) =>
    selector({
      values: { auto_apply_high_confidence_tags: mocks.enabled },
      hasValues: new Set(
        mocks.enabled === undefined ? [] : ["auto_apply_high_confidence_tags"],
      ),
    }),
}));

import { TagSettings } from "./tags";

function renderSettings() {
  const client = new QueryClient({
    defaultOptions: { mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <TagSettings />
    </QueryClientProvider>,
  );
}

describe("summary tag settings", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.enabled = undefined;
    mocks.setSettingValue.mockResolvedValue(undefined);
  });
  afterEach(cleanup);

  it("defaults to on and saves an explicit opt-out", async () => {
    renderSettings();
    const toggle = screen.getByRole("switch", {
      name: "Auto-apply tags with high confidence",
    });
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    fireEvent.click(toggle);
    await waitFor(() =>
      expect(mocks.setSettingValue).toHaveBeenCalledWith(
        "auto_apply_high_confidence_tags",
        false,
      ),
    );
    expect(toggle.getAttribute("aria-checked")).toBe("false");
  });

  it("preserves an explicit false and can turn automatic application back on", async () => {
    mocks.enabled = false;
    renderSettings();
    const toggle = screen.getByRole("switch");
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(toggle);
    await waitFor(() =>
      expect(mocks.setSettingValue).toHaveBeenCalledWith(
        "auto_apply_high_confidence_tags",
        true,
      ),
    );
  });

  it("restores the saved state and reports a failed save", async () => {
    mocks.setSettingValue.mockRejectedValue(
      new Error("Could not save settings"),
    );
    renderSettings();
    fireEvent.click(screen.getByRole("switch"));
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toBe(
        "Could not save settings",
      ),
    );
    await waitFor(() =>
      expect(screen.getByRole("switch").getAttribute("aria-checked")).toBe(
        "true",
      ),
    );
  });
});
