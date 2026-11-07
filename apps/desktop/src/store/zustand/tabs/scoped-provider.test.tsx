import { onlineManager } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, expect, it, vi } from "vitest";

import {
  ScopedTabsProvider,
  useScopedTabs,
  useScopedTabsStore,
} from "./scoped-provider";
import { deviceFixture, scopedFixture } from "./scoped-test-utils";
import type { ScopedTabsStore } from "./store";

afterEach(() => {
  cleanup();
  onlineManager.setOnline(true);
});

it("mounts independent captured stores and retires old callbacks across scope switches", async () => {
  const first = scopedFixture("remote:first"),
    second = scopedFixture("remote:second");
  const { device, recents } = deviceFixture();
  recents.set("remote:first", ["same-id", "private-first"]);
  recents.set("remote:second", ["same-id", "private-second"]);
  const stores = new Map<string, ScopedTabsStore>();
  function Probe() {
    const store = useScopedTabsStore();
    const ids = useScopedTabs((state) => state.recentlyOpenedSessionIds);
    stores.set(store.scope, store);
    return <output>{ids.join(",")}</output>;
  }
  const { rerender, unmount } = render(
    <ScopedTabsProvider scope={first.scope} deviceState={device} managePins>
      <Probe />
    </ScopedTabsProvider>,
  );
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe(
      "same-id,private-first",
    ),
  );
  const old = stores.get("remote:first")!;
  const oldAction = old.getState().openCurrent;
  rerender(
    <ScopedTabsProvider scope={second.scope} deviceState={device} managePins>
      <Probe />
    </ScopedTabsProvider>,
  );
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe(
      "same-id,private-second",
    ),
  );
  act(() => oldAction({ type: "sessions", id: "late" }));
  expect(old.getState().tabs).toEqual([]);
  expect(old.getState().recentlyOpenedSessionIds).toEqual([]);
  expect(device.rememberSession).not.toHaveBeenCalled();
  expect(stores.get("remote:second")).not.toBe(old);
  unmount();
  expect(
    stores.get("remote:second")!.getState().recentlyOpenedSessionIds,
  ).toEqual([]);
});

it("keeps login-free local device state available offline under StrictMode", async () => {
  onlineManager.setOnline(false);
  const { scope } = scopedFixture("local:offline");
  const { device, recents, stop } = deviceFixture();
  recents.set("local:offline", ["offline-note"]);
  function Probe() {
    const ids = useScopedTabs((state) => state.recentlyOpenedSessionIds);
    return <output>{ids.join(",")}</output>;
  }
  const { unmount } = render(
    <StrictMode>
      <ScopedTabsProvider scope={scope} deviceState={device} managePins>
        <Probe />
      </ScopedTabsProvider>
    </StrictMode>,
  );
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("offline-note"),
  );
  expect(device.recentSessions).toHaveBeenCalledWith("local:offline");
  unmount();
  expect(stop.mock.calls.length).toBe(device.subscribe.mock.calls.length);
});

it("requires an explicit provider instead of falling back to a global store", () => {
  const error = vi.spyOn(console, "error").mockImplementation(() => {});
  function Probe() {
    useScopedTabsStore();
    return null;
  }
  expect(() => render(<Probe />)).toThrow(
    "Tabs require an immutable vault host",
  );
  error.mockRestore();
});
