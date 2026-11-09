import { waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

import { ScopedTabsController } from "./scoped-controller";
import { deferred, deviceFixture, scopedFixture } from "./scoped-test-utils";

const controllers: ScopedTabsController[] = [];
function fixture(id = "local:fixture", managePins = true) {
  const scope = scopedFixture(id);
  const device = deviceFixture();
  const error = vi.fn();
  const controller = new ScopedTabsController(
    scope.scope,
    device.device,
    managePins,
    error,
  );
  controllers.push(controller);
  return { ...scope, ...device, controller, error, store: controller.store };
}

afterEach(() => {
  for (const controller of controllers.splice(0)) controller.dispose();
});

it("isolates identical IDs, history and closed tabs in immutable scope stores", async () => {
  const first = fixture("remote:first"),
    second = fixture("remote:second");
  await Promise.all([first.controller.start(), second.controller.start()]);
  first.store.getState().openCurrent({ type: "sessions", id: "same-id" });
  first.store.getState().openCurrent({ type: "sessions", id: "private-first" });
  first.store.getState().close(first.store.getState().currentTab!);
  second.store.getState().openCurrent({ type: "sessions", id: "same-id" });
  expect(first.store.getState().closedTabs).toHaveLength(1);
  expect(second.store.getState().closedTabs).toHaveLength(0);
  expect(second.store.getState().recentlyOpenedSessionIds).toEqual(["same-id"]);
  expect(
    [...second.store.getState().history.values()].flatMap((history) =>
      history.stack.map((tab) => ("id" in tab ? tab.id : tab.type)),
    ),
  ).not.toContain("private-first");
  expect(first.store.scope).toBe("remote:first");
  expect(second.store.scope).toBe("remote:second");
});

it("preserves a selection made while an older bootstrap reply is pending", async () => {
  const { controller, store, device } = fixture();
  const read = deferred<string[]>();
  device.recentSessions.mockReturnValueOnce(read.promise);
  const starting = controller.start();
  await waitFor(() => expect(device.recentSessions).toHaveBeenCalledOnce());
  store.getState().openCurrent({ type: "sessions", id: "new-selection" });
  read.resolve(["old-recent"]);
  await starting;
  expect(store.getState().recentlyOpenedSessionIds[0]).toBe("new-selection");
  await waitFor(() =>
    expect(device.rememberSession).toHaveBeenCalledWith(
      "local:fixture",
      "new-selection",
    ),
  );
});

it("serializes remembers and never replaces newer optimistic recents with an older reply", async () => {
  const { controller, store, device } = fixture();
  await controller.start();
  const first = deferred<string[]>(),
    second = deferred<string[]>();
  device.rememberSession
    .mockReturnValueOnce(first.promise)
    .mockReturnValueOnce(second.promise);
  store.getState().openCurrent({ type: "sessions", id: "first" });
  await waitFor(() => expect(device.rememberSession).toHaveBeenCalledTimes(1));
  store.getState().openCurrent({ type: "sessions", id: "second" });
  expect(device.rememberSession).toHaveBeenCalledTimes(1);
  first.resolve(["first"]);
  await waitFor(() => expect(device.rememberSession).toHaveBeenCalledTimes(2));
  expect(store.getState().recentlyOpenedSessionIds).toEqual([
    "second",
    "first",
  ]);
  second.resolve(["second", "first"]);
  await waitFor(() =>
    expect(store.getState().recentlyOpenedSessionIds).toEqual([
      "second",
      "first",
    ]),
  );
});

it("drops late reads and captured actions after disposal without closing/deleting notes", async () => {
  const { controller, store, device, stop, listeners, summaries } = fixture();
  const read = deferred<string[]>();
  device.recentSessions.mockReturnValueOnce(read.promise);
  device.pinnedTabs.mockResolvedValueOnce(
    JSON.stringify([{ type: "sessions", id: "private", pinned: true }]),
  );
  const starting = controller.start();
  await waitFor(() => expect(device.recentSessions).toHaveBeenCalledOnce());
  const old = store.getState();
  const onClose = vi.fn();
  old.registerOnClose(onClose);
  old.openCurrent({ type: "sessions", id: "private" });
  controller.dispose();
  old.openNew({ type: "sessions", id: "late" });
  old.restoreLastClosedTab();
  store.setState({ recentlyOpenedSessionIds: ["late"] });
  read.resolve(["private"]);
  await starting;
  expect(store.getState()).toMatchObject({
    tabs: [],
    currentTab: null,
    recentlyOpenedSessionIds: [],
    closedTabs: [],
  });
  expect(store.getState().history.size).toBe(0);
  expect(onClose).not.toHaveBeenCalled();
  expect(summaries).not.toHaveBeenCalled();
  expect(device.rememberSession).not.toHaveBeenCalled();
  expect(stop).toHaveBeenCalledOnce();
  expect(listeners.size).toBe(0);
});

it("stops a device subscription that resolves after teardown", async () => {
  const { controller, device } = fixture();
  const subscription = deferred<() => void>(),
    stop = vi.fn();
  device.subscribe.mockReturnValueOnce(subscription.promise);
  const starting = controller.start();
  controller.dispose();
  subscription.resolve(stop);
  await starting;
  expect(stop).toHaveBeenCalledOnce();
  expect(device.recentSessions).not.toHaveBeenCalled();
});

it("clears private state and aborts pending canonical lookups on access loss", async () => {
  const { controller, store, pins, state, summaries } =
    fixture("remote:private");
  pins.set(
    "remote:private",
    JSON.stringify([{ type: "sessions", id: "private", pinned: true }]),
  );
  const lookup = deferred<never[]>();
  summaries.mockReturnValueOnce(lookup.promise);
  const starting = controller.start();
  await waitFor(() => expect(summaries).toHaveBeenCalledOnce());
  const options = summaries.mock.calls[0][1] as { signal: AbortSignal };
  store.getState().openCurrent({ type: "sessions", id: "private" });
  state("access_denied");
  lookup.resolve([]);
  await starting;
  expect(options.signal.aborted).toBe(true);
  expect(store.getState().tabs).toEqual([]);
  expect(store.getState().recentlyOpenedSessionIds).toEqual([]);
});

it("restores bounded canonical pins, omits missing notes and preserves current selection", async () => {
  const { controller, store, pins, recents, summaries } = fixture();
  pins.set(
    "local:fixture",
    JSON.stringify([
      { type: "sessions", id: "missing", pinned: true },
      ...Array.from({ length: 140 }, (_, index) => ({
        type: "sessions",
        id: `pin-${index}`,
        pinned: true,
      })),
    ]),
  );
  recents.set("local:fixture", ["recent"]);
  store.getState().openCurrent({ type: "settings", state: { tab: "app" } });
  await controller.start();
  expect(
    summaries.mock.calls.map(([input]) =>
      input.kind === "ids" ? input.ids.length : 0,
    ),
  ).toEqual([100, 28]);
  expect(store.getState().tabs.filter((tab) => tab.pinned)).toHaveLength(127);
  expect(
    store.getState().tabs.some((tab) => "id" in tab && tab.id === "missing"),
  ).toBe(false);
  expect(store.getState().currentTab?.type).toBe("settings");
  expect(store.getState().recentlyOpenedSessionIds).toEqual(["recent"]);
});

it("writes pins only for the injected main-window owner and keeps other windows independent", async () => {
  const main = fixture("local:main"),
    note = fixture("local:note", false);
  await Promise.all([main.controller.start(), note.controller.start()]);
  for (const { store } of [main, note]) {
    store.getState().openCurrent({ type: "settings", state: { tab: "app" } });
    store.getState().pin(store.getState().currentTab!);
  }
  await waitFor(() =>
    expect(main.device.setPinnedTabs).toHaveBeenCalledWith(
      "local:main",
      expect.any(String),
    ),
  );
  expect(note.device.pinnedTabs).not.toHaveBeenCalled();
  expect(note.device.setPinnedTabs).not.toHaveBeenCalled();
});

it("reloads only matching device state and continues after a failed read", async () => {
  const { controller, store, device, recents, emit, error } = fixture();
  await controller.start();
  device.recentSessions.mockRejectedValueOnce(
    new Error("temporary device read"),
  );
  emit("local:fixture");
  await waitFor(() => expect(error).toHaveBeenCalledOnce());
  recents.set("local:fixture", ["changed"]);
  emit("remote:other");
  expect(device.recentSessions).toHaveBeenCalledTimes(2);
  emit("local:fixture");
  await waitFor(() =>
    expect(store.getState().recentlyOpenedSessionIds).toEqual(["changed"]),
  );
});

it("keeps current pin changes when an older restore is awaiting canonical metadata", async () => {
  const { controller, store, pins, summaries, device } = fixture();
  pins.set(
    "local:fixture",
    JSON.stringify([{ type: "sessions", id: "old-pin", pinned: true }]),
  );
  const lookup = deferred<never[]>();
  summaries.mockReturnValueOnce(lookup.promise);
  const starting = controller.start();
  await waitFor(() => expect(summaries).toHaveBeenCalledOnce());
  store.getState().openCurrent({ type: "settings", state: { tab: "app" } });
  store.getState().pin(store.getState().currentTab!);
  lookup.resolve([]);
  await starting;
  expect(store.getState().tabs).toMatchObject([
    { type: "settings", pinned: true },
  ]);
  await waitFor(() =>
    expect(device.setPinnedTabs).toHaveBeenCalledWith(
      "local:fixture",
      expect.stringContaining("settings"),
    ),
  );
});

it("persists device recents while a remote canonical lookup remains pending", async () => {
  const { controller, store, pins, summaries, device } = fixture("remote:slow");
  pins.set(
    "remote:slow",
    JSON.stringify([{ type: "sessions", id: "old-pin", pinned: true }]),
  );
  const lookup = deferred<never[]>();
  summaries.mockReturnValueOnce(lookup.promise);
  const starting = controller.start();
  await waitFor(() => expect(summaries).toHaveBeenCalledOnce());
  store.getState().openCurrent({ type: "sessions", id: "new-selection" });
  await waitFor(() =>
    expect(device.rememberSession).toHaveBeenCalledWith(
      "remote:slow",
      "new-selection",
    ),
  );
  expect(store.getState().recentlyOpenedSessionIds).toEqual(["new-selection"]);
  lookup.resolve([]);
  await starting;
  expect(store.getState().currentTab).toMatchObject({ id: "new-selection" });
});

it("cancels obsolete canonical reads and ignores their late results after a newer device event", async () => {
  const { controller, store, pins, summaries, emit } =
    fixture("remote:changed");
  pins.set(
    "remote:changed",
    JSON.stringify([{ type: "sessions", id: "old-pin", pinned: true }]),
  );
  const lookup =
    deferred<{ id: string; title: string; created_at: string }[]>();
  summaries.mockReturnValueOnce(lookup.promise);
  const starting = controller.start();
  await waitFor(() => expect(summaries).toHaveBeenCalledOnce());
  const signal = summaries.mock.calls[0][1]!.signal!;
  pins.set(
    "remote:changed",
    JSON.stringify([{ type: "sessions", id: "new-pin", pinned: true }]),
  );
  emit("remote:changed");
  await waitFor(() =>
    expect(store.getState().currentTab).toMatchObject({ id: "new-pin" }),
  );
  expect(signal.aborted).toBe(true);
  lookup.resolve([
    { id: "old-pin", title: "Private old title", created_at: "2026-01-01" },
  ]);
  await starting;
  expect(store.getState().tabs).toMatchObject([{ id: "new-pin" }]);
});

it("coalesces queued pin writes and preserves the existing membership-only persistence policy", async () => {
  const { controller, store, device } = fixture();
  await controller.start();
  const firstWrite = deferred<void>();
  device.setPinnedTabs.mockReturnValueOnce(firstWrite.promise);
  store.getState().openCurrent({ type: "sessions", id: "first" });
  store.getState().pin(store.getState().currentTab!);
  await waitFor(() => expect(device.setPinnedTabs).toHaveBeenCalledTimes(1));
  const first = store.getState().currentTab!;
  if (first.type !== "sessions") throw new Error("Expected a session tab");
  store
    .getState()
    .updateSessionTabState(first, { ...first.state, view: { type: "raw" } });
  for (const id of ["second", "third"]) {
    store.getState().openNew({ type: "sessions", id });
    store.getState().pin(store.getState().currentTab!);
  }
  expect(device.setPinnedTabs).toHaveBeenCalledTimes(1);
  firstWrite.resolve();
  await waitFor(() => expect(device.setPinnedTabs).toHaveBeenCalledTimes(2));
  expect(JSON.parse(device.setPinnedTabs.mock.calls[1][1])).toMatchObject([
    { id: "first" },
    { id: "second" },
    { id: "third" },
  ]);
});

it("can retry failed initialization without duplicating scope or device subscriptions", async () => {
  const { controller, device, listeners, store, recents } = fixture();
  device.recentSessions.mockRejectedValueOnce(
    new Error("temporary device failure"),
  );
  await expect(controller.start()).rejects.toThrow("temporary device failure");
  recents.set("local:fixture", ["retry"]);
  await controller.start();
  expect(store.getState().recentlyOpenedSessionIds).toEqual(["retry"]);
  expect(device.subscribe).toHaveBeenCalledOnce();
  expect(listeners.size).toBe(1);
});

it("rechecks canonical pins after reconnecting without choosing a local vault", async () => {
  const { controller, pins, summaries, state, store, device } =
    fixture("remote:reconnect");
  pins.set(
    "remote:reconnect",
    JSON.stringify([{ type: "sessions", id: "remote-pin", pinned: true }]),
  );
  summaries.mockRejectedValueOnce(new Error("disconnected"));
  await expect(controller.start()).rejects.toThrow("disconnected");
  state("reconnecting");
  state("connected");
  await waitFor(() =>
    expect(store.getState().currentTab).toMatchObject({ id: "remote-pin" }),
  );
  expect(
    device.recentSessions.mock.calls.every(
      ([scope]) => scope === "remote:reconnect",
    ),
  ).toBe(true);
  expect(device.subscribe).toHaveBeenCalledOnce();
});

it.each(["local:queued-recents", "remote:queued-recents"])(
  "does not admit a read queued before a newer recent mutation in %s",
  async (id) => {
    const { controller, store, device, emit, recents } = fixture(id);
    await controller.start();
    const first = deferred<string[]>(),
      second = deferred<string[]>();
    device.rememberSession
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);
    store.getState().openCurrent({ type: "sessions", id: "first" });
    await waitFor(() => expect(device.rememberSession).toHaveBeenCalledOnce());
    recents.set(id, ["first"]);
    emit(id);
    store.getState().openCurrent({ type: "sessions", id: "second" });
    const observed: string[][] = [];
    const stop = store.subscribe((next, previous) => {
      if (next.recentlyOpenedSessionIds !== previous.recentlyOpenedSessionIds)
        observed.push(next.recentlyOpenedSessionIds);
    });
    first.resolve(["first"]);
    await waitFor(() =>
      expect(device.rememberSession).toHaveBeenCalledTimes(2),
    );
    expect(store.getState().recentlyOpenedSessionIds).toEqual([
      "second",
      "first",
    ]);
    expect(observed.every((ids) => ids[0] === "second")).toBe(true);
    second.resolve(["second", "first"]);
    stop();
  },
);

