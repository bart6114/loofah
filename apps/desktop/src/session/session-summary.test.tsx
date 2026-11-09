import {
  QueryClient,
  QueryClientProvider,
  onlineManager,
} from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { StrictMode, useState, type ReactNode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  connectLocal: vi.fn(),
  sessionGet: vi.fn(),
  openCurrent: vi.fn(),
  openNew: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({
  commands: { sessionGet: native.sessionGet },
  events: { indexChanged: { listen: vi.fn() } },
}));
vi.mock("~/shared/application/vault-client", () => ({
  connectLocalVault: native.connectLocal,
}));
vi.mock("@handlewithcare/react-prosemirror", () => ({
  useEditorEventCallback: () => () => {},
}));
vi.mock("@hypr/editor/note", () => ({
  useLinkedItemOpenBehavior: () => "current",
}));
vi.mock("~/stt/contexts", () => ({
  useListener: (
    select: (state: { live: { sessionId: null; status: string } }) => unknown,
  ) => select({ live: { sessionId: null, status: "inactive" } }),
}));
vi.mock("~/store/zustand/tabs", () => ({
  useTabs: (
    select: (state: {
      openCurrent: typeof native.openCurrent;
      openNew: typeof native.openNew;
    }) => unknown,
  ) => select({ openCurrent: native.openCurrent, openNew: native.openNew }),
}));

import { useSessionSummary } from "./queries";

import { SessionNodeView } from "~/editor-bridge/session-view";
import type { SessionSummary } from "~/shared/application/protocol.gen";
import type {
  VaultClient,
  VaultClientEvent,
} from "~/shared/application/vault-client";
import {
  LocalVaultProvider,
  VaultClientProvider,
  useVaultHost,
} from "~/shared/application/vault-provider";
import type { VaultScope } from "~/shared/application/vault-scope";

beforeEach(() => vi.clearAllMocks());
afterEach(() => {
  cleanup();
  onlineManager.setOnline(true);
});

function fixture(
  scopeId = "local:header",
  networkMode: "always" | "online" = "always",
) {
  let rows: SessionSummary[] = [
    { id: "same-note", title: "Original", created_at: "2026-11-05T10:00:00Z" },
  ];
  const listeners = new Set<(event: VaultClientEvent) => void>();
  const reads = vi.fn<VaultClient["sessionSummaries"]>(async (input) =>
    input.kind === "ids"
      ? rows.filter((row) => input.ids.includes(row.id))
      : [],
  );
  const session = vi.fn();
  const readNote = vi.fn();
  const readNoteEditor = vi.fn();
  const tasks = vi.fn();
  const client = {
    scope: scopeId,
    networkMode,
    canEdit: true,
    features: ["session_summaries"],
    sessionSummaries: reads,
    session,
    readNote,
    readNoteEditor,
    tasks,
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
    reads,
    session,
    readNote,
    readNoteEditor,
    tasks,
    scopes: [] as VaultScope[],
    set: (next: SessionSummary[]) => {
      rows = next;
    },
    emit: (event: VaultClientEvent) =>
      act(() => {
        for (const listener of listeners) listener(event);
      }),
  };
}
function Gate({
  children,
  scopes,
}: {
  children: ReactNode;
  scopes: VaultScope[];
}) {
  const { scope } = useVaultHost();
  if (!scope) return null;
  if (!scopes.includes(scope)) scopes.push(scope);
  return children;
}
function Host({
  backend,
  children,
}: {
  backend: ReturnType<typeof fixture>;
  children: ReactNode;
}) {
  const [rootClient] = useState(() => new QueryClient());
  return (
    <StrictMode>
      <QueryClientProvider client={rootClient}>
        <VaultClientProvider client={backend.client}>
          <Gate scopes={backend.scopes}>{children}</Gate>
        </VaultClientProvider>
      </QueryClientProvider>
    </StrictMode>
  );
}
function Header({
  id = "same-note",
  name = "header",
}: {
  id?: string;
  name?: string;
}) {
  const value = useSessionSummary(id);
  return (
    <output data-testid={name}>
      {value ? `${value.title}|${value.created_at}` : "missing"}
    </output>
  );
}

