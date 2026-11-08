import { onlineManager } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { StrictMode, useContext } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import type { VaultScope } from "~/shared/application/vault-scope";
import type { ScopedTabsStore } from "~/store/zustand/tabs";
import { useTabs, useTabsStore } from "~/store/zustand/tabs";
import {
  deferred,
  deviceFixture,
  scopedFixture,
} from "~/store/zustand/tabs/scoped-test-utils";

const mocks = vi.hoisted(() => ({
  scope: null as VaultScope | null,
  error: null as Error | null,
  retry: vi.fn(),
  services: vi.fn(),
  navigation: vi.fn(),
  local: vi.fn(),
  notification: vi.fn(),
  taskStorage: vi.fn(),
  listener: {},
}));

vi.mock("@tanstack/react-router", () => ({
  useRouteContext: () => ({ aiTaskStore: {} }),
}));
vi.mock("@hypr/editor/task-storage", () => ({
  TaskStorageProvider: ({ children }: { children: React.ReactNode }) =>
    children,
}));
vi.mock("~/editor-bridge/task-storage", () => ({
  useStoreBackedTaskStorage: mocks.taskStorage,
}));
vi.mock("~/shared/application/vault-provider", async () => {
  const { createContext, useContext } = await import("react");
  const VaultHostContext = createContext<{
    scope: VaultScope | null;
    error: Error | null;
    retry: () => void;
  } | null>(null);
  return {
    VaultHostContext,
    LocalVaultProvider: ({ children }: { children: React.ReactNode }) => {
      mocks.local();
      return (
        <VaultHostContext.Provider
          value={{ scope: mocks.scope, error: mocks.error, retry: mocks.retry }}
        >
          {children}
        </VaultHostContext.Provider>
      );
    },
    useVaultHost: () => useContext(VaultHostContext)!,
  };
});
vi.mock("~/stt/contexts", () => ({
  useListenerStore: () => mocks.listener,
  ListenerContextProvider: ({ children }: { children: React.ReactNode }) =>
    children,
}));
vi.mock("~/shared/application/device-vault-state", () => ({
  deviceVaultState: {
    recentSessions: vi.fn(async (_scope: string): Promise<string[]> => []),
    rememberSession: vi.fn(async (_scope: string, id: string) => [id]),
    pinnedTabs: vi.fn(async (_scope: string): Promise<string | null> => null),
    setPinnedTabs: vi.fn(async (_scope: string, _value: string) => {}),
    subscribe: vi.fn(async (_scope: string, _changed: () => void) => () => {}),
  },
}));
vi.mock("~/shared/open-note-dialog", () => ({
  OpenNoteDialogProvider: ({ children }: { children: React.ReactNode }) =>
    children,
}));
vi.mock("~/contexts/shell", () => ({
  ShellProvider: ({ children }: { children: React.ReactNode }) => children,
}));
vi.mock("~/contexts/notifications", () => ({
  NotificationProvider: ({ children }: { children: React.ReactNode }) => {
    mocks.notification();
    return children;
  },
}));
vi.mock("~/ai/contexts", () => ({
  AITaskProvider: ({ children }: { children: React.ReactNode }) => children,
}));
vi.mock("./lifecycle", () => ({
  createLocalMainCaptureActivity: (scope: VaultScope) => ({
    scope,
    captureState: () => "idle",
    subscribeCapture: () => () => {},
  }),
  ClassicMainServices: () => {
    mocks.services();
    return null;
  },
}));
vi.mock("~/shared/hooks/useNavigationEvents", async () => {
  const { useTabsStore } = await import("~/store/zustand/tabs");
  return {
    useNavigationEvents: () => {
      mocks.navigation(useTabsStore().scope);
    },
  };
});

import { CapturedMainLayout, ClassicMainLayout } from "./layout";

import { deviceVaultState } from "~/shared/application/device-vault-state";
import { VaultHostContext } from "~/shared/application/vault-provider";
import type { AITaskStore } from "~/store/zustand/ai-task";
import type { ListenerStore } from "~/store/zustand/listener";

