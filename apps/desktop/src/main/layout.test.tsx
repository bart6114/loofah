import { onlineManager } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import type { VaultScope } from "~/shared/application/vault-scope";
import type { ScopedTabsStore } from "~/store/zustand/tabs";
import { useTabs, useTabsStore } from "~/store/zustand/tabs";
import { scopedFixture } from "~/store/zustand/tabs/scoped-test-utils";

const mocks = vi.hoisted(() => ({
  scope: null as VaultScope | null,
  error: null as Error | null,
  retry: vi.fn(),
  services: vi.fn(),
  navigation: vi.fn(),
}));

vi.mock("@tanstack/react-router", () => ({
  useRouteContext: () => ({ aiTaskStore: {} }),
}));
vi.mock("@hypr/editor/task-storage", () => ({
  TaskStorageProvider: ({ children }: { children: React.ReactNode }) =>
    children,
}));
vi.mock("~/editor-bridge/task-storage", () => ({
  useStoreBackedTaskStorage: () => ({}),
}));
vi.mock("~/shared/application/vault-provider", () => ({
  LocalVaultProvider: ({ children }: { children: React.ReactNode }) => children,
  useVaultHost: () => ({
    scope: mocks.scope,
    error: mocks.error,
    retry: mocks.retry,
  }),
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
  NotificationProvider: ({ children }: { children: React.ReactNode }) =>
    children,
}));
vi.mock("~/ai/contexts", () => ({
  AITaskProvider: ({ children }: { children: React.ReactNode }) => children,
}));
vi.mock("./lifecycle", () => ({
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

import { ClassicMainLayout } from "./layout";

import { deviceVaultState } from "~/shared/application/device-vault-state";

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