it("uses one bounded ID query for repeated linked rows without fetching or hydrating note bodies", async () => {
  const backend = fixture();
  const row = (slot: number) => (
    <SessionNodeView
      key={slot}
      nodeProps={
        {
          node: {
            attrs: { sessionId: "same-note", status: "todo", checked: false },
          },
          getPos: () => slot,
        } as unknown as React.ComponentProps<
          typeof SessionNodeView
        >["nodeProps"]
      }
    >
      Linked note
    </SessionNodeView>
  );
  const mounted = render(
    <Host backend={backend}>
      <Header />
      {Array.from({ length: 12 }, (_, i) => row(i))}
    </Host>,
  );
  await screen.findByText("Original|2026-11-05T10:00:00Z");
  expect(mounted.container.querySelectorAll("[data-session-row]")).toHaveLength(
    12,
  );
  expect(backend.reads).toHaveBeenCalledOnce();
  expect(backend.reads).toHaveBeenCalledWith(
    { kind: "ids", ids: ["same-note"] },
    { signal: expect.any(AbortSignal) },
  );
  expect(native.sessionGet).not.toHaveBeenCalled();
  for (const read of [
    backend.session,
    backend.readNote,
    backend.readNoteEditor,
    backend.tasks,
  ])
    expect(read).not.toHaveBeenCalled();
  const scope = backend.scopes[backend.scopes.length - 1];
  expect(
    scope.queryClient
      .getQueryCache()
      .getAll()
      .map((query) => query.queryKey),
  ).toEqual([
    [
      "vault",
      backend.client.scope,
      "session_summaries",
      { kind: "ids", ids: ["same-note"] },
    ],
  ]);
});

it("refreshes scoped header changes and removes a deleted note without using the legacy event bus", async () => {
  const backend = fixture();
  render(
    <Host backend={backend}>
      <Header />
    </Host>,
  );
  await screen.findByText("Original|2026-11-05T10:00:00Z");
  backend.set([
    { id: "same-note", title: "New title", created_at: "2026-11-06T11:00:00Z" },
  ]);
  backend.emit({
    kind: "change",
    change: {
      kind: "index_changed",
      entity: "session_headers",
      ids: ["same-note"],
    },
  });
  await screen.findByText("New title|2026-11-06T11:00:00Z");
  backend.set([]);
  backend.emit({
    kind: "change",
    change: { kind: "index_changed", entity: "sessions", ids: ["same-note"] },
  });
  await screen.findByText("missing");
  expect(native.sessionGet).not.toHaveBeenCalled();
});

it("keeps the default local connector and header reads available offline without a login flow", async () => {
  const backend = fixture();
  native.connectLocal.mockResolvedValue(backend.client);
  onlineManager.setOnline(false);
  render(
    <StrictMode>
      <QueryClientProvider client={new QueryClient()}>
        <LocalVaultProvider>
          <Gate scopes={backend.scopes}>
            <Header />
          </Gate>
        </LocalVaultProvider>
      </QueryClientProvider>
    </StrictMode>,
  );
  await screen.findByText("Original|2026-11-05T10:00:00Z");
  expect(native.connectLocal).toHaveBeenCalledOnce();
  expect(
    backend.reads.mock.calls.filter(([, options]) => !options?.signal?.aborted),
  ).toHaveLength(1);
  expect(backend.reads.mock.calls.length).toBeLessThanOrEqual(2);
  expect(native.sessionGet).not.toHaveBeenCalled();
});

it("does not send a remote read while browser networking is offline", async () => {
  const backend = fixture("remote:header", "online");
  onlineManager.setOnline(false);
  render(
    <Host backend={backend}>
      <Header />
    </Host>,
  );
  await waitFor(() => expect(backend.scopes.length).toBeGreaterThan(0));
  expect(screen.getByTestId("header").textContent).toBe("missing");
  expect(backend.reads).not.toHaveBeenCalled();
  act(() => onlineManager.setOnline(true));
  await screen.findByText("Original|2026-11-05T10:00:00Z");
});

