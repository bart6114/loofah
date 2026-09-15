import { act, renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { AppConfig } from "@hypr/plugin-settings";

import { useConfigValue, useConfigValues } from ".";
import { applyConfigSnapshot } from "./store";

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));
vi.mock("@hypr/plugin-settings", () => ({
  commands: {
    getConfig: vi.fn(async () => ({ status: "ok", data: { theme: "light" } })),
  },
}));

describe("setting subscriptions", () => {
  it("ignores unrelated and identical snapshots and observes selected changes", async () => {
    const renders = vi.fn();
    const hook = renderHook(() => {
      const theme = useConfigValue("theme");
      const selected = useConfigValues(["theme", "spoken_languages"]);
      renders();
      return { theme, selected };
    });
    await waitFor(() => expect(hook.result.current.theme).toBe("light"));
    const before = renders.mock.calls.length;
    const selected = hook.result.current.selected;
    act(() =>
      applyConfigSnapshot({
        theme: "light",
        timezone: "Europe/Brussels",
      } as AppConfig),
    );
    expect(renders).toHaveBeenCalledTimes(before);
    expect(hook.result.current.selected).toBe(selected);
    act(() =>
      applyConfigSnapshot({
        timezone: "Europe/Brussels",
        theme: "light",
      } as AppConfig),
    );
    expect(renders).toHaveBeenCalledTimes(before);
    act(() =>
      applyConfigSnapshot({
        theme: "dark",
        spoken_languages: ["en"],
      } as AppConfig),
    );
    expect(hook.result.current.theme).toBe("dark");
    expect(hook.result.current.selected.spoken_languages).toEqual(["en"]);
    hook.unmount();
  });
});