let captured: ScopedTabsStore;
function Probe() {
  captured = useTabsStore();
  const ids = useTabs((state) => state.recentlyOpenedSessionIds);
  return <output>{ids.join(",")}</output>;
}

beforeEach(() => {
  vi.clearAllMocks();
  mocks.scope = scopedFixture("local:layout").scope;
  mocks.error = null;
  vi.mocked(deviceVaultState.recentSessions).mockResolvedValue(["local-note"]);
  vi.mocked(deviceVaultState.pinnedTabs).mockResolvedValue(null);
});
afterEach(() => {
  cleanup();
  onlineManager.setOnline(true);
});

it("provides captured login-free local tabs and device state while offline", async () => {
  onlineManager.setOnline(false);
  render(
    <ClassicMainLayout>
      <Probe />
    </ClassicMainLayout>,
  );
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("local-note"),
  );
  expect(captured.scope).toBe("local:layout");
  act(() =>
    captured.getState().openCurrent({ type: "sessions", id: "selected" }),
  );
  await waitFor(() =>
    expect(deviceVaultState.rememberSession).toHaveBeenCalledWith(
      "local:layout",
      "selected",
    ),
  );
  expect(mocks.services).toHaveBeenCalled();
  expect(mocks.navigation).toHaveBeenCalledWith("local:layout");
  expect(mocks.notification).toHaveBeenCalled();
  expect(mocks.taskStorage).not.toHaveBeenCalled();
});

it("waits for the resolved local scope and never borrows another tab store", async () => {
  mocks.scope = null;
  const { rerender } = render(
    <ClassicMainLayout>
      <Probe />
    </ClassicMainLayout>,
  );
  expect(screen.getByRole("status").textContent).toBe("Loading content…");
  expect(deviceVaultState.recentSessions).not.toHaveBeenCalled();
  mocks.scope = scopedFixture("local:ready").scope;
  rerender(
    <ClassicMainLayout>
      <Probe />
    </ClassicMainLayout>,
  );
  await waitFor(() => expect(captured.scope).toBe("local:ready"));
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("local-note"),
  );
});

it("keeps standalone note windows outside main services and pin ownership", async () => {
  render(
    <ClassicMainLayout includeServices={false}>
      <Probe />
    </ClassicMainLayout>,
  );
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("local-note"),
  );
  act(() => {
    captured.getState().openCurrent({ type: "sessions", id: "standalone" });
    captured.getState().pin(captured.getState().currentTab!);
  });
  await waitFor(() =>
    expect(deviceVaultState.rememberSession).toHaveBeenCalledWith(
      "local:layout",
      "standalone",
    ),
  );
  expect(deviceVaultState.pinnedTabs).not.toHaveBeenCalled();
  expect(deviceVaultState.setPinnedTabs).not.toHaveBeenCalled();
  expect(mocks.services).not.toHaveBeenCalled();
  expect(mocks.navigation).toHaveBeenCalledWith("local:layout");
});

it("removes the previous scope before rendering replacement device state with identical IDs", async () => {
  vi.mocked(deviceVaultState.recentSessions).mockImplementation(
    async (scope) => ["same-id", scope],
  );
  mocks.scope = scopedFixture("local:first").scope;
  const { rerender } = render(
    <ClassicMainLayout>
      <Probe />
    </ClassicMainLayout>,
  );
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("same-id,local:first"),
  );
  const first = captured;
  mocks.scope = scopedFixture("local:second").scope;
  rerender(
    <ClassicMainLayout>
      <Probe />
    </ClassicMainLayout>,
  );
  expect(screen.queryByText("same-id,local:first")).toBeNull();
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("same-id,local:second"),
  );
  act(() => first.getState().openNew({ type: "sessions", id: "late-private" }));
  expect(first.getState().tabs).toEqual([]);
  expect(deviceVaultState.rememberSession).not.toHaveBeenCalled();
});

function HostProbe() {
  const host = useContext(VaultHostContext);
  return (
    <>
      <span data-testid="host">{host?.scope?.client.scope}</span>
      <Probe />
    </>
  );
}
const genericStores = {
  aiTaskStore: {} as AITaskStore,
  listenerStore: {} as ListenerStore,
};

