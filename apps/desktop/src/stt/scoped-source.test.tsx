import {
  QueryClient,
  QueryClientProvider,
  onlineManager,
} from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { StrictMode, useState, type ReactNode } from "react";
import { afterEach, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  sessionTranscripts: vi.fn(),
  transcriptGet: vi.fn(),
  peopleList: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({
  commands: native,
  events: { indexChanged: { listen: vi.fn() } },
}));

import {
  useSessionTranscriptsQuery,
  useTranscript,
  useTranscriptLabelContext,
} from "./queries";

import { usePeople } from "~/people/queries";
import type {
  PersonItem,
  TranscriptWithData,
} from "~/shared/application/protocol.gen";
import type {
  VaultClient,
  VaultClientEvent,
} from "~/shared/application/vault-client";
import {
  useVaultHost,
  VaultClientProvider,
} from "~/shared/application/vault-provider";
import type { VaultScope } from "~/shared/application/vault-scope";

function fixture(
  mode: "always" | "online" = "always",
  scopeId: string = crypto.randomUUID(),
) {
  const listeners = new Set<(event: VaultClientEvent) => void>();
  const rows = new Map<string, TranscriptWithData[]>();
  const transcript = (sessionId: string, text: string): TranscriptWithData => ({
    id: "same-transcript",
    session_id: sessionId,
    user_id: "self",
    started_at: 50,
    ended_at: 500,
    words: [
      {
        id: "same-word",
        text,
        start_ms: 0,
        end_ms: 100,
        channel: 1,
        metadata: { language: "nl", speaker_index: 2 },
      },
    ],
    speaker_hints: [
      {
        id: "hint",
        word_id: "same-word",
        type: "speaker_label",
        value: "same-person",
      },
    ],
  });
  rows.set("same-session", [transcript("same-session", "Original words")]);
  rows.set("other-session", [
    transcript("other-session", "Other session words"),
  ]);
  let people: PersonItem[] = [{ id: "same-person", name: "Original person" }];
  const read = vi.fn<VaultClient["transcripts"]>(async (id) =>
    structuredClone(rows.get(id) ?? []),
  );
  const peopleRead = vi.fn<VaultClient["people"]>(async () =>
    structuredClone(people),
  );
  const client = {
    scope: scopeId,
    networkMode: mode,
    canEdit: true,
    features: ["content_read"],
    transcripts: read,
    people: peopleRead,
    subscribe: async (changed: (event: VaultClientEvent) => void) => {
      listeners.add(changed);
      changed({ kind: "connection", state: "connected", error: null });
      return () => {
        listeners.delete(changed);
      };
    },
  } as unknown as VaultClient;
  return {
    client,
    read,
    peopleRead,
    scopes: [] as VaultScope[],
    rows,
    transcript,
    setPeople: (next: PersonItem[]) => {
      people = next;
    },
    emit: (event: VaultClientEvent) => {
      for (const changed of listeners) changed(event);
    },
  };
}
function Gate({
  backend,
  children,
}: {
  backend: ReturnType<typeof fixture>;
  children: ReactNode;
}) {
  const { scope } = useVaultHost();
  if (!scope) return null;
  if (!backend.scopes.includes(scope)) backend.scopes.push(scope);
  return children;
}
function Host({
  backend,
  children,
}: {
  backend: ReturnType<typeof fixture>;
  children: ReactNode;
}) {
  const [client] = useState(() => new QueryClient());
  return (
    <StrictMode>
      <QueryClientProvider client={client}>
        <VaultClientProvider client={backend.client}>
          <Gate backend={backend}>{children}</Gate>
        </VaultClientProvider>
      </QueryClientProvider>
    </StrictMode>
  );
}
function Reader({
  id = "same-session",
  name = "source",
  remember,
}: {
  id?: string;
  name?: string;
  remember?: (read: () => unknown) => void;
}) {
  const query = useSessionTranscriptsQuery(id);
  const one = useTranscript(id, "same-transcript");
  const context = useTranscriptLabelContext(id, "same-transcript");
  const people = usePeople();
  remember?.(query.refetch);
  return (
    <>
      <output data-testid={name}>
        {JSON.stringify({
          rows: query.data,
          one,
          name: context?.getHumanName("same-person"),
          people,
        })}
      </output>
      <output data-testid={`${name}-error`}>
        {query.error?.message ?? ""}
      </output>
    </>
  );
}
afterEach(() => {
  cleanup();
  onlineManager.setOnline(true);
  vi.clearAllMocks();
});
it.each(["always", "online"] as const)(
  "deduplicates %s session/person sources and never uses legacy bare-ID commands",
  async (mode) => {
    const backend = fixture(mode);
    render(
      <Host backend={backend}>
        <Reader />
        <Reader name="second" />
      </Host>,
    );
    await waitFor(() =>
      expect(screen.getByTestId("source").textContent).toContain(
        "Original person",
      ),
    );
    expect(screen.getByTestId("source").textContent).toContain(
      "Original words",
    );
    expect(backend.read).toHaveBeenCalledOnce();
    expect(backend.peopleRead).toHaveBeenCalledOnce();
    expect(backend.read).toHaveBeenCalledWith("same-session", {
      signal: expect.any(AbortSignal),
    });
    expect(native.sessionTranscripts).not.toHaveBeenCalled();
    expect(native.transcriptGet).not.toHaveBeenCalled();
    expect(native.peopleList).not.toHaveBeenCalled();
  },
);
it("preserves local offline reads and pauses remote reads until online", async () => {
  onlineManager.setOnline(false);
  const local = fixture();
  const mounted = render(
    <Host backend={local}>
      <Reader />
    </Host>,
  );
  await waitFor(() =>
    expect(screen.getByTestId("source").textContent).toContain(
      "Original words",
    ),
  );
  const remote = fixture("online");
  mounted.rerender(
    <Host backend={remote}>
      <Reader />
    </Host>,
  );
  await waitFor(() =>
    expect(screen.getByTestId("source").textContent).not.toContain(
      "Original words",
    ),
  );
  expect(remote.read).not.toHaveBeenCalled();
  expect(remote.peopleRead).not.toHaveBeenCalled();
  act(() => onlineManager.setOnline(true));
  await waitFor(() => expect(remote.read).toHaveBeenCalledOnce());
  await waitFor(() =>
    expect(screen.getByTestId("source").textContent).toContain(
      "Original words",
    ),
  );
});
it.each([50, 150, 500])(
  "isolates same IDs/scope-string replacement from delayed %sms transcript and person replies",
  async (latency) => {
    const old = fixture("online", "same-scope");
    const next = fixture("online", "same-scope");
    next.rows.set("same-session", [
      next.transcript("same-session", "Replacement words"),
    ]);
    next.setPeople([{ id: "same-person", name: "Replacement person" }]);
    const originalRows = old.read.getMockImplementation()!;
    const originalPeople = old.peopleRead.getMockImplementation()!;
    old.read.mockImplementation(async (...args) => {
      await new Promise((done) => setTimeout(done, latency));
      return originalRows(...args);
    });
    old.peopleRead.mockImplementation(async (...args) => {
      await new Promise((done) => setTimeout(done, latency));
      return originalPeople(...args);
    });
    const mounted = render(
      <Host backend={old}>
        <Reader />
      </Host>,
    );
    await waitFor(() => expect(old.read).toHaveBeenCalled());
    mounted.rerender(
      <Host backend={next}>
        <Reader />
      </Host>,
    );
    await waitFor(() =>
      expect(screen.getByTestId("source").textContent).toContain(
        "Replacement person",
      ),
    );
    await act(async () => {
      await new Promise((done) => setTimeout(done, latency));
    });
    expect(screen.getByTestId("source").textContent).toContain(
      "Replacement words",
    );
    expect(screen.getByTestId("source").textContent).not.toContain("Original");
    for (const scope of old.scopes)
      expect(
        scope.queryClient.getQueryData(
          scope.transcriptsOptions("same-session").queryKey,
        ),
      ).toBeUndefined();
  },
);
it("requires the owning session and disables an old session's refetch closure after switching", async () => {
  const backend = fixture();
  let oldRead: (() => unknown) | undefined;
  const mounted = render(
    <Host backend={backend}>
      <Reader
        remember={(read) => {
          oldRead ??= read;
        }}
      />
    </Host>,
  );
  await waitFor(() =>
    expect(screen.getByTestId("source").textContent).toContain(
      "Original words",
    ),
  );
  mounted.rerender(
    <Host backend={backend}>
      <Reader id="other-session" />
    </Host>,
  );
  await waitFor(() =>
    expect(screen.getByTestId("source").textContent).toContain(
      "Other session words",
    ),
  );
  const before = backend.read.mock.calls.length;
  await act(async () => {
    await oldRead?.();
  });
  expect(backend.read).toHaveBeenCalledTimes(before);
  expect(native.transcriptGet).not.toHaveBeenCalled();
});
it("hides words/people on terminal access loss, never revives or dispatches retained refetch", async () => {
  const backend = fixture();
  let retained: (() => unknown) | undefined;
  render(
    <Host backend={backend}>
      <Reader
        remember={(read) => {
          retained ??= read;
        }}
      />
    </Host>,
  );
  await waitFor(() =>
    expect(screen.getByTestId("source").textContent).toContain(
      "Original person",
    ),
  );
  act(() =>
    backend.emit({ kind: "connection", state: "access_denied", error: null }),
  );
  expect(screen.getByTestId("source").textContent).not.toContain("Original");
  const count = backend.read.mock.calls.length;
  act(() =>
    backend.emit({ kind: "connection", state: "connected", error: null }),
  );
  await act(async () => {
    await retained?.();
  });
  expect(backend.read).toHaveBeenCalledTimes(count);
  expect(screen.getByTestId("source").textContent).not.toContain("Original");
});
it("refuses foreign session rows instead of hydrating them under the requested session", async () => {
  const backend = fixture();
  backend.read.mockResolvedValue([
    backend.transcript("other-session", "Private wrong-owner words"),
  ]);
  render(
    <Host backend={backend}>
      <Reader />
    </Host>,
  );
  await waitFor(() =>
    expect(screen.getByTestId("source-error").textContent).toContain(
      "different transcript session",
    ),
  );
  expect(screen.getByTestId("source").textContent).not.toContain(
    "Private wrong-owner words",
  );
});

