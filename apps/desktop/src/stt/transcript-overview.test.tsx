import {
  QueryClient,
  QueryClientProvider,
  onlineManager,
} from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { StrictMode, useState, type ReactNode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  connect: vi.fn(),
  words: vi.fn(),
  has: vi.fn(),
  metadata: vi.fn(),
  people: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({
  commands: {
    sessionTranscripts: native.words,
    sessionHasTranscript: native.has,
    sessionTranscriptMetadata: native.metadata,
  },
  events: {},
}));
vi.mock("~/shared/application/vault-client", () => ({
  connectLocalVault: native.connect,
}));
vi.mock("~/people/queries", () => ({ usePeople: native.people }));

import { useSessionTranscriptMetadata } from "./queries";
import { useTranscriptOverview } from "./transcript-overview";

import { SessionPeople } from "~/session/components/session-people";
import { useSessionHasTranscript } from "~/session/queries";
import { ApplicationRequestError } from "~/shared/application/errors";
import type { TranscriptOverview } from "~/shared/application/protocol.gen";
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
import type { VaultScope } from "~/shared/application/vault-scope";
import { ScopedTabsProvider } from "~/store/zustand/tabs/scoped-provider";
import { deviceFixture } from "~/store/zustand/tabs/scoped-test-utils";

const overview = (label = "person", has_words = true): TranscriptOverview => ({
  has_words,
  speaker_labels: [label],
  started_at: 10,
  ended_at: 50,
});
function fixture(mode: "always" | "online" = "always", scopeId = "same-scope") {
  let value = overview();
  let people = [{ id: "person", name: "Scoped Person" }];
  const listeners = new Set<(event: VaultClientEvent) => void>();
  const read = vi.fn<VaultClient["transcriptOverview"]>(async () => value);
  const readPeople = vi.fn<VaultClient["people"]>(async () => people);
  const device = deviceFixture().device;
  const client = {
    scope: scopeId,
    networkMode: mode,
    canEdit: false,
    features: ["transcript_overview"],
    transcriptOverview: read,
    people: readPeople,
    sessionSummaries: vi.fn(async () => []),
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
    readPeople,
    device,
    roots: [] as QueryClient[],
    scopes: [] as VaultScope[],
    set: (next: TranscriptOverview) => {
      value = next;
    },
    names: (next: typeof people) => {
      people = next;
    },
    emit: (event: VaultClientEvent) =>
      act(() => {
        for (const listener of listeners) listener(event);
      }),
  };
}
function MainHost({
  backend,
  children,
}: {
  backend: ReturnType<typeof fixture>;
  children: ReactNode;
}) {
  const { scope } = useVaultHost();
  if (!scope) return null;
  if (!backend.scopes.includes(scope)) backend.scopes.push(scope);
  return (
    <ScopedTabsProvider
      scope={scope}
      deviceState={backend.device}
      managePins={false}
    >
      {children}
    </ScopedTabsProvider>
  );
}
function Host({
  backend,
  children,
}: {
  backend: ReturnType<typeof fixture>;
  children: ReactNode;
}) {
  const [root] = useState(() => new QueryClient());
  if (!backend.roots.includes(root)) backend.roots.push(root);
  return (
    <StrictMode>
      <QueryClientProvider client={root}>
        <VaultClientProvider client={backend.client}>
          <MainHost backend={backend}>{children}</MainHost>
        </VaultClientProvider>
      </QueryClientProvider>
    </StrictMode>
  );
}
function Probe({ id = "same" }: { id?: string }) {
  const has = useSessionHasTranscript(id);
  const metadata = useSessionTranscriptMetadata(id);
  return (
    <>
      <output data-testid="overview">{`${has}|${metadata.data?.started_at ?? "none"}|${metadata.data?.ended_at ?? "none"}`}</output>
      <button
        onClick={() => {
          void metadata.refetch();
        }}
      >
        Retry
      </button>
      {metadata.error && <div role="alert">{metadata.error.message}</div>}
      <SessionPeople sessionId={id} />
    </>
  );
}
beforeEach(() => vi.clearAllMocks());
afterEach(() => {
  cleanup();
  onlineManager.setOnline(true);
});

