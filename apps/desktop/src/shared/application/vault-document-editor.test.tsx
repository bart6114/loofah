import { onlineManager } from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  views: [] as import("prosemirror-view").EditorView[],
  draftCacheGet: vi.fn(),
  draftCacheInventory: vi.fn(),
  draftCacheRecovery: vi.fn(),
  draftCachePut: vi.fn(),
  draftCacheRemove: vi.fn(),
  uploadGates: [] as boolean[],
}));
vi.mock("~/types/tauri.gen", () => ({ commands: native }));
vi.mock("./use-vault-note-file-handler", () => ({
  useVaultNoteFileHandler: (
    _scope: unknown,
    _session: string,
    enabled: boolean,
    callbacks: Record<string, unknown>,
  ) => {
    native.uploadGates.push(enabled);
    return {
      targetRef: { current: null },
      onViewReady: callbacks?.onViewReady,
      onViewDisposed: callbacks?.onViewDisposed,
    };
  },
}));
vi.mock("@hypr/editor/note", async (original) => {
  const actual = await original<typeof import("@hypr/editor/note")>();
  const { createElement, forwardRef, useCallback } = await import("react");
  return {
    ...actual,
    NoteEditor: forwardRef<
      import("@hypr/editor/note").NoteEditorRef,
      import("@hypr/editor/note").NoteEditorProps
    >((props, ref) => {
      const { onViewReady } = props;
      const ready = useCallback(
        (view: import("prosemirror-view").EditorView) => {
          native.views.push(view);
          onViewReady?.(view);
        },
        [onViewReady],
      );
      return createElement(actual.NoteEditor, {
        ...props,
        ref,
        onViewReady: ready,
      });
    }),
  };
});

import { scopedAttachmentPresentation } from "@hypr/editor/attachment-presentation";

import {
  documentEditorDraftId,
  documentEditorDrafts,
} from "./document-editor-draft";
import { ApplicationRequestError } from "./errors";
import type { DocumentEditorSnapshot, DocumentTarget } from "./protocol.gen";
import { sessionEditorFlushKey } from "./session-cleanup-guard";
import { captureSessionGenerationGuard } from "./session-generation-guard";
import type { VaultClient, VaultClientEvent } from "./vault-client";
import { VaultDocumentEditor } from "./vault-document-editor";
import { VaultScope } from "./vault-scope";

import { flushDatabaseWrites } from "~/shared/write-queue";

