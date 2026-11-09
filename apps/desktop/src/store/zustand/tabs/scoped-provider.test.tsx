import { onlineManager } from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, expect, it, vi } from "vitest";

import {
  createTimelineSelectionStore,
  useTimelineSelection,
  useTimelineSelectionStore,
} from "../timeline-selection";
import {
  ScopedTabsProvider,
  TabsStoreProvider,
  useScopedTabs,
  useScopedTabsStore,
} from "./scoped-provider";
import { deviceFixture, scopedFixture } from "./scoped-test-utils";
import { createTabsStore, type ScopedTabsStore } from "./store";

import { useSessionSummaries } from "~/session/summary-queries";
import type {
  VaultClient,
  VaultClientEvent,
} from "~/shared/application/vault-client";
import { VaultScope } from "~/shared/application/vault-scope";

const realScopes: VaultScope[] = [];

afterEach(() => {
  cleanup();
  for (const scope of realScopes.splice(0)) scope.dispose();
  onlineManager.setOnline(true);
});

it("remounts query observers when a replacement scope has the same scope string and note IDs", async () => {
  const row = (title: string) => [
    { id: "same-note", title, created_at: "2026-01-01" },
  ];
  const firstRead = vi.fn().mockResolvedValue(row("PRIVATE FIRST"));
  let finish!: (rows: ReturnType<typeof row>) => void;
  const secondRead = vi.fn(
    () =>
      new Promise<ReturnType<typeof row>>((resolve) => {
        finish = resolve;
      }),
  );
  const create = (sessionSummaries: VaultClient["sessionSummaries"]) => {
    const scope = new VaultScope({
      scope: "remote:same-scope",
      networkMode: "always",
      sessionSummaries,
      subscribe: async (receive: (event: VaultClientEvent) => void) => {
        receive({ kind: "connection", state: "connected", error: null });
        return () => {};
      },
    } as unknown as VaultClient);
    realScopes.push(scope);
    return scope;
  };
  const first = create(firstRead),
    second = create(secondRead);
  await Promise.all([first.start(), second.start()]);
  const { device } = deviceFixture();
  function Probe({ scope }: { scope: VaultScope }) {
    const result = useSessionSummaries(scope, {
      kind: "ids",
      ids: ["same-note"],
    });
    return <output>{result.data?.[0]?.title ?? "pending"}</output>;
  }
  const { rerender } = render(
    <ScopedTabsProvider scope={first} deviceState={device} managePins={false}>
      <Probe scope={first} />
    </ScopedTabsProvider>,
  );
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("PRIVATE FIRST"),
  );
  rerender(
    <ScopedTabsProvider scope={second} deviceState={device} managePins={false}>
      <Probe scope={second} />
    </ScopedTabsProvider>,
  );
  expect(screen.queryByText("PRIVATE FIRST")).toBeNull();
  await waitFor(() => expect(secondRead).toHaveBeenCalledOnce());
  expect(screen.getByRole("status").textContent).toBe("pending");
  await act(async () => finish(row("SECOND SCOPE")));
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("SECOND SCOPE"),
  );
  expect(
    first.queryClient.getQueryData(
      first.sessionSummariesOptions({ kind: "ids", ids: ["same-note"] })
        .queryKey,
    ),
  ).toEqual(row("PRIVATE FIRST"));
  expect(
    second.queryClient.getQueryData(
      second.sessionSummariesOptions({ kind: "ids", ids: ["same-note"] })
        .queryKey,
    ),
  ).toEqual(row("SECOND SCOPE"));
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

function SelectionProbe({
  label,
  capture,
}: {
  label: string;
  capture: (store: ReturnType<typeof createTimelineSelectionStore>) => void;
}) {
  const selection = useTimelineSelectionStore();
  capture(selection);
  const ids = useTimelineSelection((state) => state.selectedIds);
  return (
    <button onClick={() => selection.getState().toggleSelect("session-same")}>
      {label}:{ids.join(",")}
    </button>
  );
}

