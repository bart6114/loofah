import {
  QueryClient,
  QueryClientProvider,
  onlineManager,
} from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { StrictMode, useState, type ReactNode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const legacy = vi.hoisted(() => ({
  get: vi.fn(),
  connect: vi.fn(),
  note: vi.fn(),
  tasks: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({
  commands: { sessionGet: legacy.get },
  events: {},
}));
vi.mock("~/shared/application/vault-client", () => ({
  connectLocalVault: legacy.connect,
}));

import { SessionAuthorBadge } from "./session-author-badge";
import { SessionDate } from "./session-date";

import { ApplicationRequestError } from "~/shared/application/errors";
import type { MetadataSnapshot } from "~/shared/application/protocol.gen";
import type {
  VaultClient,
  VaultClientEvent,
} from "~/shared/application/vault-client";
import {
  LocalVaultProvider,
  VaultClientProvider,
  VaultHostContext,
  useVaultHost,
} from "~/shared/application/vault-provider";
import { VaultScope } from "~/shared/application/vault-scope";

function snapshot(
  author: string | null = "writer",
  id = "same",
  revision = "r0",
): MetadataSnapshot {
  return {
    meta: {
      id,
      title: "Title",
      created_at: "2026-01-01T10:00:00Z",
      started_at: null,
      ended_at: null,
      tags: [],
      author,
    },
    revision,
  };
}
function fixture(mode: "always" | "online" = "always", scopeId = "same-scope") {
  let current = snapshot();
  const listeners = new Set<(event: VaultClientEvent) => void>();
  const read = vi.fn<VaultClient["readMetadata"]>(async () => current);
  const client = {
    scope: scopeId,
    networkMode: mode,
    canEdit: false,
    features: [],
    readMetadata: read,
    session: legacy.get,
    readNote: legacy.note,
    tasks: legacy.tasks,
    subscribe: async (listener: (event: VaultClientEvent) => void) => {
      listeners.add(listener);
      listener({ kind: "connection", state: "connected", error: null });
      return () => {
        listeners.delete(listener);
      };
    },
  } as unknown as VaultClient;
  return {
    client,
    read,
    scopes: [] as VaultScope[],
    set: (value: MetadataSnapshot) => {
      current = value;
    },
    emit: (event: VaultClientEvent) =>
      act(() => {
        for (const listener of listeners) listener(event);
      }),
  };
}
function Capture({
  backend,
  children,
}: {
  backend: ReturnType<typeof fixture>;
  children: ReactNode;
}) {
  const { scope } = useVaultHost();
  if (scope && !backend.scopes.includes(scope)) backend.scopes.push(scope);
  return children;
}
function Host({
  backend,
  children,
}: {
  backend: ReturnType<typeof fixture>;
  children: ReactNode;
}) {
  const [root] = useState(() => new QueryClient());
  return (
    <StrictMode>
      <QueryClientProvider client={root}>
        <VaultClientProvider client={backend.client}>
          <Capture backend={backend}>{children}</Capture>
        </VaultClientProvider>
      </QueryClientProvider>
    </StrictMode>
  );
}
beforeEach(() => vi.clearAllMocks());
afterEach(() => {
  cleanup();
  onlineManager.setOnline(true);
});

it("uses metadata only, sharing the Date query across repeated reader badges and preserving the author icon/tooltip/styles", async () => {
  const backend = fixture();
  render(
    <Host backend={backend}>
      <SessionDate sessionId="same" />
      <SessionAuthorBadge sessionId="same" className="mt-2" />
      <SessionAuthorBadge sessionId="same" />
    </Host>,
  );
  await waitFor(() => expect(screen.getAllByText("writer")).toHaveLength(2));
  expect(
    backend.read.mock.calls.filter(([, options]) => !options?.signal?.aborted),
  ).toHaveLength(1);
  expect(backend.read.mock.calls.length).toBeLessThanOrEqual(2);
  expect(backend.read).toHaveBeenCalledWith("same", {
    signal: expect.any(AbortSignal),
  });
  const badge = screen.getAllByText("writer")[0];
  expect(badge.title).toBe("Written by writer — not by you");
  expect(badge.querySelector("svg")).not.toBeNull();
  expect(badge.parentElement?.classList.contains("mt-2")).toBe(true);
  const scope = backend.scopes[backend.scopes.length - 1];
  expect(
    scope.queryClient
      .getQueryCache()
      .getAll()
      .map((query) => query.queryKey),
  ).toEqual([["vault", backend.client.scope, "metadata", "same"]]);
  expect(
    scope.queryClient.getQueryData(scope.metadataOptions("same").queryKey),
  ).toEqual(snapshot());
  expect(legacy.get).not.toHaveBeenCalled();
  expect(legacy.note).not.toHaveBeenCalled();
  expect(legacy.tasks).not.toHaveBeenCalled();
});

it.each([null, ""])(
  "hides an owner-authored session with author %s",
  async (author) => {
    const backend = fixture();
    backend.set(snapshot(author));
    const view = render(
      <Host backend={backend}>
        <SessionAuthorBadge sessionId="same" />
      </Host>,
    );
    await waitFor(() => expect(backend.read).toHaveBeenCalled());
    expect(view.container.textContent).toBe("");
  },
);

it("refreshes author changes through scoped metadata invalidation and hides removed authors", async () => {
  const backend = fixture();
  render(
    <Host backend={backend}>
      <SessionAuthorBadge sessionId="same" />
    </Host>,
  );
  await screen.findByText("writer");
  backend.set(snapshot("new writer", "same", "r1"));
  backend.emit({
    kind: "change",
    change: { kind: "index_changed", entity: "session_headers", ids: ["same"] },
  });
  await screen.findByText("new writer");
  backend.set(snapshot(null, "same", "r2"));
  backend.emit({
    kind: "change",
    change: { kind: "index_changed", entity: "sessions", ids: ["same"] },
  });
  await waitFor(() => expect(screen.queryByText("new writer")).toBeNull());
});

it("keeps the default local connector and read available offline without a login flow", async () => {
  const backend = fixture();
  legacy.connect.mockResolvedValue(backend.client);
  onlineManager.setOnline(false);
  render(
    <StrictMode>
      <QueryClientProvider client={new QueryClient()}>
        <LocalVaultProvider>
          <SessionAuthorBadge sessionId="same" />
        </LocalVaultProvider>
      </QueryClientProvider>
    </StrictMode>,
  );
  await screen.findByText("writer");
  expect(legacy.connect).toHaveBeenCalledOnce();
  expect(
    backend.read.mock.calls.filter(([, options]) => !options?.signal?.aborted),
  ).toHaveLength(1);
  expect(legacy.get).not.toHaveBeenCalled();
});

it("pauses new remote reads offline and resumes using the captured Scope", async () => {
  const backend = fixture("online");
  onlineManager.setOnline(false);
  render(
    <Host backend={backend}>
      <SessionAuthorBadge sessionId="same" />
    </Host>,
  );
  await waitFor(() => expect(backend.scopes.length).toBeGreaterThan(0));
  expect(backend.read).not.toHaveBeenCalled();
  act(() => onlineManager.setOnline(true));
  await screen.findByText("writer");
  expect(legacy.connect).not.toHaveBeenCalled();
});

it("retains a known author while reconnecting and revalidates when the captured channel reconnects", async () => {
  const backend = fixture("online");
  render(
    <Host backend={backend}>
      <SessionAuthorBadge sessionId="same" />
    </Host>,
  );
  await screen.findByText("writer");
  onlineManager.setOnline(false);
  backend.emit({ kind: "connection", state: "reconnecting", error: null });
  expect(screen.getByText("writer")).toBeTruthy();
  backend.set(snapshot("after reconnect"));
  backend.emit({ kind: "connection", state: "connected", error: null });
  expect(screen.getByText("writer")).toBeTruthy();
  act(() => onlineManager.setOnline(true));
  await screen.findByText("after reconnect");
});

it.each([50, 150, 500])(
  "isolates identical IDs/scope strings from retired replies delayed %sms",
  async (delay) => {
    const first = fixture("online");
    first.read.mockImplementation(
      () =>
        new Promise((resolve) =>
          setTimeout(() => resolve(snapshot("PRIVATE OLD")), delay),
        ),
    );
    const second = fixture("online");
    second.set(snapshot("Replacement"));
    const content = (backend: typeof first) => (
      <Host backend={backend}>
        <SessionAuthorBadge sessionId="same" />
      </Host>
    );
    const view = render(content(first));
    await waitFor(() => expect(first.read).toHaveBeenCalled());
    const old = first.scopes[first.scopes.length - 1];
    view.rerender(content(second));
    expect(screen.queryByText("PRIVATE OLD")).toBeNull();
    await screen.findByText("Replacement");
    await act(
      async () => new Promise((resolve) => setTimeout(resolve, delay + 10)),
    );
    expect(screen.queryByText("PRIVATE OLD")).toBeNull();
    expect(old.getSnapshot().state).toBe("closed");
    expect(old.queryClient.getQueryCache().getAll()).toHaveLength(0);
    expect(
      first.read.mock.calls.every(([, options]) => options?.signal?.aborted),
    ).toBe(true);
  },
);

it("remounts its query observer when a parent supplies a new Scope with the identical scope string", async () => {
  const first = fixture();
  const second = fixture();
  second.set(snapshot("Replacement"));
  const old = new VaultScope(first.client);
  const next = new VaultScope(second.client);
  await old.start();
  await next.start();
  const root = new QueryClient();
  const content = (scope: VaultScope) => (
    <StrictMode>
      <QueryClientProvider client={root}>
        <VaultHostContext.Provider
          value={{ scope, error: null, retry: () => {} }}
        >
          <SessionAuthorBadge sessionId="same" />
        </VaultHostContext.Provider>
      </QueryClientProvider>
    </StrictMode>
  );
  const view = render(content(old));
  try {
    await screen.findByText("writer");
    view.rerender(content(next));
    expect(screen.queryByText("writer")).toBeNull();
    await screen.findByText("Replacement");
    expect(
      next.queryClient.getQueryData(next.metadataOptions("same").queryKey),
    ).toEqual(snapshot("Replacement"));
    expect(root.getQueryCache().getAll()).toHaveLength(0);
  } finally {
    view.unmount();
    old.dispose();
    next.dispose();
    root.clear();
  }
});

it.each(["login_required", "access_denied", "closed", "incompatible"] as const)(
  "hides and latches %s even if the same Scope later emits connected",
  async (state) => {
    const backend = fixture();
    render(
      <Host backend={backend}>
        <SessionAuthorBadge sessionId="same" />
      </Host>,
    );
    await screen.findByText("writer");
    const before = backend.read.mock.calls.length;
    backend.emit({ kind: "connection", state, error: null });
    backend.emit({ kind: "connection", state: "connected", error: null });
    expect(screen.queryByText("writer")).toBeNull();
    expect(backend.read).toHaveBeenCalledTimes(before);
  },
);

it("discards pending reads after access loss", async () => {
  const backend = fixture();
  let finish!: (value: MetadataSnapshot) => void;
  backend.read.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  render(
    <Host backend={backend}>
      <SessionAuthorBadge sessionId="same" />
    </Host>,
  );
  await waitFor(() => expect(backend.read).toHaveBeenCalled());
  const scope = backend.scopes[backend.scopes.length - 1];
  backend.emit({ kind: "connection", state: "access_denied", error: null });
  await act(async () => finish(snapshot("PRIVATE OLD")));
  expect(screen.queryByText("PRIVATE OLD")).toBeNull();
  expect(
    scope.queryClient
      .getQueryCache()
      .getAll()
      .some((query) => query.state.data),
  ).toBe(false);
  act(() => scope.dispose());
  expect(screen.queryByText("PRIVATE OLD")).toBeNull();
});

it("hides unavailable or wrong-session metadata without inventing an author or using local fallbacks", async () => {
  const backend = fixture();
  backend.read.mockRejectedValue(
    new ApplicationRequestError({
      code: "unavailable",
      message: "Unavailable",
      retryable: false,
      request_id: null,
      current_revision: null,
      mutation_outcome: "not_committed",
    }),
  );
  const view = render(
    <Host backend={backend}>
      <SessionAuthorBadge sessionId="same" />
    </Host>,
  );
  await waitFor(() => expect(backend.read).toHaveBeenCalled());
  expect(view.container.textContent).toBe("");
  const scope = backend.scopes[backend.scopes.length - 1];
  await waitFor(() =>
    expect(
      scope.queryClient.getQueryState(scope.metadataOptions("same").queryKey)
        ?.status,
    ).toBe("error"),
  );
  act(() => {
    scope.queryClient.setQueryData(
      scope.metadataOptions("same").queryKey,
      snapshot("Wrong owner", "different"),
    );
  });
  expect(view.container.textContent).toBe("");
  expect(legacy.get).not.toHaveBeenCalled();
});

it("hides empty session IDs and absent/unready parents without requesting or opening a local vault", () => {
  const view = render(<SessionAuthorBadge sessionId="same" />);
  expect(view.container.textContent).toBe("");
  view.rerender(
    <VaultHostContext.Provider
      value={{ scope: null, error: null, retry: () => {} }}
    >
      <SessionAuthorBadge sessionId="same" />
    </VaultHostContext.Provider>,
  );
  expect(view.container.textContent).toBe("");
  const backend = fixture();
  view.rerender(
    <Host backend={backend}>
      <SessionAuthorBadge sessionId="" />
    </Host>,
  );
  expect(backend.read).not.toHaveBeenCalled();
  expect(legacy.connect).not.toHaveBeenCalled();
  expect(legacy.get).not.toHaveBeenCalled();
});

it("hides a cached author immediately on Scope disposal", async () => {
  const backend = fixture();
  render(
    <Host backend={backend}>
      <SessionAuthorBadge sessionId="same" />
    </Host>,
  );
  await screen.findByText("writer");
  act(() => backend.scopes[backend.scopes.length - 1].dispose());
  expect(screen.queryByText("writer")).toBeNull();
});

it("hides the previous author while a different session's metadata is pending", async () => {
  const backend = fixture();
  const content = (id: string) => (
    <Host backend={backend}>
      <SessionAuthorBadge sessionId={id} />
    </Host>
  );
  const view = render(content("same"));
  await screen.findByText("writer");
  let finish!: (value: MetadataSnapshot) => void;
  backend.read.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  view.rerender(content("different"));
  expect(screen.queryByText("writer")).toBeNull();
  await waitFor(() =>
    expect(backend.read.mock.calls.some(([id]) => id === "different")).toBe(
      true,
    ),
  );
  await act(async () => finish(snapshot("other writer", "different")));
  await screen.findByText("other writer");
});
