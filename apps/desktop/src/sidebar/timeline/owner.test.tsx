import { onlineManager } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { StrictMode, useEffect } from "react";
import { afterEach, expect, it, vi } from "vitest";

import { TimelineOwnerProvider, useTimelineOwner } from "./owner";
import { useNearestTimelineBuckets, usePagedTimeline } from "./queries";
import { deferred, header, page, timelineFixture } from "./test-fixture";

import type { TimelinePage } from "~/shared/application/protocol.gen";
import { VaultHostContext } from "~/shared/application/vault-provider";
import type { VaultScope } from "~/shared/application/vault-scope";

const mocks = vi.hoisted(() => ({ tabs: {} }));
vi.mock("~/store/zustand/tabs", () => ({ useTabsStore: () => mocks.tabs }));
vi.mock("~/shared/config", () => ({
  useConfigValue: (key: string) =>
    key === "sidebar_expanded_tags" ? [] : key === "timezone" ? "UTC" : "date",
}));
const scopes: VaultScope[] = [];
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
  onlineManager.setOnline(true);
  vi.useRealTimers();
});
function View() {
  const { table, available } = usePagedTimeline();
  return (
    <div>
      {available
        ? Object.values(table)
            .map((row) => row.title)
            .join(",")
        : "hidden"}
    </div>
  );
}
function Host({
  scope,
  children = <View />,
}: {
  scope: VaultScope;
  children?: React.ReactNode;
}) {
  return (
    <VaultHostContext.Provider value={{ scope, error: null, retry: () => {} }}>
      <TimelineOwnerProvider>{children}</TimelineOwnerProvider>
    </VaultHostContext.Provider>
  );
}

it("preserves one captured owner under unrelated rerenders, remounts for same-string Scope or actual tabs replacement", async () => {
  const a = timelineFixture(),
    b = timelineFixture();
  scopes.push(a.scope, b.scope);
  await a.start();
  await b.start();
  const mounted = vi.fn(),
    unmounted = vi.fn();
  function Probe() {
    const owner = useTimelineOwner();
    useEffect(() => {
      mounted(owner);
      return () => unmounted(owner);
    }, [owner]);
    return <View />;
  }
  a.client.timelineHeaders.mockResolvedValue(page([header("local")]));
  b.client.timelineHeaders.mockResolvedValue(page([header("remote")]));
  const view = render(
    <StrictMode>
      <Host scope={a.scope}>
        <Probe />
      </Host>
    </StrictMode>,
  );
  await screen.findByText("local");
  const original = mounted.mock.lastCall?.[0];
  const count = mounted.mock.calls.length;
  view.rerender(
    <StrictMode>
      <Host scope={a.scope}>
        <Probe />
      </Host>
    </StrictMode>,
  );
  expect(mounted).toHaveBeenCalledTimes(count);
  view.rerender(
    <StrictMode>
      <Host scope={b.scope}>
        <Probe />
      </Host>
    </StrictMode>,
  );
  await screen.findByText("remote");
  expect(screen.queryByText("local")).toBeNull();
  expect(mounted.mock.lastCall?.[0]).not.toBe(original);
  const second = mounted.mock.lastCall?.[0];
  mocks.tabs = {};
  view.rerender(
    <StrictMode>
      <Host scope={b.scope}>
        <Probe />
      </Host>
    </StrictMode>,
  );
  await waitFor(() => expect(mounted.mock.lastCall?.[0]).not.toBe(second));
});
it.each([50, 150, 500])(
  "does not admit an old private result after a %ims same-ID replacement",
  async (latency) => {
    const a = timelineFixture(),
      b = timelineFixture();
    scopes.push(a.scope, b.scope);
    await a.start();
    await b.start();
    const pending = deferred<TimelinePage>();
    a.client.timelineHeaders.mockReturnValue(pending.promise);
    b.client.timelineHeaders.mockResolvedValue(
      page([header("same-id")], { revision: "other" }),
    );
    const view = render(<Host scope={a.scope} />);
    await waitFor(() => expect(a.client.timelineHeaders).toHaveBeenCalled());
    view.rerender(<Host scope={b.scope} />);
    await screen.findByText("same-id");
    await act(async () => {
      await new Promise((done) => setTimeout(done, latency));
      pending.resolve(
        page([
          {
            ...header("same-id"),
            header: { ...header("same-id").header, title: "private-old" },
          },
        ]),
      );
    });
    expect(screen.queryByText("private-old")).toBeNull();
    expect(a.scope.queryClient.getQueryCache().getAll()).toHaveLength(0);
  },
);
it("uses the captured local always network mode offline, with no login or connector", async () => {
  onlineManager.setOnline(false);
  const local = timelineFixture("always");
  scopes.push(local.scope);
  await local.start();
  local.client.timelineHeaders.mockResolvedValue(
    page([header("offline-local")]),
  );
  render(<Host scope={local.scope} />);
  await screen.findByText("offline-local");
});
it("preserves remote loaded data while reconnecting and hides it immediately on access loss", async () => {
  const remote = timelineFixture("online");
  scopes.push(remote.scope);
  await remote.start();
  remote.client.timelineHeaders.mockResolvedValue(page([header("private")]));
  render(<Host scope={remote.scope} />);
  await screen.findByText("private");
  act(() =>
    remote.emit({ kind: "connection", state: "reconnecting", error: null }),
  );
  expect(screen.getByText("private")).toBeTruthy();
  act(() =>
    remote.emit({ kind: "connection", state: "access_denied", error: null }),
  );
  expect(screen.queryByText("private")).toBeNull();
  expect(screen.getByText("hidden")).toBeTruthy();
});

it("queries the nearest future header independently of loaded date pages and never polls RPCs each second", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-11-08T12:00:00Z"));
  const f = timelineFixture();
  scopes.push(f.scope);
  await f.start();
  f.client.timelineHeaders.mockResolvedValue(
    page([header("nearest-outside-any-loaded-page", Date.now() + 120000)]),
  );
  function Chrome() {
    const buckets = useNearestTimelineBuckets();
    return (
      <div>
        {buckets
          .flatMap((bucket) => bucket.items)
          .map((item) => item.data.title)
          .join(",")}
      </div>
    );
  }
  render(
    <Host scope={f.scope}>
      <Chrome />
    </Host>,
  );
  await act(async () => {
    for (let i = 0; i < 12; i++) await Promise.resolve();
  });
  expect(screen.getByText("nearest-outside-any-loaded-page")).toBeTruthy();
  expect(f.client.timelineHeaders).toHaveBeenCalledOnce();
  expect(f.client.timelineHeaders.mock.calls[0][0]).toMatchObject({
    view: { kind: "upcoming", after_ms: Date.now() },
    limit: 1,
    cursor: null,
  });
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(f.client.timelineHeaders).toHaveBeenCalledOnce();
  f.client.timelineHeaders.mockResolvedValue(page());
  await act(() => vi.advanceTimersByTimeAsync(115001));
  expect(f.client.timelineHeaders).toHaveBeenCalledTimes(2);
  expect(screen.queryByText("nearest-outside-any-loaded-page")).toBeNull();
});