it("isolates simultaneous UI hosts and replaces selection by exact tab-store identity", () => {
  const first = createTabsStore("same-scope", () => true),
    other = createTabsStore("same-scope", () => true),
    replacement = createTabsStore("same-scope", () => true);
  let firstSelection!: ReturnType<typeof createTimelineSelectionStore>,
    otherSelection!: ReturnType<typeof createTimelineSelectionStore>;
  const tree = (owner: ScopedTabsStore) => (
    <>
      <TabsStoreProvider store={owner}>
        <SelectionProbe
          label="first"
          capture={(store) => {
            firstSelection = store;
          }}
        />
      </TabsStoreProvider>
      <TabsStoreProvider store={other}>
        <SelectionProbe
          label="other"
          capture={(store) => {
            otherSelection = store;
          }}
        />
      </TabsStoreProvider>
    </>
  );
  const mounted = render(tree(first));
  fireEvent.click(screen.getByRole("button", { name: "first:" }));
  expect(
    screen.getByRole("button", { name: "first:session-same" }),
  ).toBeTruthy();
  expect(screen.getByRole("button", { name: "other:" })).toBeTruthy();
  const retired = firstSelection;
  const oldHandler = () => retired.getState().toggleSelect("session-same");
  fireEvent.click(screen.getByRole("button", { name: "other:" }));
  const otherStore = otherSelection;
  const history = replacement.getState().history;
  mounted.rerender(tree(replacement));
  expect(screen.getByRole("button", { name: "first:" })).toBeTruthy();
  act(oldHandler);
  expect(firstSelection).not.toBe(retired);
  expect(retired.getState().selectedIds).toEqual([]);
  expect(firstSelection.getState().selectedIds).toEqual([]);
  expect(otherSelection).toBe(otherStore);
  expect(otherSelection.getState().selectedIds).toEqual(["session-same"]);
  expect(replacement.getState().history).toBe(history);
  expect(replacement.getState().tabs).toEqual([]);
});

it("starts a fresh working selection lifetime under StrictMode for a standalone tab host", () => {
  const tabs = createTabsStore("local:standalone", () => true);
  let selection!: ReturnType<typeof createTimelineSelectionStore>;
  const mounted = render(
    <StrictMode>
      <TabsStoreProvider store={tabs}>
        <SelectionProbe
          label="standalone"
          capture={(store) => {
            selection = store;
          }}
        />
      </TabsStoreProvider>
    </StrictMode>,
  );
  fireEvent.click(screen.getByRole("button", { name: "standalone:" }));
  expect(selection.getState().selectedIds).toEqual(["session-same"]);
  const late = selection.getState().selectAll;
  mounted.unmount();
  act(() => late(["session-same"]));
  expect(selection.getState()).toMatchObject({
    selectedIds: [],
    anchorId: null,
  });
});

it("retires selection with same-string replacement scopes while standalone pin ownership stays disabled", async () => {
  const first = scopedFixture("local:same"),
    second = scopedFixture("local:same");
  const { device } = deviceFixture();
  let current!: ReturnType<typeof createTimelineSelectionStore>;
  const tree = (scope: VaultScope) => (
    <ScopedTabsProvider scope={scope} deviceState={device} managePins={false}>
      <SelectionProbe
        label="scoped"
        capture={(store) => {
          current = store;
        }}
      />
    </ScopedTabsProvider>
  );
  const mounted = render(tree(first.scope));
  fireEvent.click(await screen.findByRole("button", { name: "scoped:" }));
  const old = current,
    lateRange = old.getState().selectRange;
  mounted.rerender(tree(second.scope));
  await screen.findByRole("button", { name: "scoped:" });
  act(() => lateRange(["session-same"], "session-same"));
  expect(current).not.toBe(old);
  expect(old.getState().selectedIds).toEqual([]);
  expect(current.getState().selectedIds).toEqual([]);
  expect(device.pinnedTabs).not.toHaveBeenCalled();
  expect(device.setPinnedTabs).not.toHaveBeenCalled();
});

it("requires an explicit selection host instead of routing through a global store", () => {
  const error = vi.spyOn(console, "error").mockImplementation(() => {});
  function Probe() {
    useTimelineSelection((state) => state.selectedIds);
    return null;
  }
  expect(() => render(<Probe />)).toThrow(
    "Timeline selection requires an immutable tab host",
  );
  error.mockRestore();
});
