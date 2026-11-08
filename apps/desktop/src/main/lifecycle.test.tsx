import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import type { VaultScope } from "~/shared/application/vault-scope";
import {
  useTabs,
  useTabsStore,
  type ScopedTabsStore,
} from "~/store/zustand/tabs";
import { ScopedTabsProvider } from "~/store/zustand/tabs/scoped-provider";
import {
  deferred,
  deviceFixture,
  scopedFixture,
} from "~/store/zustand/tabs/scoped-test-utils";

const mocks = vi.hoisted(() => ({
  scope: null as VaultScope | null,
  welcome: vi.fn(),
  bridge: vi.fn(),
}));
vi.mock("~/shared/application/vault-provider", () => ({
  useVaultHost: () => ({ scope: mocks.scope }),
}));
vi.mock("~/onboarding/welcome-note", () => ({
  takePendingWelcomeSession: mocks.welcome,
}));
vi.mock("~/services/enhancer", () => ({
  initEnhancerService: vi.fn(() => ({ dispose: vi.fn() })),
}));
vi.mock("~/ai/hooks", () => ({ useLanguageModel: () => null }));
vi.mock("~/stt/window-control", () => ({
  MainListenerControlBridge: () => null,
}));
vi.mock("@tanstack/react-router", () => ({
  useRouteContext: () => ({ aiTaskStore: null }),
}));
vi.mock("~/services/event-listeners", async () => {
  const { useTabsStore } = await import("~/store/zustand/tabs");
  return {
    EventListeners: () => {
      mocks.bridge("notifications", useTabsStore().scope);
      return null;
    },
  };
});
vi.mock("~/sidebar/toast/undo-delete-toast", async () => {
  const { useTabsStore } = await import("~/store/zustand/tabs");
  return {
    UndoDeleteToast: () => {
      mocks.bridge("undo-toast", useTabsStore().scope);
      return null;
    },
  };
});
vi.mock("~/devtools-panel/host", async () => {
  const { useTabsStore } = await import("~/store/zustand/tabs");
  return {
    DevtoolsFloatingPanelHost: () => {
      mocks.bridge("devtools", useTabsStore().scope);
      return null;
    },
  };
});
vi.mock("~/session/hooks/useDeleteSession", async () => {
  const { useTabsStore } = await import("~/store/zustand/tabs");
  return {
    useRemoteSessionDeletionUndoListener: () => {
      mocks.bridge("undo-events", useTabsStore().scope);
    },
  };
});

import { MainHostContext, type MainCaptureActivity } from "./host-context";
import {
  ClassicMainServices,
  createLocalMainCaptureActivity,
  useClassicMainLifecycle,
} from "./lifecycle";

import { NativeDraftStore } from "~/shared/application/draft-store";
import { createListenerStore } from "~/store/zustand/listener";
import { listenerStore } from "~/store/zustand/listener/instance";

let captured: ScopedTabsStore;
function Probe({
  activity,
  generic = false,
}: {
  activity?: MainCaptureActivity;
  generic?: boolean;
}) {
  const scope = mocks.scope!;
  return (
    <MainHostContext.Provider
      value={{
        scope,
        listenerStore,
        activity:
          activity ??
          (generic
            ? undefined
            : createLocalMainCaptureActivity(scope, listenerStore)),
        takeInitialSession: generic ? undefined : mocks.welcome,
      }}
    >
      <LifecycleProbe />
    </MainHostContext.Provider>
  );
}
function LifecycleProbe() {
  useClassicMainLifecycle();
  captured = useTabsStore();
  const current = useTabs((state) => state.currentTab);
  return (
    <output>
      {current ? ("id" in current ? current.id : current.type) : "pending"}
    </output>
  );
}

beforeEach(() => {
  mocks.welcome.mockReset().mockReturnValue(null);
  mocks.bridge.mockClear();
});
afterEach(cleanup);

