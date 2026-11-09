import { onlineManager } from "@tanstack/react-query";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { StrictMode, type ReactNode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  legacyCreate: vi.fn(),
  loading: vi.fn(),
  error: vi.fn(),
  dismiss: vi.fn(),
  commands: {
    applicationScope: vi.fn(),
    applicationProcessingAvailable: vi.fn(),
    applicationTranscriptionAvailable: vi.fn(),
    applicationRecordingProcessingAvailable: vi.fn(),
    applicationLocalMediaAvailable: vi.fn(),
    applicationCreateSession: vi.fn(),
  },
  listen: vi.fn(),
}));
vi.mock("~/session/queries", () => ({ createSession: mocks.legacyCreate }));
vi.mock("~/types/tauri.gen", () => ({
  commands: mocks.commands,
  events: { applicationChanged: { listen: mocks.listen } },
}));
vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: {
    loading: mocks.loading,
    error: mocks.error,
    dismiss: mocks.dismiss,
  },
}));
vi.mock("~/store/zustand/listener/instance", () => ({
  listenerStore: {
    getState: () => ({ live: { sessionId: null, status: "idle" } }),
  },
}));

import { useNewNote } from "./useNewNote";

import { ApplicationRequestError } from "~/shared/application/errors";
import type { SessionDocument } from "~/shared/application/protocol.gen";
import type { NativeRemoteConnection } from "~/shared/application/remote";
import {
  connectLocalVault,
  connectRemoteVault,
  type VaultClient,
  type VaultClientEvent,
} from "~/shared/application/vault-client";
import { VaultHostContext } from "~/shared/application/vault-provider";
import { VaultScope } from "~/shared/application/vault-scope";
import { TabsStoreProvider } from "~/store/zustand/tabs/scoped-provider";
import {
  createTabsStore,
  type ScopedTabsStore,
} from "~/store/zustand/tabs/store";

const scopes: VaultScope[] = [];
beforeEach(() => vi.clearAllMocks());
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
  onlineManager.setOnline(true);
  vi.useRealTimers();
});