it("deduplicates hasTranscript/duration/pills on the captured metadata key without reading words or local names", async () => {
  const backend = fixture();
  render(
    <Host backend={backend}>
      <Probe />
      <Probe />
    </Host>,
  );
  await screen.findAllByText("Scoped Person");
  expect(
    screen
      .getAllByTestId("overview")
      .every((row) => row.textContent === "true|10|50"),
  ).toBe(true);
  expect(
    backend.read.mock.calls.filter(([, options]) => !options?.signal?.aborted),
  ).toHaveLength(1);
  expect(
    backend.readPeople.mock.calls.filter(
      ([options]) => !options?.signal?.aborted,
    ),
  ).toHaveLength(1);
  expect(backend.read).toHaveBeenCalledWith("same", {
    signal: expect.any(AbortSignal),
  });
  const scope = backend.scopes[backend.scopes.length - 1];
  expect(
    scope.queryClient.getQueryData(
      scope.transcriptOverviewOptions("same").queryKey,
    ),
  ).toEqual(overview());
  expect(
    backend.roots[backend.roots.length - 1].getQueryCache().getAll(),
  ).toHaveLength(0);
  for (const command of [
    native.words,
    native.has,
    native.metadata,
    native.people,
    native.connect,
  ])
    expect(command).not.toHaveBeenCalled();
});

it("revalidates only matching transcript/session/header events, resync and scoped people changes", async () => {
  const backend = fixture();
  render(
    <Host backend={backend}>
      <Probe />
    </Host>,
  );
  await screen.findByText("Scoped Person");
  const before = backend.read.mock.calls.length;
  backend.emit({
    kind: "change",
    change: {
      kind: "index_changed",
      entity: "transcripts",
      ids: ["different"],
    },
  });
  backend.emit({
    kind: "change",
    change: { kind: "index_changed", entity: "people", ids: [] },
  });
  expect(backend.read).toHaveBeenCalledTimes(before);
  backend.names([{ id: "person", name: "Renamed" }]);
  backend.emit({
    kind: "change",
    change: { kind: "index_changed", entity: "people", ids: [] },
  });
  await screen.findByText("Renamed");
  for (const entity of [
    "transcripts",
    "sessions",
    "session_headers",
  ] as const) {
    const count = backend.read.mock.calls.length;
    backend.emit({
      kind: "change",
      change: { kind: "index_changed", entity, ids: ["same"] },
    });
    await waitFor(() => expect(backend.read.mock.calls.length).toBe(count + 1));
  }
  backend.set({
    has_words: false,
    speaker_labels: [],
    started_at: null,
    ended_at: null,
  });
  backend.emit({ kind: "change", change: { kind: "resync_required" } });
  await waitFor(() =>
    expect(screen.getByTestId("overview").textContent).toBe("false|none|none"),
  );
  expect(screen.queryByText("Renamed")).toBeNull();
});

it("preserves raw-label fallback/order and never infers an explicit end or duration", async () => {
  const backend = fixture();
  backend.set({
    ...overview(),
    speaker_labels: ["unknown", "person", "unknown"],
    ended_at: null,
  });
  render(
    <Host backend={backend}>
      <Probe />
    </Host>,
  );
  await screen.findByText("Scoped Person");
  expect(screen.getByTestId("overview").textContent).toBe("true|10|none");
  expect(screen.getAllByText("unknown")).toHaveLength(1);
  expect(
    screen
      .getByText("unknown")
      .compareDocumentPosition(screen.getByText("Scoped Person")) &
      Node.DOCUMENT_POSITION_FOLLOWING,
  ).toBeTruthy();
});

it("preserves standalone local offline/login-free startup", async () => {
  const backend = fixture();
  native.connect.mockResolvedValue(backend.client);
  onlineManager.setOnline(false);
  render(
    <StrictMode>
      <QueryClientProvider client={new QueryClient()}>
        <LocalVaultProvider>
          <MainHost backend={backend}>
            <Probe />
          </MainHost>
        </LocalVaultProvider>
      </QueryClientProvider>
    </StrictMode>,
  );
  await screen.findByText("Scoped Person");
  expect(native.connect).toHaveBeenCalledOnce();
  expect(
    backend.read.mock.calls.filter(([, options]) => !options?.signal?.aborted),
  ).toHaveLength(1);
});