it("binds every tab-dependent bootstrap bridge to the captured main store", async () => {
  const { scope } = scopedFixture("local:services");
  const { device } = deviceFixture();
  mocks.scope = scope;
  render(
    <ScopedTabsProvider scope={scope} deviceState={device} managePins>
      <ClassicMainServices />
    </ScopedTabsProvider>,
  );
  await waitFor(() => {
    for (const bridge of [
      "notifications",
      "undo-toast",
      "devtools",
      "undo-events",
    ])
      expect(mocks.bridge).toHaveBeenCalledWith(bridge, "local:services");
  });
});

it("waits for canonical pinned restoration before opening any default tab", async () => {
  const { scope, summaries } = scopedFixture("local:pins");
  const { device, pins } = deviceFixture();
  mocks.scope = scope;
  pins.set(
    scope.client.scope,
    JSON.stringify([{ type: "sessions", id: "pinned", pinned: true }]),
  );
  const lookup =
    deferred<{ id: string; title: string; created_at: string }[]>();
  summaries.mockReturnValueOnce(lookup.promise);
  render(
    <ScopedTabsProvider scope={scope} deviceState={device} managePins>
      <Probe />
    </ScopedTabsProvider>,
  );
  await waitFor(() => expect(summaries).toHaveBeenCalledOnce());
  expect(screen.getByRole("status").textContent).toBe("pending");
  await act(async () =>
    lookup.resolve([
      { id: "pinned", title: "Pinned", created_at: "2026-01-01" },
    ]),
  );
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("pinned"),
  );
  expect(captured.getState().tabs).toMatchObject([
    { id: "pinned", pinned: true },
  ]);
});

it("consumes the local welcome handoff once after restoration under StrictMode", async () => {
  const { scope } = scopedFixture("local:welcome");
  const { device, pins } = deviceFixture();
  mocks.scope = scope;
  mocks.welcome.mockReturnValueOnce("welcome");
  pins.set(
    scope.client.scope,
    JSON.stringify([{ type: "sessions", id: "pinned", pinned: true }]),
  );
  render(
    <StrictMode>
      <ScopedTabsProvider scope={scope} deviceState={device} managePins>
        <Probe />
      </ScopedTabsProvider>
    </StrictMode>,
  );
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("welcome"),
  );
  expect(mocks.welcome).toHaveBeenCalledOnce();
  expect(captured.getState().tabs).toMatchObject([
    { id: "pinned", pinned: true },
    { id: "welcome" },
  ]);
});

it("installs a fresh live close handler after StrictMode effect teardown", async () => {
  const { scope } = scopedFixture("local:strict-cleanup");
  const request = vi.fn().mockResolvedValue({ kind: "not_empty" });
  Object.assign(scope.client, {
    features: ["session_cleanup"],
    networkMode: "always",
    deleteEmptySession: request,
  });
  const resync = vi.fn();
  Object.assign(scope, { resync });
  const inventory = vi
    .spyOn(NativeDraftStore.prototype, "inventory")
    .mockResolvedValue([]);
  const { device } = deviceFixture();
  mocks.scope = scope;
  render(
    <StrictMode>
      <ScopedTabsProvider scope={scope} deviceState={device} managePins>
        <Probe />
      </ScopedTabsProvider>
    </StrictMode>,
  );
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("empty"),
  );
  act(() =>
    captured.getState().openCurrent({ type: "sessions", id: "strict-note" }),
  );
  act(() => captured.getState().close(captured.getState().currentTab!));
  expect(screen.getByRole("status").textContent).toBe("empty");
  await waitFor(() => expect(request).toHaveBeenCalledOnce());
  await waitFor(() => expect(resync).toHaveBeenCalledOnce());
  inventory.mockRestore();
});

it("keeps startup callbacks pending until the captured local connection is ready", async () => {
  const fixture = scopedFixture("local:connecting");
  const { device } = deviceFixture();
  fixture.state("connecting");
  mocks.scope = fixture.scope;
  render(
    <ScopedTabsProvider scope={fixture.scope} deviceState={device} managePins>
      <Probe />
    </ScopedTabsProvider>,
  );
  await waitFor(() => expect(device.recentSessions).toHaveBeenCalledOnce());
  expect(screen.getByRole("status").textContent).toBe("pending");
  expect(mocks.welcome).not.toHaveBeenCalled();
  act(() => fixture.state("connected"));
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("empty"),
  );
});