it("uses only the explicit remote host without connecting local or mounting local runtime/model/task bridges", async () => {
  const { scope } = scopedFixture("remote:main");
  render(
    <CapturedMainLayout scope={scope} {...genericStores}>
      <HostProbe />
    </CapturedMainLayout>,
  );
  await waitFor(() => expect(captured.scope).toBe("remote:main"));
  expect(screen.getByTestId("host").textContent).toBe("remote:main");
  expect(mocks.local).not.toHaveBeenCalled();
  expect(mocks.services).not.toHaveBeenCalled();
  expect(mocks.notification).not.toHaveBeenCalled();
  expect(mocks.navigation).not.toHaveBeenCalled();
  expect(mocks.taskStorage).not.toHaveBeenCalled();
});

it("remounts the main subtree for a new Scope object with the same scope string under StrictMode", async () => {
  const first = scopedFixture("remote:same"),
    second = scopedFixture("remote:same");
  const { rerender } = render(
    <StrictMode>
      <CapturedMainLayout scope={first.scope} {...genericStores}>
        <HostProbe />
      </CapturedMainLayout>
    </StrictMode>,
  );
  await waitFor(() => expect(captured.scope).toBe("remote:same"));
  const old = captured;
  act(() => old.getState().openCurrent({ type: "sessions", id: "same-id" }));
  rerender(
    <StrictMode>
      <CapturedMainLayout scope={second.scope} {...genericStores}>
        <HostProbe />
      </CapturedMainLayout>
    </StrictMode>,
  );
  await waitFor(() => expect(captured).not.toBe(old));
  expect(old.getState().tabs).toEqual([]);
  act(() => old.getState().openCurrent({ type: "sessions", id: "late-old" }));
  expect(captured.getState().tabs).toEqual([]);
  expect(mocks.local).not.toHaveBeenCalled();
});

it.each([50, 150, 500])(
  "never admits delayed old host device state after Scope replacement (%ims)",
  async (latency) => {
    const first = scopedFixture("remote:first"),
      second = scopedFixture("remote:second");
    const { device } = deviceFixture();
    const read = deferred<string[]>();
    device.recentSessions.mockImplementation((scope) =>
      scope === first.scope.client.scope
        ? read.promise
        : Promise.resolve(["same-id", "second-only"]),
    );
    const { rerender, unmount } = render(
      <CapturedMainLayout
        scope={first.scope}
        deviceState={device}
        {...genericStores}
      >
        <HostProbe />
      </CapturedMainLayout>,
    );
    await waitFor(() =>
      expect(device.recentSessions).toHaveBeenCalledWith(
        first.scope.client.scope,
      ),
    );
    const old = captured;
    rerender(
      <CapturedMainLayout
        scope={second.scope}
        deviceState={device}
        {...genericStores}
      >
        <HostProbe />
      </CapturedMainLayout>,
    );
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toBe(
        "same-id,second-only",
      ),
    );
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, latency));
      read.resolve(["same-id", "first-private"]);
    });
    expect(old.getState().tabs).toEqual([]);
    expect(screen.getByRole("status").textContent).toBe("same-id,second-only");
    unmount();
    expect(device.setPinnedTabs).not.toHaveBeenCalled();
  },
);

it("remounts pinned provider contexts when an explicit listener or AI store is replaced in the same Scope", async () => {
  const { scope } = scopedFixture("remote:stores");
  const { rerender } = render(
    <CapturedMainLayout scope={scope} {...genericStores}>
      <HostProbe />
    </CapturedMainLayout>,
  );
  await waitFor(() => expect(captured.scope).toBe("remote:stores"));
  const old = captured;
  act(() =>
    old.getState().openCurrent({ type: "sessions", id: "old-selection" }),
  );
  const replacement = {
    ...genericStores,
    aiTaskStore: {} as AITaskStore,
    listenerStore: {} as ListenerStore,
  };
  rerender(
    <CapturedMainLayout scope={scope} {...replacement}>
      <HostProbe />
    </CapturedMainLayout>,
  );
  await waitFor(() => expect(captured).not.toBe(old));
  expect(old.getState().tabs).toEqual([]);
  expect(captured.getState().tabs).toEqual([]);
});