it("pauses remote reads offline, resumes once, and keeps known metadata during event-channel reconnect", async () => {
  const backend = fixture("online");
  onlineManager.setOnline(false);
  render(
    <Host backend={backend}>
      <Probe />
    </Host>,
  );
  await waitFor(() => expect(backend.scopes.length).toBeGreaterThan(0));
  expect(backend.read).not.toHaveBeenCalled();
  act(() => onlineManager.setOnline(true));
  await screen.findByText("Scoped Person");
  backend.emit({ kind: "connection", state: "reconnecting", error: null });
  expect(screen.getByTestId("overview").textContent).toBe("true|10|50");
  backend.set({ ...overview(), ended_at: 70 });
  backend.emit({ kind: "connection", state: "connected", error: null });
  await waitFor(() =>
    expect(screen.getByTestId("overview").textContent).toBe("true|10|70"),
  );
});

it.each(
  [50, 150, 500].flatMap((delay) => [
    { stage: "overview", delay },
    { stage: "people", delay },
  ]),
)(
  "isolates identical IDs/scope strings and late $stage replies at $delay ms through the real host barrier",
  async ({ stage, delay }) => {
    const first = fixture("online");
    if (stage === "overview")
      first.read.mockImplementation(
        () =>
          new Promise((resolve) =>
            setTimeout(() => resolve(overview("PRIVATE OLD")), delay),
          ),
      );
    else
      first.readPeople.mockImplementation(
        () =>
          new Promise((resolve) =>
            setTimeout(
              () => resolve([{ id: "person", name: "PRIVATE OLD NAME" }]),
              delay,
            ),
          ),
      );
    const second = fixture("online");
    second.names([{ id: "person", name: "Replacement Name" }]);
    const content = (backend: typeof first) => (
      <Host backend={backend}>
        <Probe />
      </Host>
    );
    const view = render(content(first));
    await waitFor(() =>
      expect(
        stage === "overview" ? first.read : first.readPeople,
      ).toHaveBeenCalled(),
    );
    const old = first.scopes[first.scopes.length - 1];
    view.rerender(content(second));
    await screen.findByText("Replacement Name");
    await act(
      async () => new Promise((resolve) => setTimeout(resolve, delay + 10)),
    );
    expect(screen.queryByText(/PRIVATE OLD/)).toBeNull();
    expect(old.getSnapshot().state).toBe("closed");
    expect(old.queryClient.getQueryCache().getAll()).toHaveLength(0);
  },
);

it.each(["login_required", "access_denied", "closed", "incompatible"] as const)(
  "latches terminal %s, hides known data and rejects stale retries",
  async (state) => {
    const backend = fixture();
    let retry: (() => unknown) | undefined;
    function Capture() {
      retry = useTranscriptOverview("same").refetch;
      return <Probe />;
    }
    render(
      <Host backend={backend}>
        <Capture />
      </Host>,
    );
    await screen.findByText("Scoped Person");
    const stale = retry!;
    const before = backend.read.mock.calls.length;
    backend.emit({ kind: "connection", state, error: null });
    backend.emit({ kind: "connection", state: "connected", error: null });
    expect(screen.getByTestId("overview").textContent).toBe("false|none|none");
    expect(screen.queryByText("Scoped Person")).toBeNull();
    await act(async () => {
      await stale();
    });
    expect(backend.read).toHaveBeenCalledTimes(before);
  },
);

it("never dispatches a paused read after access loss and discards a pending reply on disposal", async () => {
  const backend = fixture("online");
  onlineManager.setOnline(false);
  const view = render(
    <Host backend={backend}>
      <Probe />
    </Host>,
  );
  await waitFor(() => expect(backend.scopes.length).toBeGreaterThan(0));
  backend.emit({ kind: "connection", state: "access_denied", error: null });
  act(() => onlineManager.setOnline(true));
  expect(backend.read).not.toHaveBeenCalled();
  view.unmount();
  const next = fixture();
  let finish!: (value: TranscriptOverview) => void;
  next.read.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  render(
    <Host backend={next}>
      <Probe />
    </Host>,
  );
  await waitFor(() => expect(next.read).toHaveBeenCalled());
  const scope = next.scopes[next.scopes.length - 1];
  act(() => scope.dispose());
  await act(async () => finish(overview("PRIVATE OLD")));
  expect(screen.queryByText("PRIVATE OLD")).toBeNull();
  expect(
    scope.queryClient
      .getQueryCache()
      .getAll()
      .some((query) => query.state.data),
  ).toBe(false);
});