it.each(["empty", "unsupported"])(
  "skips %s requests without any fallback",
  async (kind) => {
    const backend = fixture();
    if (kind === "unsupported")
      backend.client = { ...backend.client, features: [] };
    render(
      <Host backend={backend}>
        <Header id={kind === "empty" ? "" : "same-note"} />
      </Host>,
    );
    await waitFor(() => expect(backend.scopes.length).toBeGreaterThan(0));
    expect(screen.getByTestId("header").textContent).toBe("missing");
    expect(backend.reads).not.toHaveBeenCalled();
    expect(native.sessionGet).not.toHaveBeenCalled();
  },
);

it.each([50, 150, 500])(
  "isolates identical IDs and scope strings when a retired read returns after %sms",
  async (delay) => {
    const first = fixture("same-scope", "online");
    first.reads.mockImplementation(
      () =>
        new Promise((resolve) =>
          setTimeout(
            () =>
              resolve([
                {
                  id: "same-note",
                  title: "PRIVATE OLD TITLE",
                  created_at: "old",
                },
              ]),
            delay,
          ),
        ),
    );
    const second = fixture("same-scope", "online");
    second.set([{ id: "same-note", title: "Replacement", created_at: "new" }]);
    const mounted = render(
      <Host backend={first}>
        <Header />
      </Host>,
    );
    await waitFor(() => expect(first.reads).toHaveBeenCalled());
    const retired = first.scopes[first.scopes.length - 1];
    mounted.rerender(
      <Host backend={second}>
        <Header />
      </Host>,
    );
    await screen.findByText("Replacement|new");
    await act(
      async () => new Promise((resolve) => setTimeout(resolve, delay + 10)),
    );
    expect(screen.queryByText("PRIVATE OLD TITLE|old")).toBeNull();
    expect(retired.getSnapshot().state).toBe("closed");
    expect(retired.queryClient.getQueryCache().getAll()).toHaveLength(0);
    expect(
      first.reads.mock.calls.every(([, options]) => options?.signal?.aborted),
    ).toBe(true);
    expect(
      second.reads.mock.calls.filter(
        ([, options]) => !options?.signal?.aborted,
      ),
    ).toHaveLength(1);
    expect(second.reads.mock.calls.length).toBeLessThanOrEqual(2);
  },
);

it.each(["access_denied", "login_required", "closed", "incompatible"] as const)(
  "immediately hides cached private headers on %s",
  async (state) => {
    const backend = fixture("remote:header", "online");
    render(
      <Host backend={backend}>
        <Header />
      </Host>,
    );
    await screen.findByText("Original|2026-11-05T10:00:00Z");
    const before = backend.reads.mock.calls.length;
    backend.emit({ kind: "connection", state, error: null });
    expect(screen.getByTestId("header").textContent).toBe("missing");
    expect(backend.reads).toHaveBeenCalledTimes(before);
  },
);

it("hides stale headers while reconnecting and revalidates the captured scope on reconnect", async () => {
  const backend = fixture();
  render(
    <Host backend={backend}>
      <Header />
    </Host>,
  );
  await screen.findByText("Original|2026-11-05T10:00:00Z");
  backend.emit({ kind: "connection", state: "reconnecting", error: null });
  expect(screen.getByTestId("header").textContent).toBe("missing");
  backend.set([{ id: "same-note", title: "Reconnected", created_at: "new" }]);
  backend.emit({ kind: "connection", state: "connected", error: null });
  await screen.findByText("Reconnected|new");
});

it("refuses a missing vault provider instead of consulting local commands", () => {
  expect(() => render(<Header />)).toThrow("requires a vault host");
  expect(native.sessionGet).not.toHaveBeenCalled();
  expect(native.connectLocal).not.toHaveBeenCalled();
});