it("does not apply delayed startup or welcome callbacks from a retired scope", async () => {
  const first = scopedFixture("local:first"),
    second = scopedFixture("local:second");
  const { device, pins } = deviceFixture();
  mocks.scope = first.scope;
  pins.set(
    first.scope.client.scope,
    JSON.stringify([{ type: "sessions", id: "private-first", pinned: true }]),
  );
  const lookup =
    deferred<{ id: string; title: string; created_at: string }[]>();
  first.summaries.mockReturnValueOnce(lookup.promise);
  const { rerender } = render(
    <ScopedTabsProvider scope={first.scope} deviceState={device} managePins>
      <Probe />
    </ScopedTabsProvider>,
  );
  await waitFor(() => expect(first.summaries).toHaveBeenCalledOnce());
  const old = captured;
  mocks.scope = second.scope;
  rerender(
    <ScopedTabsProvider scope={second.scope} deviceState={device} managePins>
      <Probe />
    </ScopedTabsProvider>,
  );
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("empty"),
  );
  await act(async () =>
    lookup.resolve([
      { id: "private-first", title: "Private", created_at: "2026-01-01" },
    ]),
  );
  expect(old.getState().tabs).toEqual([]);
  expect(screen.getByRole("status").textContent).toBe("empty");
  expect(mocks.welcome).toHaveBeenCalledOnce();
});

it.each(["absent", "different-scope", "unknown"])(
  "skips close cleanup for %s capture ownership and never consumes Local welcome",
  async (kind) => {
    const { scope } = scopedFixture("remote:cleanup");
    const request = vi.fn().mockResolvedValue({ kind: "not_empty" });
    Object.assign(scope.client, {
      features: ["session_cleanup"],
      networkMode: "always",
      deleteEmptySession: request,
    });
    const { device } = deviceFixture();
    mocks.scope = scope;
    const activity =
      kind === "absent"
        ? undefined
        : {
            scope:
              kind === "different-scope"
                ? scopedFixture("remote:cleanup").scope
                : scope,
            captureState: () => (kind === "unknown" ? "unknown" : "idle"),
            subscribeCapture: vi.fn(() => () => {}),
          };
    render(
      <ScopedTabsProvider scope={scope} deviceState={device} managePins>
        <Probe generic activity={activity} />
      </ScopedTabsProvider>,
    );
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toBe("empty"),
    );
    act(() =>
      captured.getState().openCurrent({ type: "sessions", id: "same-id" }),
    );
    act(() => captured.getState().close(captured.getState().currentTab!));
    expect(screen.getByRole("status").textContent).toBe("empty");
    await act(async () => {});
    expect(request).not.toHaveBeenCalled();
    expect(mocks.welcome).not.toHaveBeenCalled();
  },
);

it("captures Local activity from the supplied store and treats uninitialized capture as unknown", () => {
  const { scope } = scopedFixture("local:captured-activity");
  const capture = createListenerStore(),
    unrelated = createListenerStore();
  const activity = createLocalMainCaptureActivity(scope, capture);
  expect(activity.scope).toBe(scope);
  capture.setState((state) => ({ live: { ...state.live, loading: true } }));
  expect(activity.captureState("same-id")).toBe("unknown");
  capture.setState((state) => ({
    live: {
      ...state.live,
      loading: false,
      sessionId: "same-id",
      status: "inactive",
    },
  }));
  expect(activity.captureState("same-id")).toBe("idle");
  const changed = vi.fn();
  const stop = activity.subscribeCapture("same-id", changed);
  unrelated.setState((state) => ({ live: { ...state.live, loading: true } }));
  expect(changed).not.toHaveBeenCalled();
  capture.setState((state) => ({ live: { ...state.live, status: "active" } }));
  expect(activity.captureState("same-id")).toBe("active");
  expect(changed).toHaveBeenCalledOnce();
  stop();
  capture.setState((state) => ({
    live: { ...state.live, status: "inactive" },
  }));
  expect(changed).toHaveBeenCalledOnce();
});
