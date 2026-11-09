import { onlineManager, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { StrictMode, type ReactNode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { md2json } from "@hypr/editor/markdown";

import { useSession, useSessionMetadata, useSessionRawMd } from "./queries";
import {
  createSessionSourceHost,
  sessionMetadata,
  sessionNote,
} from "./session-source.test-support";

import { VaultHostContext } from "~/shared/application/vault-provider";

const native = vi.hoisted(() => ({
  get: vi.fn(),
  read: vi.fn(),
  loadSource: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({
  commands: { sessionGet: native.get, sessionReadNote: native.read },
  events: {},
}));
vi.mock("@hypr/editor/task-storage", () => ({
  useTaskStorageOptional: () => ({ loadSource: native.loadSource }),
}));
const hosts: ReturnType<typeof createSessionSourceHost>[] = [];
function host(mode: "always" | "online" = "always", scope?: string) {
  const value = createSessionSourceHost(mode, scope ? { scope } : {});
  hosts.push(value);
  return value;
}
beforeEach(() => {
  vi.clearAllMocks();
  onlineManager.setOnline(true);
});
afterEach(() => {
  cleanup();
  hosts.splice(0).forEach((value) => value.dispose());
  onlineManager.setOnline(true);
});

it.each(["always", "online"] as const)(
  "deduplicates scoped raw/session/editor reads in %s mode without global hydration",
  async (mode) => {
    const value = host(mode);
    const view = renderHook(
      () => ({
        session: useSession("session-1"),
        raw: useSessionRawMd("session-1"),
        metadata: useSessionMetadata("session-1"),
      }),
      { wrapper: value.Host },
    );
    await waitFor(() =>
      expect(view.result.current.raw).toBe(
        JSON.stringify(md2json("Original body")),
      ),
    );
    expect(view.result.current.session).toMatchObject({
      id: "session-1",
      title: "Original title",
      raw_md: view.result.current.raw,
      tags: ["keep"],
      author: "writer",
      folder_id: "folder",
    });
    expect(value.readNoteEditor).toHaveBeenCalledOnce();
    expect(value.readMetadata).toHaveBeenCalledOnce();
    expect(value.rootClient.getQueryCache().getAll()).toEqual([]);
    expect(
      value.scope.queryClient.getQueryData(
        value.scope.noteEditorOptions("session-1").queryKey,
      ),
    ).toEqual(sessionNote());
    expect(native.get).not.toHaveBeenCalled();
    expect(native.read).not.toHaveBeenCalled();
    expect(native.loadSource).not.toHaveBeenCalled();
  },
);
it("uses only metadata for a metadata-only reader", async () => {
  const value = host();
  const view = renderHook(() => useSessionMetadata("session-1"), {
    wrapper: value.Host,
  });
  await waitFor(() =>
    expect(view.result.current?.meta.title).toBe("Original title"),
  );
  expect(value.readNoteEditor).not.toHaveBeenCalled();
});
it("returns null while missing or pending, and an empty string for an absent note in an existing session", async () => {
  const value = host();
  let finish!: () => void;
  value.readNoteEditor.mockImplementationOnce(
    () =>
      new Promise((done) => {
        finish = () => done(sessionNote(null));
      }),
  );
  const view = renderHook(() => useSessionRawMd("session-1"), {
    wrapper: value.Host,
  });
  expect(view.result.current).toBeNull();
  await waitFor(() => expect(value.readNoteEditor).toHaveBeenCalledOnce());
  await act(async () => finish());
  await waitFor(() => expect(view.result.current).toBe(""));
});
it("revalidates source on remount and session/task change events, scoped to the selected session", async () => {
  const value = host();
  const first = renderHook(() => useSessionRawMd("session-1"), {
    wrapper: value.Host,
  });
  await waitFor(() => expect(first.result.current).toContain("Original body"));
  first.unmount();
  value.setNote(sessionNote("External body", "Original title", "n1"));
  const reopened = renderHook(() => useSessionRawMd("session-1"), {
    wrapper: value.Host,
  });
  await waitFor(() =>
    expect(reopened.result.current).toContain("External body"),
  );
  value.setNote(sessionNote("Task event body", "Original title", "n2"));
  act(() =>
    value.emit({
      kind: "change",
      change: { kind: "index_changed", entity: "tasks", ids: ["session-1"] },
    }),
  );
  await waitFor(() =>
    expect(reopened.result.current).toContain("Task event body"),
  );
  const count = value.readNoteEditor.mock.calls.length;
  act(() =>
    value.emit({
      kind: "change",
      change: { kind: "index_changed", entity: "sessions", ids: ["another"] },
    }),
  );
  expect(value.readNoteEditor).toHaveBeenCalledTimes(count);
});
it("preserves local offline reads and resumes remote reads without a global client mount", async () => {
  onlineManager.setOnline(false);
  const local = host();
  const view = renderHook(() => useSessionRawMd("session-1"), {
    wrapper: local.Host,
  });
  await waitFor(() => expect(view.result.current).toContain("Original body"));
  const remote = host("online");
  const paused = renderHook(() => useSessionRawMd("session-1"), {
    wrapper: remote.Host,
  });
  expect(remote.readNoteEditor).not.toHaveBeenCalled();
  expect(paused.result.current).toBeNull();
  act(() => onlineManager.setOnline(true));
  await waitFor(() => expect(paused.result.current).toContain("Original body"));
  expect(remote.readNoteEditor).toHaveBeenCalledOnce();
});
it.each([50, 150, 500])(
  "rejects late %ims source replies across same-ID and same-string Scope replacement",
  async (delay) => {
    const old = host("online", "identical");
    const next = host("online", "identical");
    old.readNoteEditor.mockImplementation(
      () =>
        new Promise((done) =>
          setTimeout(() => done(sessionNote("Old private body")), delay),
        ),
    );
    old.readMetadata.mockImplementation(
      () =>
        new Promise((done) =>
          setTimeout(() => done(sessionMetadata("Old private title")), delay),
        ),
    );
    next.setNote(sessionNote("Replacement body"));
    next.setMetadata(sessionMetadata("Replacement title"));
    function Host({
      value,
      children,
    }: {
      value: typeof old;
      children?: ReactNode;
    }) {
      return (
        <QueryClientProvider client={value.rootClient}>
          <VaultHostContext.Provider
            value={{ scope: value.scope, error: null, retry: () => {} }}
          >
            <StrictMode>
              <Reader key={value.scope === old.scope ? "old" : "new"} />
              {children}
            </StrictMode>
          </VaultHostContext.Provider>
        </QueryClientProvider>
      );
    }
    function Reader() {
      const session = useSession("session-1");
      return (
        <output>
          {session?.title}|{session?.raw_md}
        </output>
      );
    }
    const { render, screen } = await import("@testing-library/react");
    const view = render(<Host value={old} />);
    await waitFor(() => expect(old.readNoteEditor).toHaveBeenCalled());
    old.scope.dispose();
    view.rerender(<Host value={next} />);
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toContain(
        "Replacement body",
      ),
    );
    await act(async () => {
      await new Promise((done) => setTimeout(done, delay + 10));
    });
    expect(screen.getByRole("status").textContent).not.toContain("Old private");
    expect(
      old.scope.queryClient.getQueryData(
        old.scope.noteEditorOptions("session-1").queryKey,
      ),
    ).toBeUndefined();
  },
);
it("hides source on terminal access loss and never falls back to native or revives the same host", async () => {
  const value = host();
  const view = renderHook(() => useSession("session-1"), {
    wrapper: value.Host,
  });
  await waitFor(() =>
    expect(view.result.current?.title).toBe("Original title"),
  );
  act(() =>
    value.emit({ kind: "connection", state: "access_denied", error: null }),
  );
  await waitFor(() => expect(view.result.current).toBeNull());
  act(() =>
    value.emit({ kind: "connection", state: "connected", error: null }),
  );
  expect(view.result.current).toBeNull();
  expect(native.get).not.toHaveBeenCalled();
});
