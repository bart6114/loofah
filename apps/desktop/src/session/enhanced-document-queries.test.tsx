import {
  QueryClient,
  QueryClientProvider,
  onlineManager,
} from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { StrictMode, useState, type ReactNode } from "react";
import { afterEach, expect, it, vi } from "vitest";

import {
  useEnhancedDocumentTarget,
  useEnhancedNote,
  useEnhancedNoteRecords,
  useEnhancedNoteTitle,
} from "./queries";

import type { DocumentEditorSnapshot } from "~/shared/application/protocol.gen";
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
  networkMode: "always" | "online" = "always",
  scope: string = crypto.randomUUID(),
) {
  const listeners = new Set<(event: VaultClientEvent) => void>();
  const values: DocumentEditorSnapshot[] = ["summary", "template_output"].map(
    (kind) => ({
      document: {
        document: {
          id: kind === "summary" ? "same" : "output",
          session_id: "same",
          kind: kind as "summary" | "template_output",
          title: `${scope} ${kind}`,
          template_id: "template",
          sort_order: 0,
          markdown: "# Actions\n- [ ] Send proposal",
        },
        revision: "d0",
      },
      metadata: {
        meta: {
          id: "same",
          title: "SESSION metadata title",
          created_at: "",
          started_at: null,
          ended_at: null,
          tags: [],
        },
        revision: "m0",
      },
      tasks: {
        revision: "t0",
        tasks: [
          {
            id: "saved-task",
            source_type:
              kind === "summary" ? "session_summary" : "enhanced_note",
            source_id: kind === "summary" ? "same" : "output",
            status: "done",
            source_order: 0,
            text: "Send proposal",
            body: [
              {
                type: "paragraph",
                content: [{ type: "text", text: "Send proposal" }],
              },
            ],
            due_at: "2026-10-01",
            assignee: "person",
            created_at: "",
            updated_at: "",
          },
        ],
      },
    }),
  );
  const documents = vi.fn<VaultClient["documents"]>(async () =>
    values.flatMap((value) =>
      value.document.document ? [structuredClone(value.document.document)] : [],
    ),
  );
  const read = vi.fn<VaultClient["readDocumentEditor"]>(
    async (_session, target) =>
      structuredClone(
        values.find((value) => value.document.document?.kind === target.kind)!,
      ),
  );
  const client = {
    scope,
    networkMode,
    canEdit: true,
    features: ["document_editor_read"],
    documents,
    readDocumentEditor: read,
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
    documents,
    read,
    values,
    scopes: [] as VaultScope[],
    emit: (event: VaultClientEvent) => {
      for (const listener of listeners) listener(event);
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
  const [queryClient] = useState(() => new QueryClient());
  return (
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <VaultClientProvider client={backend.client}>
          <Gate backend={backend}>{children}</Gate>
        </VaultClientProvider>
      </QueryClientProvider>
    </StrictMode>
  );
}
function Reader({ id = "output", name = "selected" }) {
  const list = useEnhancedNoteRecords("same");
  const target = useEnhancedDocumentTarget("same", id);
  const value = useEnhancedNote(id, undefined, "same");
  return (
    <>
      <output data-testid={`${name}-list`}>
        {list.map((row) => row.title).join("|")}
      </output>
      <output data-testid={`${name}-target`}>{JSON.stringify(target)}</output>
      <output data-testid={name}>{value?.content ?? "missing"}</output>
    </>
  );
}
afterEach(() => {
  cleanup();
  onlineManager.setOnline(true);
});
it.each(["always", "online"] as const)(
  "deduplicates captured %s document reads and hydrates only owned tasks without global TaskStorage",
  async (mode) => {
    const backend = fixture(mode);
    render(
      <Host backend={backend}>
        <Reader />
        <Reader name="second" />
      </Host>,
    );
    await waitFor(() =>
      expect(screen.getByTestId("selected").textContent).toContain(
        "saved-task",
      ),
    );
    expect(screen.getByTestId("selected").textContent).toContain(
      '"status":"done"',
    );
    expect(screen.getByTestId("selected").textContent).not.toContain(
      "SESSION metadata title",
    );
    expect(backend.documents).toHaveBeenCalledOnce();
    expect(backend.read).toHaveBeenCalledOnce();
    expect(backend.read).toHaveBeenCalledWith(
      "same",
      { kind: "template_output", document_id: "output" },
      { signal: expect.any(AbortSignal) },
    );
  },
);
it("preserves local offline/login-free reads and pauses remote reads until online", async () => {
  onlineManager.setOnline(false);
  const local = fixture();
  const rendered = render(
    <Host backend={local}>
      <Reader />
    </Host>,
  );
  await waitFor(() =>
    expect(screen.getByTestId("selected").textContent).toContain("saved-task"),
  );
  const remote = fixture("online");
  rendered.rerender(
    <Host backend={remote}>
      <Reader />
    </Host>,
  );
  await waitFor(() =>
    expect(screen.getByTestId("selected").textContent).toBe("missing"),
  );
  expect(remote.documents).not.toHaveBeenCalled();
  act(() => onlineManager.setOnline(true));
  await waitFor(() => expect(remote.read).toHaveBeenCalledOnce());
});
it("blocks ambiguous or session-shaped template IDs without reading either target", async () => {
  const backend = fixture();
  backend.values[1].document.document!.id = "same";
  render(
    <Host backend={backend}>
      <Reader id="same" />
    </Host>,
  );
  await waitFor(() =>
    expect(screen.getByTestId("selected-target").textContent).toBe("null"),
  );
  expect(backend.read).not.toHaveBeenCalled();
});
it.each([50, 150, 500])(
  "isolates same-ID/same-scope-string replacement from a late %sms reply",
  async (latency) => {
    const old = fixture("online", "same-scope");
    old.values[1].document.document!.markdown = "Old canonical body";
    const next = fixture("online", "same-scope");
    next.values[1].document.document!.markdown = "Replacement content";
    const actual = old.read.getMockImplementation()!;
    old.read.mockImplementationOnce(async (...args) => {
      await new Promise((done) => setTimeout(done, latency));
      return actual(...args);
    });
    const rendered = render(
      <Host backend={old}>
        <Reader />
      </Host>,
    );
    await waitFor(() => expect(old.read).toHaveBeenCalledOnce());
    rendered.rerender(
      <Host backend={next}>
        <Reader />
      </Host>,
    );
    await waitFor(() =>
      expect(screen.getByTestId("selected").textContent).toContain(
        "Replacement content",
      ),
    );
    await act(async () => {
      await new Promise((done) => setTimeout(done, latency));
    });
    expect(screen.getByTestId("selected").textContent).not.toContain(
      "Old canonical body",
    );
    for (const scope of old.scopes)
      expect(
        scope.queryClient.getQueryData(
          scope.documentEditorOptions("same", {
            kind: "template_output",
            document_id: "output",
          }).queryKey,
        ),
      ).toBeUndefined();
  },
);
it("hides canonical rows/body on terminal access loss and never reopens them within that Scope", async () => {
  const backend = fixture();
  render(
    <Host backend={backend}>
      <Reader />
    </Host>,
  );
  await waitFor(() =>
    expect(screen.getByTestId("selected").textContent).toContain("saved-task"),
  );
  act(() =>
    backend.emit({ kind: "connection", state: "access_denied", error: null }),
  );
  expect(screen.getByTestId("selected").textContent).toBe("missing");
  expect(screen.getByTestId("selected-list").textContent).toBe("");
  act(() =>
    backend.emit({ kind: "connection", state: "connected", error: null }),
  );
  expect(screen.getByTestId("selected").textContent).toBe("missing");
});

it("shows inactive document titles from one scoped descriptor read without compound task hydration", async () => {
  const backend = fixture();
  function Pill({ index }: { index: number }) {
    return (
      <output data-testid={`pill-${index}`}>
        {useEnhancedNoteTitle("same", "output")}
      </output>
    );
  }
  render(
    <Host backend={backend}>
      {Array.from({ length: 12 }, (_, index) => (
        <Pill key={index} index={index} />
      ))}
    </Host>,
  );
  await waitFor(() =>
    expect(screen.getByTestId("pill-11").textContent).toContain(
      "template_output",
    ),
  );
  expect(backend.documents).toHaveBeenCalledOnce();
  expect(backend.read).not.toHaveBeenCalled();
});