function session(id = "same-note"): SessionDocument {
  return {
    meta: {
      id,
      title: "",
      created_at: "2026-11-08",
      started_at: null,
      ended_at: null,
      tags: [],
    },
    note: { title: "", markdown: "", revision: "r" },
  };
}
function failure(outcome: "unknown" | "not_committed" = "unknown") {
  return new ApplicationRequestError({
    code: "unavailable",
    message: "Reply lost",
    request_id: null,
    retryable: true,
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
async function fixture(
  networkMode: "always" | "online" = "always",
  options: { canEdit?: boolean; features?: string[]; scope?: string } = {},
) {
  let receive!: (event: VaultClientEvent) => void;
  const create = vi
    .fn<VaultClient["createSession"]>()
    .mockResolvedValue(session());
  const client = {
    scope: options.scope ?? "same-scope",
    networkMode,
    canEdit: options.canEdit ?? true,
    features: options.features ?? ["notes"],
    createSession: create,
    subscribe: async (listener: (event: VaultClientEvent) => void) => {
      receive = listener;
      listener({ kind: "connection", state: "connected", error: null });
      return () => {};
    },
  } as unknown as VaultClient;
  return {
    ...(await host(client)),
    create,
    emit: (state: Parameters<typeof connection>[0]) =>
      act(() => receive(connection(state))),
  };
}
function connection(
  state: "connected" | "reconnecting" | "access_denied" | "login_required",
) {
  return { kind: "connection" as const, state, error: null };
}
async function host(client: VaultClient) {
  const scope = new VaultScope(client);
  scopes.push(scope);
  await scope.start();
  const store = createTabsStore(
    client.scope,
    () =>
      !["closed", "access_denied", "login_required"].includes(
        scope.getSnapshot().state,
      ),
  );
  store.getState().openCurrent({ type: "empty" });
  return { scope, store };
}
function Host({
  scope,
  store,
  children,
}: {
  scope: VaultScope;
  store: ScopedTabsStore;
  children: ReactNode;
}) {
  return (
    <StrictMode>
      <VaultHostContext.Provider value={{ scope, error: null, retry() {} }}>
        <TabsStoreProvider store={store}>{children}</TabsStoreProvider>
      </VaultHostContext.Provider>
    </StrictMode>
  );
}
function mount(
  f: { scope: VaultScope; store: ScopedTabsStore },
  behavior: "new" | "current" = "new",
) {
  return renderHook(
    () => ({
      create: useNewNote({ behavior }),
      keyboard: useNewNote({ behavior: "current" }),
    }),
    { wrapper: ({ children }) => <Host {...f}>{children}</Host> },
  );
}
function retry() {
  const options =
    mocks.error.mock.calls[mocks.error.mock.calls.length - 1]?.[1];
  expect(options?.action?.label).toBe("Retry");
  const preventDefault = vi.fn();
  act(() => {
    options.action.onClick({ preventDefault });
  });
  expect(preventDefault).toHaveBeenCalledOnce();
}

it.each(["always", "online"] as const)(
  "creates through the captured %s client with current/new tab behavior and no local fallback",
  async (mode) => {
    const f = await fixture(mode);
    const originalSlot = f.store.getState().currentTab!.slotId;
    const hook = mount(f, "current");
    act(() => hook.result.current.create());
    expect(mocks.loading).toHaveBeenCalledOnce();
    await waitFor(() =>
      expect(f.store.getState().currentTab).toMatchObject({
        type: "sessions",
        id: "same-note",
        slotId: originalSlot,
      }),
    );
    expect(f.create).toHaveBeenCalledWith(
      { title: "", markdown: "" },
      { requestId: expect.any(String) },
    );
    act(() => hook.result.current.create());
    await waitFor(() => expect(f.create).toHaveBeenCalledTimes(2));
    expect(f.scope.queryClient.getQueryCache().getAll()).toHaveLength(0);
    expect(mocks.legacyCreate).not.toHaveBeenCalled();
  },
);

it("opens a new tab normally and preserves the original slot/history", async () => {
  const f = await fixture();
  const original = f.store.getState().currentTab;
  const hook = mount(f);
  act(() => hook.result.current.create());
  await waitFor(() => expect(f.store.getState().tabs).toHaveLength(2));
  expect(f.store.getState().tabs[0].slotId).toBe(original!.slotId);
  expect(f.store.getState().currentTab!.slotId).not.toBe(original!.slotId);
});

it("keeps connected local creation offline and never asks for login", async () => {
  onlineManager.setOnline(false);
  const f = await fixture("always");
  const hook = mount(f);
  act(() => hook.result.current.create());
  await waitFor(() =>
    expect(f.store.getState().currentTab?.type).toBe("sessions"),
  );
  expect(f.create).toHaveBeenCalledOnce();
  expect(mocks.error).not.toHaveBeenCalled();
});

it.each(["reader", "unsupported", "disconnected", "remote-offline"])(
  "refuses %s before creating or queueing a request",
  async (reason) => {
    const f = await fixture(reason === "remote-offline" ? "online" : "always", {
      canEdit: reason !== "reader",
      features: reason === "unsupported" ? [] : ["notes"],
    });
    if (reason === "disconnected") f.emit("reconnecting");
    if (reason === "remote-offline") onlineManager.setOnline(false);
    const hook = mount(f);
    act(() => hook.result.current.create());
    expect(f.create).not.toHaveBeenCalled();
    expect(f.scope.queryClient.getMutationCache().getAll()).toHaveLength(0);
    expect(mocks.loading).not.toHaveBeenCalled();
  },
);

it.each([50, 150, 500])(
  "deduplicates keyboard/toolbar attempts and preserves unrelated active content during %sms latency",
  async (delay) => {
    const f = await fixture("online");
    f.create.mockImplementation(
      () =>
        new Promise((resolve) => setTimeout(() => resolve(session()), delay)),
    );
    const hook = mount(f, "current");
    vi.useFakeTimers();
    act(() => {
      hook.result.current.create();
      hook.result.current.keyboard();
      hook.result.current.create();
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(delay / 2);
    });
    act(() => f.store.getState().openCurrent({ type: "settings" }));
    const settingsSlot = f.store.getState().currentTab!.slotId;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(delay / 2);
    });
    expect(f.create).toHaveBeenCalledOnce();
    expect(f.store.getState().tabs).toHaveLength(2);
    expect(f.store.getState().tabs[0]).toMatchObject({
      type: "settings",
      slotId: settingsSlot,
    });
    expect(f.store.getState().currentTab).toMatchObject({
      type: "sessions",
      id: "same-note",
    });
  },
);

it("never replaces a closed then recreated destination slot", async () => {
  const f = await fixture();
  const pending = deferred<SessionDocument>();
  f.create.mockReturnValue(pending.promise);
  const original = f.store.getState().currentTab!;
  const hook = mount(f, "current");
  act(() => hook.result.current.create());
  await waitFor(() => expect(f.create).toHaveBeenCalledOnce());
  act(() => {
    f.store.getState().close(original);
    f.store.setState({ tabs: [original], currentTab: original });
  });
  await act(async () => pending.resolve(session()));
  expect(f.store.getState().tabs).toHaveLength(2);
  expect(f.store.getState().tabs[0]).toMatchObject({
    slotId: original.slotId,
    type: original.type,
  });
});

it("retains one unknown UUID beyond five minutes and requires explicit replay across hook instances", async () => {
  const f = await fixture("online");
  f.create.mockRejectedValueOnce(failure());
  const hook = mount(f);
  act(() => hook.result.current.create());
  await waitFor(() => expect(mocks.error).toHaveBeenCalledOnce());
  const requestId = f.create.mock.calls[0][1]!.requestId;
  hook.unmount();
  vi.useFakeTimers();
  await act(async () => vi.advanceTimersByTimeAsync(6 * 60 * 1000));
  const replacement = mount(f, "current");
  for (let i = 0; i < 12; i++) act(() => replacement.result.current.keyboard());
  expect(f.create).toHaveBeenCalledOnce();
  expect(f.scope.queryClient.getMutationCache().getAll()).toHaveLength(1);
  retry();
  await act(async () => vi.advanceTimersByTimeAsync(0));
  expect(f.create).toHaveBeenCalledTimes(2);
  expect(f.create.mock.calls[1][1]!.requestId).toBe(requestId);
  expect(f.store.getState().tabs).toHaveLength(2);
  expect(f.scope.queryClient.getMutationCache().getAll()).toHaveLength(0);
});

it("keeps the original unknown intent when a replay is definitely refused", async () => {
  const f = await fixture();
  f.create
    .mockRejectedValueOnce(failure())
    .mockRejectedValueOnce(failure("not_committed"));
  const hook = mount(f);
  act(() => hook.result.current.create());
  await waitFor(() => expect(mocks.error).toHaveBeenCalledOnce());
  retry();
  await waitFor(() => expect(f.create).toHaveBeenCalledTimes(2));
  await waitFor(() => expect(mocks.error).toHaveBeenCalledTimes(2));
  act(() => hook.result.current.keyboard());
  expect(f.create).toHaveBeenCalledTimes(2);
  retry();
  await waitFor(() =>
    expect(f.store.getState().currentTab?.type).toBe("sessions"),
  );
  expect(
    new Set(f.create.mock.calls.map((call) => call[1]!.requestId)).size,
  ).toBe(1);
});

it("does not automatically replay unknown errors even when query defaults request retries", async () => {
  const f = await fixture();
  f.scope.queryClient.setDefaultOptions({ mutations: { retry: 4 } });
  f.create.mockRejectedValue(failure());
  const hook = mount(f);
  act(() => hook.result.current.create());
  await waitFor(() => expect(mocks.error).toHaveBeenCalledOnce());
  expect(f.create).toHaveBeenCalledOnce();
  hook.unmount();
  const next = mount(f);
  act(() => next.result.current.create());
  expect(f.create).toHaveBeenCalledOnce();
  retry();
  await waitFor(() => expect(f.create).toHaveBeenCalledTimes(2));
});

it.each(["access_denied", "login_required"] as const)(
  "suppresses late success/cache effects after %s even if connection returns",
  async (state) => {
    const f = await fixture();
    const pending = deferred<SessionDocument>();
    f.create.mockReturnValue(pending.promise);
    const invalidate = vi.spyOn(f.scope.queryClient, "invalidateQueries");
    const hook = mount(f);
    act(() => hook.result.current.create());
    await waitFor(() => expect(f.create).toHaveBeenCalledOnce());
    f.emit(state);
    f.emit("connected");
    invalidate.mockClear();
    await act(async () => pending.resolve(session()));
    expect(f.store.getState().currentTab?.type).toBe("empty");
    expect(invalidate).not.toHaveBeenCalled();
    expect(f.store.getState().recentlyOpenedSessionIds).toEqual([]);
  },
);

it("keeps identical IDs isolated across replacement scopes and captured stores", async () => {
  const first = await fixture();
  const next = await fixture();
  const pending = deferred<SessionDocument>();
  first.create.mockReturnValue(pending.promise);
  const old = mount(first);
  act(() => old.result.current.create());
  await waitFor(() => expect(first.create).toHaveBeenCalledOnce());
  const stale = old.result.current.create;
  old.unmount();
  first.scope.dispose();
  const current = mount(next);
  act(() => {
    stale();
    current.result.current.create();
  });
  await waitFor(() =>
    expect(next.store.getState().currentTab?.type).toBe("sessions"),
  );
  await act(async () => pending.resolve(session()));
  expect(first.store.getState().recentlyOpenedSessionIds).toEqual([]);
  expect(next.store.getState().tabs).toHaveLength(2);
  expect(first.create).toHaveBeenCalledOnce();
});

it("only invalidates captured collection caches and never admits historical creation content", async () => {
  const f = await fixture();
  const other = ["vault", "other", "session_page"];
  const sessionKey = f.scope.sessionOptions("same-note").queryKey;
  const list = [
    "vault",
    f.scope.client.scope,
    "session_summaries",
    { kind: "list", limit: 10 },
  ];
  f.scope.queryClient.setQueryData(list, []);
  f.scope.queryClient.setQueryData(other, ["other"]);
  const canonical = {
    ...session(),
    note: { title: "Newer", markdown: "Newer content", revision: "r2" },
  };
  f.scope.queryClient.setQueryData(sessionKey, canonical);
  const hook = mount(f);
  act(() => hook.result.current.create());
  await waitFor(() =>
    expect(f.store.getState().currentTab?.type).toBe("sessions"),
  );
  expect(f.scope.queryClient.getQueryState(list)?.isInvalidated).toBe(true);
  expect(f.scope.queryClient.getQueryState(other)?.isInvalidated).toBe(false);
  expect(f.scope.queryClient.getQueryData(sessionKey)).toEqual(canonical);
});

it.each(["ipc", "https"] as const)(
  "uses the actual %s creation adapter without the legacy writer",
  async (mode) => {
    let client: VaultClient;
    const request = vi.fn().mockResolvedValue(session());
    if (mode === "ipc") {
      mocks.commands.applicationScope.mockResolvedValue({
        status: "ok",
        data: "local:fixture",
      });
      mocks.commands.applicationProcessingAvailable.mockResolvedValue(false);
      mocks.commands.applicationLocalMediaAvailable.mockResolvedValue(false);
      mocks.commands.applicationTranscriptionAvailable.mockResolvedValue(false);
      mocks.commands.applicationRecordingProcessingAvailable.mockResolvedValue(
        false,
      );
      mocks.commands.applicationCreateSession.mockResolvedValue({
        status: "ok",
        data: session(),
      });
      mocks.listen.mockResolvedValue(() => {});
      client = await connectLocalVault();
      onlineManager.setOnline(false);
    } else {
      client = connectRemoteVault(
        {
          info: {
            server: "https://example.com",
            capabilities: { features: ["notes"] },
          },
          request,
          subscribe: async (
            _id: string,
            receive: (event: VaultClientEvent) => void,
          ) => {
            receive(connection("connected"));
            return () => {};
          },
        } as unknown as NativeRemoteConnection,
        "member",
        { id: "vault", role: "editor" },
      );
    }
    const f = await host(client);
    const hook = mount(f);
    act(() => hook.result.current.create());
    await waitFor(() =>
      expect(f.store.getState().currentTab?.type).toBe("sessions"),
    );
    if (mode === "ipc")
      expect(mocks.commands.applicationCreateSession).toHaveBeenCalledWith(
        expect.any(String),
        { title: "", markdown: "" },
      );
    else
      expect(request).toHaveBeenCalledWith(
        {
          kind: "create_session",
          vault_id: "vault",
          input: { title: "", markdown: "" },
        },
        "session",
        { requestId: expect.any(String) },
      );
    expect(mocks.legacyCreate).not.toHaveBeenCalled();
  },
);
