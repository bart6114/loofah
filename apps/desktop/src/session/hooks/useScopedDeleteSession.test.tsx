import { onlineManager } from "@tanstack/react-query";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  loading: vi.fn(),
  error: vi.fn(),
  dismiss: vi.fn(),
  emit: vi.fn(async () => {}),
  label: vi.fn(() => "main"),
  windows: vi.fn(async () => []),
  legacyDelete: vi.fn(),
  legacyRestore: vi.fn(() => vi.fn(async () => {})),
}));
vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: {
    loading: mocks.loading,
    error: mocks.error,
    dismiss: mocks.dismiss,
  },
}));
vi.mock("@tauri-apps/api/event", () => ({
  emitTo: mocks.emit,
  listen: vi.fn(async () => () => {}),
}));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getAllWebviewWindows: mocks.windows,
}));
vi.mock("@hypr/plugin-windows", () => ({
  getCurrentWebviewWindowLabel: mocks.label,
}));
vi.mock("~/session/queries", () => ({
  softDeleteSession: mocks.legacyDelete,
  captureSessionRestore: mocks.legacyRestore,
}));

import { useDeleteSession } from "./useDeleteSession";

import { MainHostContext } from "~/main/host-context";
import { ApplicationRequestError } from "~/shared/application/errors";
import type { DeleteSessionOutcome } from "~/shared/application/protocol.gen";
import type {
  VaultClient,
  VaultClientEvent,
} from "~/shared/application/vault-client";
import { VaultHostContext } from "~/shared/application/vault-provider";
import { VaultScope } from "~/shared/application/vault-scope";
import type { ListenerStore } from "~/store/zustand/listener";
import { TabsStoreProvider } from "~/store/zustand/tabs/scoped-provider";
import { createTabsStore } from "~/store/zustand/tabs/store";
import { deletionKey, useUndoDelete } from "~/store/zustand/undo-delete";

const scopes: VaultScope[] = [];
const revision = "a".repeat(64),
  trash_token = "b".repeat(64);