it("refreshes transcript/person changes and resync only within the captured Scope", async () => {
  const own = fixture();
  const other = fixture();
  render(
    <>
      <Host backend={own}>
        <Reader name="own" />
      </Host>
      <Host backend={other}>
        <Reader name="other" />
      </Host>
    </>,
  );
  await waitFor(() =>
    expect(screen.getByTestId("own").textContent).toContain("Original person"),
  );
  await waitFor(() =>
    expect(screen.getByTestId("other").textContent).toContain(
      "Original person",
    ),
  );
  const otherReads = other.read.mock.calls.length;
  const otherPeopleReads = other.peopleRead.mock.calls.length;
  own.rows.set("same-session", [
    own.transcript("same-session", "Updated words"),
  ]);
  own.setPeople([{ id: "same-person", name: "Updated person" }]);
  act(() => {
    own.emit({
      kind: "change",
      change: {
        kind: "index_changed",
        entity: "transcripts",
        ids: ["same-session"],
      },
    });
    own.emit({
      kind: "change",
      change: { kind: "index_changed", entity: "people", ids: ["same-person"] },
    });
  });
  await waitFor(() =>
    expect(screen.getByTestId("own").textContent).toContain("Updated person"),
  );
  expect(screen.getByTestId("own").textContent).toContain("Updated words");
  expect(other.read).toHaveBeenCalledTimes(otherReads);
  expect(other.peopleRead).toHaveBeenCalledTimes(otherPeopleReads);
  const before = own.read.mock.calls.length;
  act(() => {
    for (const scope of own.scopes) scope.resync();
  });
  await waitFor(() =>
    expect(own.read.mock.calls.length).toBeGreaterThan(before),
  );
  expect(other.read).toHaveBeenCalledTimes(otherReads);
});
it("cannot dispatch paused remote source reads after retirement even under a mounted global provider", async () => {
  onlineManager.setOnline(false);
  const backend = fixture("online");
  const mounted = render(
    <Host backend={backend}>
      <Reader />
    </Host>,
  );
  await waitFor(() => expect(backend.scopes.length).toBeGreaterThan(0));
  mounted.unmount();
  act(() => onlineManager.setOnline(true));
  await new Promise((done) => setTimeout(done, 20));
  expect(backend.read).not.toHaveBeenCalled();
  expect(backend.peopleRead).not.toHaveBeenCalled();
});
