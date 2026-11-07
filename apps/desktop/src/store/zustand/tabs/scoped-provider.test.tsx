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