it.each(["local:queued-pins", "remote:queued-pins"])(
  "does not resurrect pins from an event read queued before a newer unpin in %s",
  async (id) => {
    const { controller, store, device, emit, pins } = fixture(id);
    await controller.start();
    const first = deferred<void>(),
      second = deferred<void>();
    device.setPinnedTabs
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);
    store.getState().openCurrent({ type: "settings", state: { tab: "app" } });
    store.getState().pin(store.getState().currentTab!);
    await waitFor(() => expect(device.setPinnedTabs).toHaveBeenCalledOnce());
    pins.set(id, device.setPinnedTabs.mock.calls[0][1]);
    emit(id);
    store.getState().unpin(store.getState().currentTab!);
    const observed: boolean[] = [];
    const stop = store.subscribe((next) => {
      observed.push(next.tabs.some((tab) => tab.pinned));
    });
    first.resolve();
    await waitFor(() => expect(device.setPinnedTabs).toHaveBeenCalledTimes(2));
    expect(store.getState().tabs[0].pinned).toBe(false);
    expect(observed).not.toContain(true);
    expect(JSON.parse(device.setPinnedTabs.mock.calls[1][1])).toEqual([]);
    pins.set(id, "[]");
    second.resolve();
    stop();
  },
);

it.each(["local:bootstrap-pins", "remote:bootstrap-pins"])(
  "merges early pin additions/removals with the unread device baseline in %s",
  async (id) => {
    const { controller, store, device } = fixture(id);
    const baseline = deferred<string | null>();
    device.pinnedTabs.mockReturnValueOnce(baseline.promise);
    const starting = controller.start();
    await waitFor(() => expect(device.pinnedTabs).toHaveBeenCalledOnce());
    store.getState().openCurrent({ type: "settings", state: { tab: "app" } });
    store.getState().pin(store.getState().currentTab!);
    store.getState().unpin(store.getState().currentTab!);
    store.getState().openNew({ type: "sessions", id: "new-pin" });
    store.getState().pin(store.getState().currentTab!);
    expect(device.setPinnedTabs).not.toHaveBeenCalled();
    baseline.resolve(
      JSON.stringify([
        { type: "settings", state: { tab: "app" }, pinned: true },
        { type: "sessions", id: "old-pin", pinned: true },
      ]),
    );
    await starting;
    await waitFor(() => expect(device.setPinnedTabs).toHaveBeenCalledOnce());
    expect(JSON.parse(device.setPinnedTabs.mock.calls[0][1])).toMatchObject([
      { id: "old-pin" },
      { id: "new-pin" },
    ]);
    expect(
      store
        .getState()
        .tabs.filter((tab) => tab.pinned)
        .map((tab) => ("id" in tab ? tab.id : tab.type))
        .sort(),
    ).toEqual(["new-pin", "old-pin"]);
    expect(store.getState().currentTab).toMatchObject({ id: "new-pin" });
  },
);