it("rejects unsupported servers and missing sessions without local fallback and allows explicit retry of a failed read", async () => {
  const unsupported = fixture();
  unsupported.client = { ...unsupported.client, features: [] };
  const view = render(
    <Host backend={unsupported}>
      <Probe />
    </Host>,
  );
  await screen.findByRole("alert");
  expect(unsupported.read).not.toHaveBeenCalled();
  expect(unsupported.readPeople).not.toHaveBeenCalled();
  fireEvent.click(screen.getByText("Retry"));
  expect(unsupported.read).not.toHaveBeenCalled();
  const next = fixture();
  next.read.mockRejectedValue(
    new ApplicationRequestError({
      code: "not_found",
      message: "Session not found",
      retryable: false,
      request_id: null,
      current_revision: null,
      mutation_outcome: null,
    }),
  );
  view.rerender(
    <Host backend={next}>
      <Probe />
    </Host>,
  );
  await screen.findByText("Session not found");
  expect(screen.getByTestId("overview").textContent).toBe("false|none|none");
  next.read.mockResolvedValue(overview());
  fireEvent.click(screen.getByText("Retry"));
  await screen.findByText("Scoped Person");
  expect(native.metadata).not.toHaveBeenCalled();
});

it("fails closed for an unkeyed Scope-object change and requires a provider rather than a fixed local fallback", async () => {
  const first = fixture(),
    second = fixture();
  const { VaultScope } = await import("~/shared/application/vault-scope");
  const old = new VaultScope(first.client),
    next = new VaultScope(second.client);
  await old.start();
  await next.start();
  const content = (scope: VaultScope) => (
    <VaultHostContext.Provider value={{ scope, error: null, retry: () => {} }}>
      <Probe />
    </VaultHostContext.Provider>
  );
  const view = render(content(old));
  try {
    await screen.findByText("Scoped Person");
    view.rerender(content(next));
    expect(screen.queryByText("Scoped Person")).toBeNull();
    expect(second.read).not.toHaveBeenCalled();
  } finally {
    view.unmount();
    old.dispose();
    next.dispose();
  }
  expect(() => render(<Probe />)).toThrow("requires a vault host");
  expect(native.connect).not.toHaveBeenCalled();
});

it("never lets an old session retry refetch the new selection or exposes an empty-ID cache entry", async () => {
  const backend = fixture();
  const retries: Array<() => unknown> = [];
  function Read({ id }: { id: string }) {
    const query = useTranscriptOverview(id);
    retries.push(query.refetch);
    return <output>{query.data?.speaker_labels.join("|") ?? "none"}</output>;
  }
  const content = (id: string) => (
    <Host backend={backend}>
      <Read id={id} />
    </Host>
  );
  const view = render(content("first"));
  await screen.findByText("person");
  const retry = retries[retries.length - 1];
  view.rerender(content("second"));
  await waitFor(() =>
    expect(backend.read.mock.calls.some(([id]) => id === "second")).toBe(true),
  );
  const before = backend.read.mock.calls.length;
  await act(async () => {
    await retry();
  });
  expect(backend.read).toHaveBeenCalledTimes(before);
  const scope = backend.scopes[backend.scopes.length - 1];
  act(() => {
    scope.queryClient.setQueryData(
      scope.transcriptOverviewOptions("").queryKey,
      overview("PRIVATE EMPTY"),
    );
  });
  view.rerender(content(""));
  expect(screen.queryByText("PRIVATE EMPTY")).toBeNull();
  expect(screen.getByText("none")).toBeTruthy();
});

it("does not rerender hasTranscript consumers for unchanged background overview reads", async () => {
  const backend = fixture();
  let renders = 0;
  function Has() {
    const value = useSessionHasTranscript("same");
    renders += 1;
    return <output>{String(value)}</output>;
  }
  render(
    <Host backend={backend}>
      <Has />
    </Host>,
  );
  await screen.findByText("true");
  const before = renders;
  const reads = backend.read.mock.calls.length;
  let finish!: (value: TranscriptOverview) => void;
  backend.read.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  backend.emit({
    kind: "change",
    change: { kind: "index_changed", entity: "transcripts", ids: ["same"] },
  });
  await waitFor(() => expect(backend.read.mock.calls.length).toBe(reads + 1));
  expect(renders).toBe(before);
  await act(async () => finish(structuredClone(overview())));
  expect(renders).toBe(before);
});
