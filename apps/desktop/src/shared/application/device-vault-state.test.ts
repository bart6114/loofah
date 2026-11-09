import { beforeEach, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  deviceRecentSessions: vi.fn(),
  deviceRememberSession: vi.fn(),
  devicePinnedTabs: vi.fn(),
  deviceSetPinnedTabs: vi.fn(),
  listen: vi.fn(),
}));

vi.mock("~/types/tauri.gen", () => ({
  commands: native,
  events: { deviceVaultStateChanged: { listen: native.listen } },
}));

import { deviceVaultState } from "./device-vault-state";

beforeEach(() => vi.resetAllMocks());

it("passes captured scope and primitive device preferences through typed native commands", async () => {
  native.deviceRecentSessions.mockResolvedValue({
    status: "ok",
    data: ["same"],
  });
  native.deviceRememberSession.mockResolvedValue({
    status: "ok",
    data: ["new", "same"],
  });
  native.devicePinnedTabs.mockResolvedValue({ status: "ok", data: "[]" });
  native.deviceSetPinnedTabs.mockResolvedValue({ status: "ok", data: null });
  expect(await deviceVaultState.recentSessions("scope")).toEqual(["same"]);
  expect(await deviceVaultState.rememberSession("scope", "new")).toEqual([
    "new",
    "same",
  ]);
  expect(await deviceVaultState.pinnedTabs("scope")).toBe("[]");
  await deviceVaultState.setPinnedTabs("scope", "[]");
  expect(native.deviceRecentSessions).toHaveBeenCalledWith("scope");
  expect(native.deviceRememberSession).toHaveBeenCalledWith("scope", "new");
  expect(native.devicePinnedTabs).toHaveBeenCalledWith("scope");
  expect(native.deviceSetPinnedTabs).toHaveBeenCalledWith("scope", "[]");
});

it("filters other-vault device notifications and preserves listener teardown", async () => {
  const stop = vi.fn();
  native.listen.mockResolvedValue(stop);
  const changed = vi.fn();
  expect(await deviceVaultState.subscribe("first", changed)).toBe(stop);
  const emit = native.listen.mock.calls[0][0];
  emit({ payload: { scope: "second" } });
  expect(changed).not.toHaveBeenCalled();
  emit({ payload: { scope: "first" } });
  expect(changed).toHaveBeenCalledOnce();
});

it("reports native failures without replacing them with empty state", async () => {
  native.deviceRecentSessions.mockResolvedValue({
    status: "error",
    error: {
      code: "unavailable",
      message: "Cannot read preferences",
      retryable: true,
    },
  });
  await expect(deviceVaultState.recentSessions("scope")).rejects.toMatchObject({
    detail: { code: "unavailable" },
  });
});
