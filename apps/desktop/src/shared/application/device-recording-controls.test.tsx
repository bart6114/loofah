import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

import {
  DeviceRecordingControls,
  useDeviceStartActions,
  type DeviceStartActions,
} from "./device-recording-controls";

vi.mock("./device-recording-shell", () => ({
  DeviceRecordingShell: () => null,
}));
vi.mock("./device-processing-options", () => ({
  DeviceProcessingOptions: () => null,
  useDeviceProcessingOptions: () => ({ state: { values: {} } }),
  captureProcessingOptions: () => null,
}));
const caches: QueryClient[] = [];
afterEach(() => {
  cleanup();
  for (const cache of caches.splice(0)) cache.clear();
});
function setup(start: ReturnType<typeof vi.fn>) {
  const cache = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  caches.push(cache);
  cache.setQueryData(["device", "recordings", "profile"], []);
  cache.setQueryData(["device", "recordings", "next"], []);
  let actions: DeviceStartActions;
  function Probe() {
    actions = useDeviceStartActions()!;
    return null;
  }
  const owner = { scopeKey: "original", client: { start } } as any;
  const ui = (
    recording = owner,
    profileId = "profile",
    startAllowed = true,
  ) => (
    <QueryClientProvider client={cache}>
      <DeviceRecordingControls
        profileId={profileId}
        recording={recording}
        monitor={null}
        startAllowed={startAllowed}
      >
        <Probe />
      </DeviceRecordingControls>
    </QueryClientProvider>
  );
  const view = render(ui());
  return {
    cache,
    get actions() {
      return actions!;
    },
    allow: (allowed: boolean) => view.rerender(ui(owner, "profile", allowed)),
    replace: () =>
      view.rerender(ui({ scopeKey: "replacement", client: { start } }, "next")),
    unmount: view.unmount,
  };
}
it("coalesces starts and freezes both request identities through an unknown reply and explicit retry", async () => {
  let reject!: (cause: Error) => void;
  const start = vi.fn(
    (_request: unknown) =>
      new Promise((_, rejecter) => {
        reject = rejecter;
      }),
  );
  const view = setup(start);
  let first!: Promise<unknown>;
  let second!: Promise<unknown>;
  await act(async () => {
    first = view.actions.startPending().catch((error) => error);
    second = view.actions.startPending().catch((error) => error);
    await Promise.resolve();
  });
  expect(start).toHaveBeenCalledOnce();
  const original = structuredClone(start.mock.calls[0][0]);
  await act(async () => {
    reject(new Error("Unknown outcome"));
    await first;
    await second;
  });
  await expect(view.actions.startPending()).rejects.toThrow(
    "original recording request",
  );
  expect(start).toHaveBeenCalledOnce();
  start.mockResolvedValueOnce({
    identity: { capture_id: "original" },
    phase: "finished",
  });
  await act(async () => {
    await view.actions.startPending({ retry: true });
  });
  expect(start.mock.calls[1][0]).toEqual(original);
});
it("refuses a delayed reminder action after its original destination retires", async () => {
  const start = vi.fn();
  const view = setup(start);
  const original = view.actions.startPending;
  view.replace();
  await expect(original()).rejects.toThrow("destination is not ready");
  expect(start).not.toHaveBeenCalled();
  const current = view.actions.startPending;
  view.unmount();
  await expect(current()).rejects.toThrow("destination is not ready");
  expect(start).not.toHaveBeenCalled();
});
it("places a late successful receipt in its original profile cache", async () => {
  let complete!: (value: unknown) => void;
  const start = vi.fn(
    (_request: unknown) =>
      new Promise((resolve) => {
        complete = resolve;
      }),
  );
  const view = setup(start);
  let pending!: Promise<unknown>;
  await act(async () => {
    pending = view.actions.startPending();
    await Promise.resolve();
  });
  view.replace();
  const result = { identity: { capture_id: "original" }, phase: "finished" };
  await act(async () => {
    complete(result);
    await pending;
  });
  expect(view.cache.getQueryData(["device", "recordings", "profile"])).toEqual([
    result,
  ]);
  expect(view.cache.getQueryData(["device", "recordings", "next"])).toEqual([]);
});

it("rejects retained reminder callbacks after a connected reader downgrade without losing original retry identity", async () => {
  const start = vi.fn().mockRejectedValueOnce(new Error("Unknown outcome"));
  const view = setup(start);
  const retained = view.actions.startPending;
  await act(async () => {
    await view.actions.startPending().catch(() => {});
  });
  const original = structuredClone(start.mock.calls[0][0]);
  view.allow(false);
  expect(view.actions.canStart).toBe(false);
  await expect(retained({ retry: true })).rejects.toThrow("read-only");
  expect(start).toHaveBeenCalledTimes(1);
  view.allow(true);
  start.mockResolvedValueOnce({
    identity: { capture_id: "original" },
    phase: "finished",
  });
  await act(async () => {
    await view.actions.startPending({ retry: true });
  });
  expect(start.mock.calls[1][0]).toEqual(original);
});