const scopes: VaultScope[] = [];
const summary: DocumentTarget = { kind: "summary" };
const output: DocumentTarget = {
  kind: "template_output",
  document_id: "output",
};
function fixture(
  target = summary,
  scopeId: string = crypto.randomUUID(),
  networkMode: "always" | "online" = "always",
) {
  let current: DocumentEditorSnapshot = {
    document: {
      revision: "d0",
      document: {
        id: target.kind === "summary" ? "same" : target.document_id,
        session_id: "same",
        kind: target.kind,
        title: "Output frontmatter",
        template_id: "template",
        sort_order: 7,
        markdown: "Original body",
      },
    },
    metadata: {
      revision: "m0",
      meta: {
        id: "same",
        title: "Session title",
        created_at: "2026-11-08T10:00:00Z",
        started_at: null,
        ended_at: null,
        tags: [],
      },
    },
    tasks: { revision: "t0", tasks: [] },
  };
  let sequence = 0;
  const listeners = new Set<(event: VaultClientEvent) => void>();
  const read = vi.fn<VaultClient["readDocumentEditor"]>(async () =>
    structuredClone(current),
  );
  const save = vi.fn<VaultClient["saveDocumentEditor"]>(
    async (_session, requested, input) => {
      expect(requested).toEqual(target);
      if (
        input.expected_revision !== current.document.revision ||
        input.expected_metadata_revision !== current.metadata.revision ||
        input.expected_tasks_revision !== current.tasks.revision
      )
        return { kind: "conflict", snapshot: structuredClone(current) };
      const revision = ++sequence;
      current = {
        document: {
          revision: `d${revision}`,
          document: { ...current.document.document!, markdown: input.markdown },
        },
        metadata: {
          revision: `m${revision}`,
          meta: {
            ...current.metadata.meta,
            title: input.title ?? current.metadata.meta.title,
          },
        },
        tasks: {
          revision: `t${revision}`,
          tasks: input.tasks.map((task) => ({
            ...task,
            source_type:
              target.kind === "summary" ? "session_summary" : "enhanced_note",
            source_id: target.kind === "summary" ? "same" : target.document_id,
            assignee: "",
            created_at: "",
            updated_at: "",
          })),
        },
      };
      return { kind: "saved", snapshot: structuredClone(current) };
    },
  );
  const client = {
    scope: scopeId,
    networkMode,
    canEdit: true,
    attachmentPresentation: scopedAttachmentPresentation,
    features: [
      "document_editor_read",
      "document_editor_write",
      "attachment_upload",
    ],
    readDocumentEditor: read,
    saveDocumentEditor: save,
    subscribe: async (listener: (event: VaultClientEvent) => void) => {
      listeners.add(listener);
      listener({ kind: "connection", state: "connected", error: null });
      return () => {
        listeners.delete(listener);
      };
    },
  } as unknown as VaultClient;
  const scope = new VaultScope(client);
  scopes.push(scope);
  return {
    scope,
    client,
    read,
    save,
    value: () => structuredClone(current),
    replace: (value: DocumentEditorSnapshot) => {
      current = value;
    },
    emit: (event: VaultClientEvent) => {
      for (const listener of listeners) listener(event);
    },
  };
}
async function mount(backend: ReturnType<typeof fixture>, target = summary) {
  await backend.scope.start();
  const result = render(
    <StrictMode>
      <VaultDocumentEditor
        scope={backend.scope}
        sessionId="same"
        target={target}
      />
    </StrictMode>,
  );
  await waitFor(() =>
    expect(native.views[native.views.length - 1]?.dom.isConnected).toBe(true),
  );
  return { ...result, view: native.views[native.views.length - 1]! };
}
beforeEach(() => {
  native.views = [];
  native.uploadGates = [];
  native.draftCacheGet
    .mockReset()
    .mockResolvedValue({ status: "ok", data: null });
  native.draftCacheInventory
    .mockReset()
    .mockResolvedValue({ status: "ok", data: [] });
  native.draftCacheRecovery
    .mockReset()
    .mockResolvedValue({ status: "ok", data: null });
  native.draftCachePut
    .mockReset()
    .mockResolvedValue({ status: "ok", data: null });
  native.draftCacheRemove
    .mockReset()
    .mockResolvedValue({ status: "ok", data: null });
});
afterEach(async () => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
  await flushDatabaseWrites();
  onlineManager.setOnline(true);
});
it.each([summary, output])(
  "flushes first-500ms $kind typing as one compound request with all revision baselines",
  async (target) => {
    const backend = fixture(target);
    const mounted = await mount(backend, target);
    act(() =>
      mounted.view.dispatch(mounted.view.state.tr.insertText("Typed ", 1)),
    );
    await act(async () => {
      await flushDatabaseWrites([
        sessionEditorFlushKey(backend.client.scope, "same"),
      ]);
    });
    expect(backend.save).toHaveBeenCalledOnce();
    expect(backend.save.mock.calls[0][2]).toMatchObject({
      title: "Typed Session title",
      expected_revision: "d0",
      expected_metadata_revision: "m0",
      expected_tasks_revision: "t0",
    });
    expect(backend.read.mock.calls.length).toBeGreaterThan(1);
    expect(backend.value().document.document?.title).toBe("Output frontmatter");
    expect(native.draftCachePut).toHaveBeenCalledWith(
      expect.objectContaining({
        documentEditor: expect.objectContaining({ sessionId: "same", target }),
        editorTasks: expect.anything(),
      }),
    );
  },
);
it("keeps a rejected/uncertain save in version3 recovery without reporting success", async () => {
  const backend = fixture();
  backend.save.mockRejectedValue(
    new ApplicationRequestError({
      code: "unavailable",
      message: "Reply lost",
      retryable: true,
      mutation_outcome: "unknown",
      request_id: null,
      current_revision: null,
    }),
  );
  const mounted = await mount(backend);
  act(() =>
    mounted.view.dispatch(mounted.view.state.tr.insertText("Keep ", 1)),
  );
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(screen.getByText("Reply lost")).toBeDefined());
  const saved = documentEditorDrafts.get(
    backend.client.scope,
    documentEditorDraftId("same", summary),
  );
  expect(saved?.getSnapshot().dirty).toBe(true);
  expect(saved?.recovery()).toMatchObject({
    uncertain: true,
    documentEditor: { target: summary },
  });
});
it("remounts on same-string Scope replacement and never admits the old reply", async () => {
  const old = fixture(summary, "same-scope");
  const replacement = fixture(summary, "same-scope");
  let resolve!: (value: DocumentEditorSnapshot) => void;
  old.read.mockImplementationOnce(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  await old.scope.start();
  await replacement.scope.start();
  const rendered = render(
    <VaultDocumentEditor scope={old.scope} sessionId="same" target={summary} />,
  );
  await waitFor(() => expect(old.read).toHaveBeenCalledOnce());
  old.scope.dispose();
  const next = replacement.value();
  next.metadata.meta.title = "Replacement title";
  replacement.replace(next);
  rendered.rerender(
    <VaultDocumentEditor
      scope={replacement.scope}
      sessionId="same"
      target={summary}
    />,
  );
  await waitFor(() =>
    expect(
      native.views[native.views.length - 1]?.state.doc.textContent,
    ).toContain("Replacement title"),
  );
  await act(async () => resolve(old.value()));
  expect(
    native.views[native.views.length - 1]?.state.doc.textContent,
  ).not.toContain("Session title");
  expect(
    old.scope.queryClient.getQueryData(
      old.scope.documentEditorOptions("same", summary).queryKey,
    ),
  ).toBeUndefined();
});
it("disables editing and file uploads for reader/lock, hiding canonical content at terminal access loss", async () => {
  const backend = fixture();
  Object.defineProperty(backend.client, "canEdit", { value: false });
  const mounted = await mount(backend);
  expect(mounted.view.editable).toBe(false);
  expect(native.uploadGates[native.uploadGates.length - 1]).toBe(false);
  act(() =>
    backend.emit({ kind: "connection", state: "access_denied", error: null }),
  );
  await waitFor(() => expect(mounted.view.isDestroyed).toBe(true));
  expect(screen.queryByText("Original body")).toBeNull();
  expect(backend.save).not.toHaveBeenCalled();
});

it("reads and saves the default local compound editor offline without authentication", async () => {
  onlineManager.setOnline(false);
  const backend = fixture();
  const mounted = await mount(backend);
  act(() =>
    mounted.view.dispatch(mounted.view.state.tr.insertText("Offline ", 1)),
  );
  await act(async () => {
    await flushDatabaseWrites([
      sessionEditorFlushKey(backend.client.scope, "same"),
    ]);
  });
  expect(backend.save).toHaveBeenCalledOnce();
  expect(backend.value().metadata.meta.title).toBe("Offline Session title");
});
it("shows an explicit upgrade error without issuing an unsupported compound read", async () => {
  const backend = fixture();
  Object.defineProperty(backend.client, "features", { value: [] });
  await backend.scope.start();
  render(
    <VaultDocumentEditor
      scope={backend.scope}
      sessionId="same"
      target={summary}
    />,
  );
  await screen.findByText(/does not support the document editor/);
  expect(backend.read).not.toHaveBeenCalled();
  expect(backend.save).not.toHaveBeenCalled();
});

it("flushes the mounted compound target before generation rather than dropping its pending title/body/task intent", async () => {
  const backend = fixture(output);
  const current = backend.value();
  current.document.document!.markdown = "# Actions\n- [ ] Task";
  current.tasks.tasks = [
    {
      id: "owned-task",
      source_type: "enhanced_note",
      source_id: "output",
      source_order: 0,
      status: "in_progress",
      text: "Task",
      body: [
        {
          type: "paragraph",
          content: [{ type: "text", text: "Full rich task body" }],
        },
      ],
      due_at: "2027-02-01",
      assignee: "person",
      created_at: "",
      updated_at: "",
    },
  ];
  backend.replace(current);
  const mounted = await mount(backend, output);
  act(() =>
    mounted.view.dispatch(mounted.view.state.tr.insertText("Pending ", 1)),
  );
  const guard = captureSessionGenerationGuard(backend.scope, "same", output);
  try {
    await act(async () => {
      await guard.prepare();
    });
    expect(() => guard.assertReady()).not.toThrow();
    expect(backend.save).toHaveBeenCalledOnce();
    expect(backend.save.mock.calls[0][2]).toMatchObject({
      title: "Pending Session title",
      tasks: [
        {
          id: "owned-task",
          status: "in_progress",
          due_at: "2027-02-01",
          body: expect.arrayContaining([
            expect.objectContaining({ type: "paragraph" }),
          ]),
        },
      ],
    });
  } finally {
    guard.dispose();
  }
});

it("does not let a retired save sweep a mounted replacement target with the same scope string", async () => {
  const old = fixture(summary, "retired-save-scope");
  const next = fixture(output, "retired-save-scope");
  const actual = old.save.getMockImplementation()!;
  let finish!: () => void;
  const pending = new Promise<void>((done) => {
    finish = done;
  });
  old.save.mockImplementationOnce(async (...args) => {
    await pending;
    return actual(...args);
  });
  const mounted = await mount(old);
  try {
    act(() =>
      mounted.view.dispatch(mounted.view.state.tr.insertText("Old ", 1)),
    );
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(old.save).toHaveBeenCalledOnce());
    old.scope.dispose();
    await next.scope.start();
    mounted.rerender(
      <VaultDocumentEditor
        scope={next.scope}
        sessionId="same"
        target={output}
      />,
    );
    await waitFor(() =>
      expect(native.views[native.views.length - 1]?.dom.isConnected).toBe(true),
    );
    const key = documentEditorDraftId("same", output);
    const replacement = documentEditorDrafts.get(next.client.scope, key);
    expect(replacement).toBeDefined();
    await act(async () => {
      finish();
      await flushDatabaseWrites();
    });
    expect(documentEditorDrafts.get(next.client.scope, key)).toBe(replacement);
  } finally {
    finish();
  }
});