function failure(
  outcome: "unknown" | "not_committed" = "unknown",
  code: "unavailable" | "not_found" = "unavailable",
) {
  return new ApplicationRequestError({
    code,
    message: "lost response",
    request_id: null,
    retryable: false,
    current_revision: null,
    mutation_outcome: outcome,
  });
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
beforeEach(() => {
  vi.clearAllMocks();
  useUndoDelete.setState({ pendingDeletions: {} });
  onlineManager.setOnline(true);
});
afterEach(() => {
  cleanup();
  for (const entry of Object.values(useUndoDelete.getState().pendingDeletions))
    if (entry.timeoutId) clearTimeout(entry.timeoutId);
  useUndoDelete.setState({ pendingDeletions: {} });
  for (const scope of scopes.splice(0)) scope.dispose();
});
async function fixture(mode: "always" | "online" = "online", legacy = false) {
  let receive!: (event: VaultClientEvent) => void;
  let present = true;
  const access = {
    canEdit: true,
    features: legacy
      ? ["legacy_desktop_session_controls"]
      : ["session_lifecycle"],
  };
  const read = vi
    .fn<VaultClient["readSessionLifecycle"]>()
    .mockImplementation(async (session_id) => {
      if (!present) throw failure("not_committed", "not_found");
      return { session_id, revision };
    });
  const remove = vi
    .fn<VaultClient["deleteSession"]>()
    .mockImplementation(async () => {
      present = false;
      return { kind: "trashed", trash_token, revision };
    });
  const restore = vi
    .fn<VaultClient["restoreSession"]>()
    .mockImplementation(async () => {
      present = true;
      return { kind: "restored", session_id: "same-id", revision };
    });
  const client = {
    scope: "same-vault",
    networkMode: mode,
    get canEdit() {
      return access.canEdit;
    },
    get features() {
      return access.features;
    },
    readSessionLifecycle: read,
    deleteSession: remove,
    restoreSession: restore,
    subscribe: async (listener: (event: VaultClientEvent) => void) => {
      receive = listener;
      listener({ kind: "connection", state: "connected", error: null });
      return () => {};
    },
  } as unknown as VaultClient;
  const scope = new VaultScope(client);
  scopes.push(scope);
  await scope.start();
  const store = createTabsStore(
    client.scope,
    () => scope.getSnapshot().state !== "closed",
  );
  store.getState().openCurrent({ type: "sessions", id: "same-id" });
  const listener = {
    getState: () => ({
      live: { status: "inactive", sessionId: null, loading: false },
      stop: vi.fn(),
    }),
    subscribe: () => () => {},
  } as unknown as ListenerStore;
  const host = {
    scope,
    listenerStore: listener,
    activity: {
      scope,
      captureState: () => "idle",
      subscribeCapture: () => () => {},
    },
  };
  function Wrapper({ children }: { children: ReactNode }) {
    return (
      <VaultHostContext.Provider value={{ scope, error: null, retry() {} }}>
        <MainHostContext.Provider value={host}>
          <TabsStoreProvider store={store}>{children}</TabsStoreProvider>
        </MainHostContext.Provider>
      </VaultHostContext.Provider>
    );
  }
  return {
    scope,
    store,
    read,
    remove,
    restore,
    access,
    Wrapper,
    setPresent: (value: boolean) => {
      present = value;
    },
    emit: (state: "access_denied" | "reconnecting") =>
      receive({ kind: "connection", state, error: null }),
  };
}
it.each(["always", "online"] as const)(
  "shared %s requests dedupe, preserve IDs, and put an owned opaque receipt in the real undo store",
  async (mode) => {
    const f = await fixture(mode);
    const { result } = renderHook(() => useDeleteSession(), {
      wrapper: f.Wrapper,
    });
    act(() => {
      result.current("same-id", { title: "Note" });
      result.current("same-id", { title: "Note" });
    });
    await waitFor(() =>
      expect(
        useUndoDelete.getState().pendingDeletions[
          deletionKey("same-vault", "same-id")
        ],
      ).toBeDefined(),
    );
    expect(f.remove).toHaveBeenCalledTimes(1);
    const entry =
      useUndoDelete.getState().pendingDeletions[
        deletionKey("same-vault", "same-id")
      ];
    expect(entry.owner).toBe(f.scope);
    expect(entry.data.receipt?.outcome).toEqual({
      kind: "trashed",
      trash_token,
      revision,
    });
    expect(entry.data.confirmedAbsent).toBe(true);
    expect(entry.data.tombstone).toBe(f.remove.mock.calls[0][1]?.requestId);
    await act(async () => entry.restore(() => {}));
    expect(f.restore).toHaveBeenCalledWith(
      { trash_token, expected_revision: revision },
      expect.objectContaining({ requestId: expect.any(String) }),
    );
    expect(mocks.legacyDelete).not.toHaveBeenCalled();
    expect(mocks.legacyRestore).not.toHaveBeenCalled();
    expect(mocks.emit).not.toHaveBeenCalled();
    expect(mocks.label).not.toHaveBeenCalled();
    expect(mocks.windows).not.toHaveBeenCalled();
  },
);
it("keeps default Local legacy deletion and its offline, login-free path while the checked host capability is disabled", async () => {
  const f = await fixture("always", true);
  onlineManager.setOnline(false);
  mocks.legacyDelete.mockResolvedValue({
    session: { id: "same-id", title: "Old" },
    tombstone: "legacy",
    deletedAt: Date.now(),
  });
  const { result } = renderHook(() => useDeleteSession(), {
    wrapper: f.Wrapper,
  });
  act(() => result.current("same-id", { title: "Old" }));
  await waitFor(() => expect(mocks.legacyDelete).toHaveBeenCalledTimes(1));
  expect(f.read).not.toHaveBeenCalled();
  expect(f.remove).not.toHaveBeenCalled();
});
it("holds unknown requests beyond the undo timer and retries only the original UUID and baseline", async () => {
  const f = await fixture();
  f.remove.mockRejectedValueOnce(failure());
  const { result } = renderHook(() => useDeleteSession(), {
    wrapper: f.Wrapper,
  });
  act(() => result.current("same-id"));
  await waitFor(() =>
    expect(mocks.error).toHaveBeenCalledWith(
      expect.stringContaining("original request"),
      expect.anything(),
    ),
  );
  const options = mocks.error.mock.calls[mocks.error.mock.calls.length - 1][1];
  const id = f.remove.mock.calls[0][1]?.requestId;
  f.read.mockResolvedValue({ session_id: "same-id", revision: "c".repeat(64) });
  await act(async () => options.action.onClick({ preventDefault() {} }));
  await waitFor(() => expect(f.remove).toHaveBeenCalledTimes(2));
  expect(f.remove.mock.calls[1]).toEqual([
    { session_id: "same-id", expected_revision: revision },
    { requestId: id },
  ]);
});
it("does not hide a currently present note or close device windows merely from historical success", async () => {
  const f = await fixture();
  f.remove.mockResolvedValue({ kind: "trashed", trash_token, revision });
  const { result } = renderHook(() => useDeleteSession(), {
    wrapper: f.Wrapper,
  });
  act(() => result.current("same-id"));
  await waitFor(() =>
    expect(
      useUndoDelete.getState().pendingDeletions[
        deletionKey("same-vault", "same-id")
      ],
    ).toBeDefined(),
  );
  const entry =
    useUndoDelete.getState().pendingDeletions[
      deletionKey("same-vault", "same-id")
    ];
  expect(entry.data.confirmedAbsent).toBe(false);
  expect(f.store.getState().currentTab).toMatchObject({ id: "same-id" });
  expect(mocks.windows).not.toHaveBeenCalled();
});
it("reader and disconnected clients refuse before any legacy or shared dispatch", async () => {
  for (const reason of ["reader", "disconnected"]) {
    const f = await fixture();
    if (reason === "reader") f.access.canEdit = false;
    else f.emit("reconnecting");
    const { result, unmount } = renderHook(() => useDeleteSession(), {
      wrapper: f.Wrapper,
    });
    act(() => result.current("same-id"));
    expect(f.read).not.toHaveBeenCalled();
    expect(f.remove).not.toHaveBeenCalled();
    expect(mocks.legacyDelete).not.toHaveBeenCalled();
    unmount();
  }
});
it("a late response cannot add undo state or replace another same-ID Scope", async () => {
  const old = await fixture();
  const delayed = deferred<DeleteSessionOutcome>();
  old.remove.mockReturnValue(delayed.promise);
  const { result } = renderHook(() => useDeleteSession(), {
    wrapper: old.Wrapper,
  });
  act(() => result.current("same-id"));
  await waitFor(() => expect(old.remove).toHaveBeenCalled());
  old.scope.dispose();
  const next = await fixture();
  delayed.resolve({ kind: "trashed", trash_token, revision });
  await act(async () => {
    await delayed.promise;
    await Promise.resolve();
  });
  expect(useUndoDelete.getState().pendingDeletions).toEqual({});
  expect(next.store.getState().currentTab).toMatchObject({ id: "same-id" });
  expect(next.remove).not.toHaveBeenCalled();
});

it("cancel after a lost acknowledgement reviews the original deletion, then restores only after explicit same-UUID retry", async () => {
  const f = await fixture();
  const lost = deferred<DeleteSessionOutcome>();
  f.remove.mockReturnValueOnce(lost.promise);
  const { result } = renderHook(() => useDeleteSession(), {
    wrapper: f.Wrapper,
  });
  act(() => result.current("same-id"));
  await waitFor(() => expect(f.remove).toHaveBeenCalledTimes(1));
  const id = f.remove.mock.calls[0][1]?.requestId;
  act(() => {
    mocks.loading.mock.calls[
      mocks.loading.mock.calls.length - 1
    ][1].action.onClick({ preventDefault() {} });
  });
  lost.reject(failure());
  await waitFor(() =>
    expect(mocks.error).toHaveBeenCalledWith(
      expect.stringContaining("original request"),
      expect.anything(),
    ),
  );
  expect(f.restore).not.toHaveBeenCalled();
  await act(async () =>
    mocks.error.mock.calls[mocks.error.mock.calls.length - 1][1].action.onClick(
      { preventDefault() {} },
    ),
  );
  await waitFor(() => expect(f.restore).toHaveBeenCalledTimes(1));
  expect(f.remove.mock.calls[1][1]?.requestId).toBe(id);
  expect(f.store.getState().currentTab).toMatchObject({ id: "same-id" });
});
it("does not reopen a missing note from a typed not-found deletion result", async () => {
  const f = await fixture();
  f.remove.mockResolvedValue({ kind: "not_found" });
  const { result } = renderHook(() => useDeleteSession(), {
    wrapper: f.Wrapper,
  });
  act(() => result.current("same-id"));
  await waitFor(() => expect(f.remove).toHaveBeenCalledTimes(1));
  await act(async () => {
    await Promise.resolve();
  });
  expect(
    f.store
      .getState()
      .tabs.some((tab) => tab.type === "sessions" && tab.id === "same-id"),
  ).toBe(false);
  expect(useUndoDelete.getState().pendingDeletions).toEqual({});
});
