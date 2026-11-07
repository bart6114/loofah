import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  views: [] as import("prosemirror-view").EditorView[],
  serialize: null as
    | null
    | typeof import("@hypr/editor/markdown").json2mdStrict,
  draftCacheGet: vi.fn(),
  draftCacheInventory: vi.fn(),
  draftCacheRecovery: vi.fn(),
  draftCachePut: vi.fn(),
  draftCacheRemove: vi.fn(),
  deviceJobSubmissions: vi.fn(),
  deviceJobSubmissionPrepare: vi.fn(),
  deviceJobSubmissionGet: vi.fn(),
  deviceJobSubmissionObserve: vi.fn(),
  deviceJobSubmissionForget: vi.fn(),
  receipts: [] as import("~/types/tauri.gen").JobSubmissionReceipt[],
}));
vi.mock("~/types/tauri.gen", () => ({ commands: native }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));
vi.mock("@hypr/editor/markdown", async (importOriginal) => {
  const original =
    await importOriginal<typeof import("@hypr/editor/markdown")>();
  native.serialize = original.json2mdStrict;
  return { ...original, json2mdStrict: vi.fn(original.json2mdStrict) };
});
vi.mock("@hypr/editor/note", async (importOriginal) => {
  const original = await importOriginal<typeof import("@hypr/editor/note")>();
  const { createElement, forwardRef, useCallback } = await import("react");
  return {
    ...original,
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
      return createElement(original.NoteEditor, {
        ...props,
        ref,
        onViewReady: ready,
      });
    }),
  };
});

import { writeText } from "@tauri-apps/plugin-clipboard-manager";

import { scopedAttachmentPresentation } from "@hypr/editor/attachment-presentation";
import * as editorMarkdown from "@hypr/editor/markdown";

import { documentDraftId } from "./document-draft";
import { ApplicationRequestError } from "./errors";
import { noteEditorDraftId, noteEditorDrafts } from "./note-editor-draft";
import { VaultNotesWorkspace } from "./notes-workspace";
import type {
  NoteSnapshot,
  DocumentSnapshot,
  TasksSnapshot,
  SummaryJobRequest,
} from "./protocol.gen";
import { sessionEditorFlushKey } from "./session-cleanup-guard";
import type { VaultClient, VaultClientEvent } from "./vault-client";

import { flushDatabaseWrites } from "~/shared/write-queue";

const note = (markdown: string, revision: string): NoteSnapshot => ({
  markdown,
  title: "Seed",
  revision,
});
function fixture(rich = false) {
  const listeners = new Set<(event: VaultClientEvent) => void>();
  let current = note("Original", "a");
  let currentTasks: TasksSnapshot = { tasks: [], revision: "tasks-a" };
  let editorSequence = 0;
  let currentDocument: DocumentSnapshot = {
    document: {
      id: "same",
      session_id: "same",
      kind: "summary",
      title: "Summary",
      template_id: "",
      sort_order: 0,
      markdown: "Canonical summary",
    },
    revision: "doc-a",
  };
  let documentSequence = 0;
  const client = {
    scope: crypto.randomUUID(),
    networkMode: "always",
    canEdit: true,
    features: [
      "notes",
      "search",
      ...(rich ? ["note_editor_read", "note_editor_write"] : []),
    ],
    attachmentPresentation: scopedAttachmentPresentation,
    openAttachment: vi.fn(),
    attachments: vi.fn(),
    readNoteEditor: vi.fn(async () => ({ note: current, tasks: currentTasks })),
    saveNoteEditor: vi.fn(async (_id, input) => {
      if (
        input.expected_revision !== current.revision ||
        input.expected_tasks_revision !== currentTasks.revision
      )
        return {
          kind: "conflict",
          snapshot: { note: current, tasks: currentTasks },
        };
      editorSequence += 1;
      current = {
        markdown: input.markdown,
        title: input.title ?? current.title,
        revision: `editor-${editorSequence}`,
      };
      currentTasks = {
        revision: `editor-tasks-${editorSequence}`,
        tasks: input.tasks.map((task: any) => ({
          ...task,
          source_type: "session_raw_note",
          source_id: _id,
          assignee: "",
          created_at: "",
          updated_at: "",
        })),
      };
      return {
        kind: "saved",
        snapshot: { note: current, tasks: currentTasks },
      };
    }),
    documents: vi.fn(async () =>
      currentDocument.document ? [currentDocument.document] : [],
    ),
    readDocument: vi.fn(async () => currentDocument),
    saveDocument: vi.fn(async (_sessionId, _target, input) => {
      if (input.expected_revision !== currentDocument.revision)
        return { kind: "conflict", snapshot: currentDocument };
      currentDocument = {
        document: {
          ...(currentDocument.document ?? {
            id: "same",
            session_id: "same",
            kind: "summary",
            title: "Summary",
            template_id: "",
            sort_order: 0,
          }),
          markdown: input.markdown,
        },
        revision: `doc-${++documentSequence}`,
      };
      return { kind: "saved", snapshot: currentDocument };
    }),
    processingJobs: vi.fn(async () => ({ items: [], next_cursor: null })),
    submitSummaryJob: vi.fn(async (input: SummaryJobRequest) => ({
      id: crypto.randomUUID(),
      session_id: input.session_id,
      target: input.target,
      state: "queued",
      phase: null,
      failure: null,
      created_at: "2026-11-07T12:00:00Z",
      updated_at: "2026-11-07T12:00:00Z",
    })),
    transcripts: vi.fn(async () => [
      {
        id: "transcript",
        session_id: "same",
        user_id: "",
        created_at: "",
        started_at: 0,
        ended_at: null,
        memo_md: "",
        words: [
          {
            text: "Canonical transcript",
            start_ms: 0,
            end_ms: 100,
            channel: 0,
            speaker: null,
            metadata: null,
          },
        ],
        speaker_hints: [],
      },
    ]),
    tasks: vi.fn(async () => [
      {
        id: "task",
        source_type: "session_raw_note",
        source_id: "same",
        source_order: 0,
        status: "todo",
        text: "Canonical task",
        body: [],
        due_at: "",
        assignee: "",
        created_at: "",
        updated_at: "",
      },
    ]),
    search: vi.fn(async () => ({ hits: [], count: 0 })),
    readNote: vi.fn(async () => current),
    sessions: vi.fn(async () => ({
      sessions: [
        {
          id: "same",
          title: "Seed",
          created_at: "2026-11-05T10:00:00Z",
          started_at: null,
          ended_at: null,
          tags: [],
        },
      ],
      next_cursor: null,
    })),
    saveNote: vi.fn(async (_id, input) => {
      current = { markdown: input.markdown, title: input.title, revision: "b" };
      return { kind: "saved", note: current };
    }),
    subscribe: vi.fn(async (listener) => {
      listeners.add(listener);
      listener({ kind: "connection", state: "connected", error: null });
      return () => {
        listeners.delete(listener);
      };
    }),
  } as unknown as VaultClient & {
    saveNote: ReturnType<typeof vi.fn>;
    openAttachment: ReturnType<typeof vi.fn>;
    attachments: ReturnType<typeof vi.fn>;
    saveNoteEditor: ReturnType<typeof vi.fn<VaultClient["saveNoteEditor"]>>;
    saveDocument: ReturnType<typeof vi.fn<VaultClient["saveDocument"]>>;
    search: ReturnType<typeof vi.fn>;
    submitSummaryJob: ReturnType<typeof vi.fn<VaultClient["submitSummaryJob"]>>;
  };
  return {
    client,
    set: (value: NoteSnapshot) => {
      current = value;
    },
    setTasks: (value: TasksSnapshot) => {
      currentTasks = value;
    },
    setDocument: (value: DocumentSnapshot) => {
      currentDocument = value;
    },
    emit: (event: VaultClientEvent) => {
      for (const listener of listeners) listener(event);
    },
  };
}
async function open(client: VaultClient) {
  render(
    <StrictMode>
      <VaultNotesWorkspace client={client} />
    </StrictMode>,
  );
  fireEvent.click(await screen.findByRole("button", { name: "Seed" }));
  return screen.findByRole("textbox", { name: "Markdown" });
}

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(editorMarkdown.json2mdStrict).mockImplementation(native.serialize!);
  native.views.length = 0;
  native.draftCacheInventory.mockResolvedValue({ status: "ok", data: [] });
  native.draftCacheRecovery.mockResolvedValue({ status: "ok", data: null });
  native.draftCacheGet.mockResolvedValue({ status: "ok", data: null });
  native.draftCachePut.mockResolvedValue({ status: "ok", data: undefined });
  native.draftCacheRemove.mockResolvedValue({ status: "ok", data: undefined });
  native.receipts.length = 0;
  native.deviceJobSubmissions.mockImplementation(async (scope, sessionId) => ({
    status: "ok",
    data: native.receipts.filter(
      (receipt) =>
        receipt.scope === scope &&
        (!sessionId || receipt.input.session_id === sessionId),
    ),
  }));
  native.deviceJobSubmissionPrepare.mockImplementation(async (scope, input) => {
    const receipt = {
      scope,
      input,
      request_id: crypto.randomUUID(),
      job: null,
      created_at_ms: Date.now(),
    };
    native.receipts.push(receipt);
    return { status: "ok", data: receipt };
  });
  native.deviceJobSubmissionGet.mockImplementation(async (scope, id) => ({
    status: "ok",
    data:
      native.receipts.find(
        (receipt) => receipt.scope === scope && receipt.request_id === id,
      ) ?? null,
  }));
  native.deviceJobSubmissionObserve.mockImplementation(
    async (scope, id, job) => {
      const receipt = native.receipts.find(
        (receipt) => receipt.scope === scope && receipt.request_id === id,
      )!;
      receipt.job = job;
      return { status: "ok", data: receipt };
    },
  );
});
afterEach(async () => {
  cleanup();
  await flushDatabaseWrites();
});

async function openRich(client: VaultClient) {
  const rendered = render(
    <StrictMode>
      <VaultNotesWorkspace client={client} />
    </StrictMode>,
  );
  fireEvent.click(await screen.findByRole("button", { name: "Seed" }));
  await waitFor(() => {
    const view = native.views[native.views.length - 1];
    expect(view?.dom.isConnected).toBe(true);
    expect(view?.isDestroyed).toBe(false);
    expect(view?.editable).toBe(client.canEdit);
  });
  return { ...rendered, view: native.views[native.views.length - 1] };
}

function richFixture() {
  const backend = fixture(true);
  backend.set(
    note(
      "# Seed\n\n- [ ] Follow up\n\n| Keep | Table |\n| --- | --- |\n| Rich | Content |",
      "a",
    ),
  );
  backend.setTasks({
    revision: "tasks-a",
    tasks: [
      {
        id: "stable-task",
        source_type: "session_raw_note",
        source_id: "same",
        source_order: 0,
        status: "in_progress",
        text: "Follow up",
        body: [
          { type: "paragraph", content: [{ type: "text", text: "Follow up" }] },
        ],
        due_at: "2027-01-01",
        assignee: "",
        created_at: "",
        updated_at: "",
      },
    ],
  });
  return backend;
}

describe("mounted shared rich editor", () => {
  it("releases a clean editor draft while its vault workspace remains open", async () => {
    const backend = richFixture();
    const client = {
      ...backend.client,
      features: [...backend.client.features, "content_read"],
    };
    const mounted = await openRich(client);
    const id = noteEditorDraftId("same");
    const first = noteEditorDrafts.get(client.scope, id);
    expect(first).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: "Outputs" }));
    await waitFor(() => expect(mounted.view.isDestroyed).toBe(true));
    expect(noteEditorDrafts.get(client.scope, id)).toBeUndefined();
    fireEvent.click(screen.getByRole("button", { name: "Note" }));
    await waitFor(() =>
      expect(native.views[native.views.length - 1].dom.isConnected).toBe(true),
    );
    expect(noteEditorDrafts.get(client.scope, id)).toBeDefined();
    expect(noteEditorDrafts.get(client.scope, id)).not.toBe(first);
  });

  it("keeps an unmounted pending draft until its save confirms, then releases it", async () => {
    const backend = richFixture();
    const client = {
      ...backend.client,
      features: [...backend.client.features, "content_read"],
    };
    let finish!: () => void;
    const pending = new Promise<void>((resolve) => {
      finish = resolve;
    });
    const actualSave = backend.client.saveNoteEditor.getMockImplementation()!;
    backend.client.saveNoteEditor.mockImplementationOnce(async (...args) => {
      await pending;
      return actualSave(...args);
    });
    const mounted = await openRich(client);
    act(() =>
      mounted.view.dispatch(mounted.view.state.tr.insertText("Pending ", 1)),
    );
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(client.saveNoteEditor).toHaveBeenCalledOnce());
    fireEvent.click(screen.getByRole("button", { name: "Outputs" }));
    await waitFor(() => expect(mounted.view.isDestroyed).toBe(true));
    const id = noteEditorDraftId("same");
    expect(noteEditorDrafts.get(client.scope, id)?.getSnapshot().dirty).toBe(
      true,
    );
    await act(async () => {
      finish();
      await flushDatabaseWrites();
    });
    await waitFor(() =>
      expect(noteEditorDrafts.get(client.scope, id)).toBeUndefined(),
    );
    expect((await client.readNoteEditor("same")).note.title).toBe(
      "Pending Seed",
    );
  });

  it("flushes an edit through the existing local note key before its editor debounce fires", async () => {
    const backend = richFixture();
    backend.client.noteWriteQueueKey = (id) => `session:${id}:note`;
    const mounted = await openRich(backend.client);
    act(() =>
      mounted.view.dispatch(mounted.view.state.tr.insertText("Flushed ", 1)),
    );
    await act(async () => {
      await flushDatabaseWrites(["session:same:note"]);
    });
    expect(backend.client.saveNoteEditor).toHaveBeenCalledOnce();
    expect(backend.client.saveNoteEditor.mock.calls[0][1].title).toBe(
      "Flushed Seed",
    );
    expect(native.draftCachePut).toHaveBeenCalledWith(
      expect.objectContaining({
        draft: expect.objectContaining({ title: "Flushed Seed" }),
      }),
    );
  });
  it("flushes a rich editor's captured first-500ms change through the scoped cleanup owner key", async () => {
    const backend = richFixture();
    const mounted = await openRich(backend.client);
    act(() =>
      mounted.view.dispatch(mounted.view.state.tr.insertText("Owner ", 1)),
    );
    await act(async () => {
      await flushDatabaseWrites([
        sessionEditorFlushKey(backend.client.scope, "same"),
      ]);
    });
    expect(backend.client.saveNoteEditor).toHaveBeenCalledOnce();
    expect(backend.client.saveNoteEditor.mock.calls[0][1].title).toBe(
      "Owner Seed",
    );
    expect(native.draftCachePut).toHaveBeenCalledWith(
      expect.objectContaining({
        scope: backend.client.scope,
        sessionId: noteEditorDraftId("same"),
        draft: expect.objectContaining({ title: "Owner Seed" }),
      }),
    );
  });
  it("keeps the rendered draft through a serialization failure and remount until a safe save succeeds", async () => {
    const backend = richFixture();
    const mounted = await openRich(backend.client);
    const serialize = vi
      .mocked(editorMarkdown.json2mdStrict)
      .mockImplementation(() => {
        throw new Error("Serialization refused this content");
      });
    try {
      act(() =>
        mounted.view.dispatch(
          mounted.view.state.tr.insertText("Keep rendered ", 1),
        ),
      );
      await screen.findAllByText(
        "This content cannot be safely serialized by the current editor",
        {},
        { timeout: 3000 },
      );
      expect(backend.client.saveNoteEditor).not.toHaveBeenCalled();
      mounted.unmount();
      const reopened = await openRich(backend.client);
      expect(reopened.view.state.doc.textContent).toContain(
        "Keep rendered Seed",
      );
      await screen.findAllByText(
        "This content cannot be safely serialized by the current editor",
        {},
        { timeout: 3000 },
      );
      serialize.mockImplementation(native.serialize!);
      act(() =>
        reopened.view.dispatch(reopened.view.state.tr.insertText("Fixed ", 1)),
      );
      await waitFor(
        () => expect(backend.client.saveNoteEditor).toHaveBeenCalled(),
        { timeout: 3000 },
      );
      expect(
        backend.client.saveNoteEditor.mock.calls[
          backend.client.saveNoteEditor.mock.calls.length - 1
        ][1].title,
      ).toBe("Fixed Keep rendered Seed");
    } finally {
      serialize.mockImplementation(native.serialize!);
    }
  });
  it("saves title, Markdown and explicit task intent together without writing on mount", async () => {
    const backend = richFixture();
    const { view, container } = await openRich(backend.client);
    expect(backend.client.saveNoteEditor).not.toHaveBeenCalled();
    expect(container.querySelector("table")).not.toBeNull();
    expect(
      container.querySelector('[data-task-id="stable-task"]'),
    ).not.toBeNull();
    act(() => view.dispatch(view.state.tr.insertText("Updated ", 1)));
    fireEvent.click(container.querySelector('input[type="checkbox"]')!);
    await waitFor(
      () => expect(backend.client.saveNoteEditor).toHaveBeenCalledOnce(),
      { timeout: 3000 },
    );
    const input = backend.client.saveNoteEditor.mock.calls[0][1];
    expect(input).toMatchObject({
      title: "Updated Seed",
      expected_revision: "a",
      expected_tasks_revision: "tasks-a",
      tasks: [{ id: "stable-task", status: "done", due_at: "2027-01-01" }],
    });
    expect(input.markdown).toContain("# Updated Seed");
    expect(input.markdown).toContain("- [x] Follow up");
    expect(input.markdown).toContain("| Keep | Table |");
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Save" })).toHaveProperty(
        "disabled",
        true,
      ),
    );
    expect(backend.client.saveNote).not.toHaveBeenCalled();
    expect(native.draftCachePut).toHaveBeenCalledWith(
      expect.objectContaining({
        sessionId: noteEditorDraftId("same"),
        editorTasks: expect.objectContaining({
          draft: expect.arrayContaining([
            expect.objectContaining({
              id: "stable-task",
              due_at: "2027-01-01",
            }),
          ]),
        }),
      }),
    );
  });

  it("preserves edits made during a slow save and uses its returned revisions for the next save", async () => {
    const backend = richFixture();
    let finish!: () => void;
    const waiting = new Promise<void>((done) => {
      finish = done;
    });
    const commit = backend.client.saveNoteEditor.getMockImplementation()!;
    backend.client.saveNoteEditor.mockImplementationOnce(async (...args) => {
      await waiting;
      return commit(...args);
    });
    const { view } = await openRich(backend.client);
    act(() => view.dispatch(view.state.tr.insertText("First ", 1)));
    await waitFor(
      () => expect(backend.client.saveNoteEditor).toHaveBeenCalledOnce(),
      { timeout: 3000 },
    );
    act(() => view.dispatch(view.state.tr.insertText("Second ", 1)));
    finish();
    await waitFor(
      () => expect(backend.client.saveNoteEditor).toHaveBeenCalledTimes(2),
      { timeout: 3000 },
    );
    expect(backend.client.saveNoteEditor.mock.calls[1][1]).toMatchObject({
      expected_revision: "editor-1",
      expected_tasks_revision: "editor-tasks-1",
      title: "Second First Seed",
    });
    expect(view.state.doc.textContent).toContain("Second First Seed");
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Save" })).toHaveProperty(
        "disabled",
        true,
      ),
    );
  });

  it("retains task-aware device recovery through a disconnect and saves after reconnection", async () => {
    const backend = richFixture();
    const { view, container } = await openRich(backend.client);
    act(() => {
      view.dispatch(view.state.tr.insertText("Disconnected ", 1));
      backend.emit({ kind: "connection", state: "reconnecting", error: null });
    });
    await waitFor(() => expect(view.editable).toBe(false));
    const before = view.state.doc.toJSON();
    fireEvent.click(container.querySelector('input[type="checkbox"]')!);
    expect(view.state.doc.toJSON()).toEqual(before);
    await waitFor(() =>
      expect(native.draftCachePut).toHaveBeenCalledWith(
        expect.objectContaining({
          editorTasks: expect.any(Object),
          draft: expect.objectContaining({ title: "Disconnected Seed" }),
        }),
      ),
    );
    expect(backend.client.saveNoteEditor).not.toHaveBeenCalled();
    act(() =>
      backend.emit({ kind: "connection", state: "connected", error: null }),
    );
    await waitFor(
      () => expect(backend.client.saveNoteEditor).toHaveBeenCalledOnce(),
      { timeout: 3000 },
    );
    expect(
      backend.client.saveNoteEditor.mock.calls[0][1].tasks[0],
    ).toMatchObject({
      id: "stable-task",
      status: "in_progress",
      due_at: "2027-01-01",
    });
  });

  it("does not expose raw device image URLs or send scoped mentions to the local desktop router", async () => {
    const backend = fixture(true);
    backend.set(
      note(
        '# Seed\n\n<mention data-id="other" data-type="session" data-label="Other note"></mention>\n\n![Private](asset://localhost/private/secret.png)',
        "a",
      ),
    );
    const legacy = vi.fn();
    (window as any).__HYPR_NAVIGATE__ = legacy;
    try {
      const { container } = await openRich(backend.client);
      expect(container.querySelector("img")?.getAttribute("src")).toBeNull();
      fireEvent.click(container.querySelector('[data-mention="true"]')!);
      expect(legacy).not.toHaveBeenCalled();
      await waitFor(() =>
        expect(backend.client.readNoteEditor).toHaveBeenCalledWith(
          "other",
          expect.anything(),
        ),
      );
    } finally {
      delete (window as any).__HYPR_NAVIGATE__;
    }
  });

  it("keeps an existing Markdown recovery in its original editor instead of silently changing save contracts", async () => {
    const backend = richFixture();
    native.draftCacheGet.mockImplementation(async (_scope, id) => ({
      status: "ok",
      data:
        id === "same"
          ? {
              scope: backend.client.scope,
              sessionId: "same",
              baseline: note("Original", "a"),
              draft: { markdown: "Older recovery", title: "Seed" },
              uncertain: true,
              editorTasks: null,
            }
          : null,
    }));
    render(<VaultNotesWorkspace client={backend.client} />);
    fireEvent.click(await screen.findByRole("button", { name: "Seed" }));
    const editor = await screen.findByRole("textbox", { name: "Markdown" });
    expect(editor).toHaveProperty("value", "Older recovery");
    expect(native.views).toHaveLength(0);
    expect(backend.client.saveNoteEditor).not.toHaveBeenCalled();
  });

  it("reviews a conflicting remote note and discards the old editor's pending flush when choosing the server version", async () => {
    const backend = richFixture();
    const { view } = await openRich(backend.client);
    act(() => view.dispatch(view.state.tr.insertText("Mine ", 1)));
    backend.set({
      title: "Server",
      markdown: "# Server\n\n- [x] Changed remotely",
      revision: "remote-b",
    });
    backend.setTasks({
      revision: "remote-tasks",
      tasks: [
        {
          id: "remote-task",
          source_type: "session_raw_note",
          source_id: "same",
          source_order: 0,
          status: "done",
          text: "Changed remotely",
          body: [],
          due_at: "2027-02-01",
          assignee: "",
          created_at: "",
          updated_at: "",
        },
      ],
    });
    act(() =>
      backend.emit({ kind: "change", change: { kind: "resync_required" } }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "Use saved version" }),
    );
    await waitFor(() =>
      expect(
        native.views[native.views.length - 1].state.doc.textContent,
      ).toContain("Changed remotely"),
    );
    expect(
      native.views[native.views.length - 1].state.doc.textContent,
    ).not.toContain("Mine");
    expect(backend.client.saveNoteEditor).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Save" })).toHaveProperty(
      "disabled",
      true,
    );
  });

  it("keeps a reader's rich note and task checkboxes read-only without generating recovery writes", async () => {
    const backend = richFixture();
    const { view, container } = await openRich({
      ...backend.client,
      canEdit: false,
    });
    expect(view.editable).toBe(false);
    const before = view.state.doc.toJSON();
    fireEvent.click(container.querySelector('input[type="checkbox"]')!);
    expect(view.state.doc.toJSON()).toEqual(before);
    expect(backend.client.saveNoteEditor).not.toHaveBeenCalled();
    expect(native.draftCachePut).not.toHaveBeenCalled();
  });

  it("keeps and copies the latest rich draft when vault access is revoked before autosave", async () => {
    const backend = richFixture();
    const { view } = await openRich(backend.client);
    act(() => {
      view.dispatch(view.state.tr.insertText("Keep this ", 1));
      backend.emit({ kind: "connection", state: "access_denied", error: null });
    });
    fireEvent.click(
      await screen.findByRole("button", { name: "Copy unsaved text" }),
    );
    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith(
        expect.stringContaining("# Keep this Seed"),
      ),
    );
    await waitFor(() =>
      expect(native.draftCachePut).toHaveBeenCalledWith(
        expect.objectContaining({
          sessionId: noteEditorDraftId("same"),
          editorTasks: expect.any(Object),
          draft: expect.objectContaining({ title: "Keep this Seed" }),
        }),
      ),
    );
    expect(backend.client.saveNoteEditor).not.toHaveBeenCalled();
    expect(screen.queryByRole("textbox", { name: "Markdown" })).toBeNull();
  });

  it("does not repeat a manual save when the editor's debounce later flushes the same content", async () => {
    const backend = richFixture();
    const { view } = await openRich(backend.client);
    act(() => view.dispatch(view.state.tr.insertText("Manual ", 1)));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(backend.client.saveNoteEditor).toHaveBeenCalledOnce(),
    );
    await act(async () => {
      await new Promise((done) => setTimeout(done, 750));
    });
    expect(backend.client.saveNoteEditor).toHaveBeenCalledOnce();
    expect(screen.getByRole("button", { name: "Save" })).toHaveProperty(
      "disabled",
      true,
    );
  });

  it("recognizes a task-aware recovery whose lost save was already committed without sending another write", async () => {
    const backend = richFixture();
    const baseline = await backend.client.readNoteEditor("same");
    const committed = {
      ...baseline.note,
      title: "Recovered",
      markdown: "# Recovered\n\n- [x] Follow up",
      revision: "saved-b",
    };
    const tasks = baseline.tasks.tasks.map((task) => ({
      ...task,
      status: "done",
    }));
    backend.set(committed);
    backend.setTasks({ tasks, revision: "saved-tasks" });
    native.draftCacheGet.mockImplementation(async (_scope, id) => ({
      status: "ok",
      data:
        id === noteEditorDraftId("same")
          ? {
              scope: backend.client.scope,
              sessionId: id,
              baseline: baseline.note,
              draft: { title: committed.title, markdown: committed.markdown },
              uncertain: true,
              editorTasks: {
                baseline: baseline.tasks,
                draft: tasks.map(
                  ({ id, source_order, status, text, body, due_at }) => ({
                    id,
                    source_order,
                    status,
                    text,
                    body,
                    due_at,
                  }),
                ),
              },
            }
          : null,
    }));
    const { view } = await openRich(backend.client);
    expect(view.state.doc.textContent).toContain("Recovered");
    expect(screen.getByRole("button", { name: "Save" })).toHaveProperty(
      "disabled",
      true,
    );
    expect(backend.client.saveNoteEditor).not.toHaveBeenCalled();
  });
});

describe("shared note workspace", () => {
  it("persists and copies unsaved text while the browser is offline and vault access is lost", async () => {
    const { onlineManager } = await import("@tanstack/react-query");
    const backend = fixture();
    const editor = await open(backend.client);
    onlineManager.setOnline(false);
    try {
      fireEvent.change(editor, {
        target: { value: "Durable device text while offline" },
      });
      backend.emit({ kind: "connection", state: "reconnecting", error: null });
      await waitFor(() =>
        expect(native.draftCachePut).toHaveBeenCalledWith(
          expect.objectContaining({
            draft: expect.objectContaining({
              markdown: "Durable device text while offline",
            }),
          }),
        ),
      );
      backend.emit({ kind: "connection", state: "access_denied", error: null });
      fireEvent.click(
        await screen.findByRole("button", { name: "Copy unsaved text" }),
      );
      await waitFor(() =>
        expect(writeText).toHaveBeenCalledWith(
          "Durable device text while offline",
        ),
      );
      expect(backend.client.saveNote).not.toHaveBeenCalled();
    } finally {
      onlineManager.setOnline(true);
    }
  });

  it("saves a local note immediately without an internet connection", async () => {
    const { onlineManager } = await import("@tanstack/react-query");
    const backend = fixture();
    const editor = await open(backend.client);
    onlineManager.setOnline(false);
    try {
      fireEvent.change(editor, { target: { value: "Standalone local text" } });
      await waitFor(() =>
        expect(backend.client.saveNote).toHaveBeenCalledOnce(),
      );
      expect(backend.client.saveNote.mock.calls[0][1].markdown).toBe(
        "Standalone local text",
      );
    } finally {
      onlineManager.setOnline(true);
    }
  });

  it("reports a remote failure immediately instead of queuing a write until browser reconnection", async () => {
    const { onlineManager } = await import("@tanstack/react-query");
    const backend = fixture();
    backend.client.saveNote.mockRejectedValue(
      new ApplicationRequestError({
        code: "unavailable",
        message: "The server connection stopped",
        retryable: true,
        request_id: "remote-request",
        current_revision: null,
        mutation_outcome: "unknown",
      }),
    );
    const editor = await open({ ...backend.client, networkMode: "online" });
    onlineManager.setOnline(false);
    try {
      fireEvent.change(editor, {
        target: { value: "Keep my remote draft on this device" },
      });
      await waitFor(() =>
        expect(backend.client.saveNote).toHaveBeenCalledOnce(),
      );
      expect((await screen.findByRole("alert")).textContent).toContain(
        "The server connection stopped",
      );
      onlineManager.setOnline(true);
      await new Promise((resolve) => setTimeout(resolve, 600));
      expect(backend.client.saveNote).toHaveBeenCalledOnce();
    } finally {
      onlineManager.setOnline(true);
    }
  });

  it("durably marks a draft uncertain before sending its replacement", async () => {
    const backend = fixture();
    let finish!: (value: unknown) => void;
    backend.client.saveNote.mockReturnValueOnce(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    const editor = await open(backend.client);
    fireEvent.change(editor, { target: { value: "Pending during crash" } });
    await waitFor(
      () => expect(backend.client.saveNote).toHaveBeenCalledOnce(),
      { timeout: 3000 },
    );
    const prepared = native.draftCachePut.mock.calls.findIndex(
      ([value]) =>
        value.uncertain && value.draft.markdown === "Pending during crash",
    );
    expect(prepared).toBeGreaterThanOrEqual(0);
    expect(
      native.draftCachePut.mock.invocationCallOrder[prepared],
    ).toBeLessThan(backend.client.saveNote.mock.invocationCallOrder[0]);
    finish({ kind: "saved", note: note("Pending during crash", "b") });
    await screen.findByText("Saved");
  });

  it("cleans a recovered output whose exact body already committed without resending it", async () => {
    const backend = fixture();
    const key = documentDraftId("same", { kind: "summary" });
    backend.setDocument({
      document: {
        id: "same",
        session_id: "same",
        kind: "summary",
        title: "Summary",
        template_id: "",
        sort_order: 0,
        markdown: "Already committed",
      },
      revision: "doc-b",
    });
    native.draftCacheGet.mockImplementation(async (_scope, id) => ({
      status: "ok",
      data:
        id === key
          ? {
              scope: backend.client.scope,
              sessionId: key,
              baseline: {
                markdown: "Old summary",
                title: "",
                revision: "doc-a",
              },
              draft: { markdown: "Already committed", title: "" },
              uncertain: true,
            }
          : null,
    }));
    await open({
      ...backend.client,
      features: ["notes", "content_read", "document_edit"],
    });
    fireEvent.click(screen.getByRole("button", { name: "Outputs" }));
    await screen.findByText("Already committed");
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    await screen.findByRole("textbox", { name: "Markdown" });
    await screen.findByText("Saved");
    await waitFor(() =>
      expect(native.draftCacheRemove).toHaveBeenCalledWith(
        backend.client.scope,
        key,
      ),
    );
    expect(backend.client.saveDocument).not.toHaveBeenCalled();
    expect(
      screen.queryByRole("button", { name: "Save my version" }),
    ).toBeNull();
  });
  it("creates a missing summary only after the user edits", async () => {
    const backend = fixture();
    backend.setDocument({ document: null, revision: "absent" });
    await open({
      ...backend.client,
      features: ["notes", "content_read", "document_edit"],
    });
    fireEvent.click(screen.getByRole("button", { name: "Outputs" }));
    fireEvent.click(await screen.findByRole("button", { name: "Add summary" }));
    const editor = await screen.findByRole("textbox", { name: "Markdown" });
    expect(backend.client.saveDocument).not.toHaveBeenCalled();
    fireEvent.change(editor, { target: { value: "New summary" } });
    await waitFor(
      () => expect(backend.client.saveDocument).toHaveBeenCalledOnce(),
      { timeout: 3000 },
    );
    expect(backend.client.saveDocument.mock.calls[0][2]).toEqual({
      markdown: "New summary",
      expected_revision: "absent",
    });
  });

  it("flushes a pending scoped summary draft through its owning session before autosave", async () => {
    const backend = fixture();
    await open({
      ...backend.client,
      features: ["notes", "content_read", "document_edit"],
    });
    fireEvent.click(screen.getByRole("button", { name: "Outputs" }));
    await screen.findByText("Canonical summary");
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    const editor = await screen.findByRole("textbox", { name: "Markdown" });
    fireEvent.change(editor, { target: { value: "Owner-flushed summary" } });
    await act(async () => {
      await flushDatabaseWrites([
        sessionEditorFlushKey(backend.client.scope, "same"),
      ]);
    });
    expect(backend.client.saveDocument).toHaveBeenCalledOnce();
    expect(backend.client.saveDocument.mock.calls[0][2].markdown).toBe(
      "Owner-flushed summary",
    );
    expect(native.draftCachePut).toHaveBeenCalledWith(
      expect.objectContaining({
        scope: backend.client.scope,
        sessionId: documentDraftId("same", { kind: "summary" }),
      }),
    );
  });

  it("regenerates an existing output only after its first-500ms editor save confirms", async () => {
    const backend = fixture();
    backend.setDocument({
      document: {
        id: "output",
        session_id: "same",
        kind: "template_output",
        title: "Kept title",
        template_id: "kept-template",
        sort_order: 7,
        markdown: "Canonical output",
      },
      revision: "output-r0",
    });
    const save = backend.client.saveDocument.getMockImplementation()!;
    let release!: () => void;
    backend.client.saveDocument.mockImplementationOnce(async (...args) => {
      await new Promise<void>((resolve) => {
        release = resolve;
      });
      return save(...args);
    });
    await open({
      ...backend.client,
      features: [
        "notes",
        "content_read",
        "document_edit",
        "summary_body_jobs",
        "template_output_jobs",
      ],
    });
    fireEvent.click(screen.getByRole("button", { name: "Outputs" }));
    await screen.findByText("Canonical output");
    expect(screen.queryByRole("button", { name: "Regenerate" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    const editor = await screen.findByRole("textbox", { name: "Markdown" });
    const generate = await screen.findByRole("button", { name: "Regenerate" });
    await waitFor(() => expect(generate).toHaveProperty("disabled", false));
    fireEvent.change(editor, { target: { value: "Just typed output" } });
    expect(backend.client.saveDocument).not.toHaveBeenCalled();
    fireEvent.click(generate);
    await waitFor(() =>
      expect(backend.client.saveDocument).toHaveBeenCalledOnce(),
    );
    expect(native.deviceJobSubmissionPrepare).not.toHaveBeenCalled();
    expect(backend.client.submitSummaryJob).not.toHaveBeenCalled();
    expect((editor as HTMLTextAreaElement).value).toBe("Just typed output");
    release();
    await waitFor(() =>
      expect(backend.client.submitSummaryJob).toHaveBeenCalledOnce(),
    );
    expect(backend.client.saveDocument.mock.calls[0][2]).toEqual({
      markdown: "Just typed output",
      expected_revision: "output-r0",
    });
    expect(native.receipts[0].input).toEqual({
      session_id: "same",
      target: { kind: "template_output", document_id: "output" },
      expected_revision: "doc-1",
    });
    expect(
      backend.client.saveDocument.mock.invocationCallOrder[0],
    ).toBeLessThan(
      native.deviceJobSubmissionPrepare.mock.invocationCallOrder[0],
    );
    expect(backend.client.submitSummaryJob.mock.calls[0][0]).toEqual(
      native.receipts[0].input,
    );
    expect(backend.client.saveNote).not.toHaveBeenCalled();
    expect((editor as HTMLTextAreaElement).value).toBe("Just typed output");
  });

  it("keeps a target editor's lost-save recovery and skips generation instead of using the older body", async () => {
    const backend = fixture();
    backend.setDocument({
      document: {
        id: "output",
        session_id: "same",
        kind: "template_output",
        title: "Output",
        template_id: "template",
        sort_order: 2,
        markdown: "Canonical output",
      },
      revision: "output-r0",
    });
    backend.client.saveDocument.mockRejectedValueOnce(
      new Error("Output save reply was lost"),
    );
    await open({
      ...backend.client,
      features: [
        "notes",
        "content_read",
        "document_edit",
        "summary_body_jobs",
        "template_output_jobs",
      ],
    });
    fireEvent.click(screen.getByRole("button", { name: "Outputs" }));
    fireEvent.click(await screen.findByRole("button", { name: "Edit" }));
    const editor = await screen.findByRole("textbox", { name: "Markdown" });
    const generate = await screen.findByRole("button", { name: "Regenerate" });
    await waitFor(() => expect(generate).toHaveProperty("disabled", false));
    fireEvent.change(editor, { target: { value: "Preserved private output" } });
    fireEvent.click(generate);
    await waitFor(() =>
      expect(backend.client.saveDocument).toHaveBeenCalledOnce(),
    );
    await waitFor(() =>
      expect(
        screen.queryAllByText("Output save reply was lost").length,
      ).toBeGreaterThan(0),
    );
    expect(backend.client.submitSummaryJob).not.toHaveBeenCalled();
    expect(native.deviceJobSubmissionPrepare).not.toHaveBeenCalled();
    expect((editor as HTMLTextAreaElement).value).toBe(
      "Preserved private output",
    );
    expect(native.draftCachePut).toHaveBeenCalledWith(
      expect.objectContaining({
        scope: backend.client.scope,
        sessionId: documentDraftId("same", {
          kind: "template_output",
          document_id: "output",
        }),
        uncertain: true,
        draft: { markdown: "Preserved private output", title: "" },
      }),
    );
  });

  it("allows readers to inspect an existing output without editable fields", async () => {
    const backend = fixture();
    await open({
      ...backend.client,
      canEdit: false,
      features: ["notes", "content_read", "document_edit"],
    });
    fireEvent.click(screen.getByRole("button", { name: "Outputs" }));
    fireEvent.click(await screen.findByRole("button", { name: "View" }));
    const editor = await screen.findByRole("textbox", { name: "Markdown" });
    expect((editor as HTMLTextAreaElement).readOnly).toBe(true);
    expect(screen.getByRole("button", { name: "Save" })).toHaveProperty(
      "disabled",
      true,
    );
    expect(backend.client.saveDocument).not.toHaveBeenCalled();
  });

  it("reloads after a save reports deletion even when no change event arrives", async () => {
    const backend = fixture();
    const editor = await open(backend.client);
    const missing = new ApplicationRequestError({
      code: "not_found",
      message: "Session was deleted",
      retryable: false,
      request_id: null,
      current_revision: null,
      mutation_outcome: "not_committed",
    });
    backend.client.saveNote.mockRejectedValueOnce(missing);
    vi.mocked(backend.client.readNote).mockRejectedValue(missing);
    fireEvent.change(editor, {
      target: { value: "Retain this deletion draft" },
    });
    await waitFor(
      () => expect((editor as HTMLTextAreaElement).readOnly).toBe(true),
      { timeout: 3000 },
    );
    expect((editor as HTMLTextAreaElement).value).toBe(
      "Retain this deletion draft",
    );
    expect(backend.client.saveNote).toHaveBeenCalledOnce();
  });
  it("saves summary Markdown to its own revision when leaving before autosave", async () => {
    const backend = fixture();
    await open({
      ...backend.client,
      features: ["notes", "content_read", "document_edit"],
    });
    fireEvent.click(screen.getByRole("button", { name: "Outputs" }));
    await screen.findByText("Canonical summary");
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    const editor = await screen.findByRole("textbox", { name: "Markdown" });
    expect(screen.queryByRole("textbox", { name: "Title" })).toBeNull();
    fireEvent.change(editor, { target: { value: "Edited summary" } });
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    await waitFor(() =>
      expect(backend.client.saveDocument).toHaveBeenCalledOnce(),
    );
    expect(backend.client.saveDocument).toHaveBeenCalledWith(
      "same",
      { kind: "summary" },
      { markdown: "Edited summary", expected_revision: "doc-a" },
      undefined,
    );
    await screen.findByText("Edited summary");
    expect(backend.client.saveNote).not.toHaveBeenCalled();
    expect(native.draftCachePut).toHaveBeenCalledWith(
      expect.objectContaining({
        scope: backend.client.scope,
        sessionId: documentDraftId("same", { kind: "summary" }),
      }),
    );
  });

  it("reviews output conflicts and does not treat a metadata rename as an editable title", async () => {
    const backend = fixture();
    await open({
      ...backend.client,
      features: ["notes", "content_read", "document_edit"],
    });
    fireEvent.click(screen.getByRole("button", { name: "Outputs" }));
    await screen.findByText("Canonical summary");
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    const editor = await screen.findByRole("textbox", { name: "Markdown" });
    fireEvent.change(editor, { target: { value: "My summary" } });
    backend.setDocument({
      document: {
        id: "same",
        session_id: "same",
        kind: "summary",
        title: "Renamed elsewhere",
        template_id: "",
        sort_order: 0,
        markdown: "Other client text",
      },
      revision: "doc-b",
    });
    backend.emit({
      kind: "change",
      change: { kind: "index_changed", entity: "docs", ids: ["same"] },
    });
    await screen.findByText("Other client text");
    expect((editor as HTMLTextAreaElement).value).toBe("My summary");
    fireEvent.click(screen.getByRole("button", { name: "Save my version" }));
    await waitFor(() =>
      expect(backend.client.saveDocument).toHaveBeenCalledOnce(),
    );
    expect(backend.client.saveDocument.mock.calls[0][2]).toEqual({
      markdown: "My summary",
      expected_revision: "doc-b",
    });
    await screen.findByText("Saved");
    expect(screen.queryByRole("textbox", { name: "Title" })).toBeNull();
    expect(backend.client.saveNote).not.toHaveBeenCalled();
  });

  it("keeps a deleted output draft read-only and can copy it after access is revoked", async () => {
    const backend = fixture();
    backend.setDocument({
      document: {
        id: "output",
        session_id: "same",
        kind: "template_output",
        title: "Template",
        template_id: "template",
        sort_order: 1,
        markdown: "Template text",
      },
      revision: "out-a",
    });
    await open({
      ...backend.client,
      features: ["notes", "content_read", "document_edit"],
    });
    fireEvent.click(screen.getByRole("button", { name: "Outputs" }));
    await screen.findByText("Template text");
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    const editor = await screen.findByRole("textbox", { name: "Markdown" });
    fireEvent.change(editor, { target: { value: "Keep my output draft" } });
    backend.setDocument({ document: null, revision: "absent" });
    backend.emit({
      kind: "change",
      change: { kind: "index_changed", entity: "docs", ids: ["same"] },
    });
    await screen.findByText("This output no longer exists");
    expect((editor as HTMLTextAreaElement).readOnly).toBe(true);
    expect((editor as HTMLTextAreaElement).value).toBe("Keep my output draft");
    backend.emit({ kind: "connection", state: "access_denied", error: null });
    fireEvent.click(
      await screen.findByRole("button", { name: "Copy unsaved text" }),
    );
    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith("Keep my output draft"),
    );
    expect(backend.client.saveDocument).not.toHaveBeenCalled();
  });

  it("loads a device summary draft using the shared vault quota and original revision", async () => {
    const backend = fixture();
    const key = documentDraftId("same", { kind: "summary" });
    native.draftCacheGet.mockImplementation(async (_scope, id) => ({
      status: "ok",
      data:
        id === key
          ? {
              scope: backend.client.scope,
              sessionId: key,
              baseline: {
                markdown: "Canonical summary",
                title: "",
                revision: "doc-a",
              },
              draft: { markdown: "Recovered summary", title: "" },
              uncertain: false,
            }
          : null,
    }));
    await open({
      ...backend.client,
      features: ["notes", "content_read", "document_edit"],
    });
    fireEvent.click(screen.getByRole("button", { name: "Outputs" }));
    await screen.findByText("Canonical summary");
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    const editor = await screen.findByRole("textbox", { name: "Markdown" });
    expect((editor as HTMLTextAreaElement).value).toBe("Recovered summary");
    await waitFor(
      () => expect(backend.client.saveDocument).toHaveBeenCalledOnce(),
      { timeout: 3000 },
    );
    expect(backend.client.saveDocument.mock.calls[0][2]).toEqual({
      markdown: "Recovered summary",
      expected_revision: "doc-a",
    });
    await waitFor(() =>
      expect(native.draftCacheRemove).toHaveBeenCalledWith(
        backend.client.scope,
        key,
      ),
    );
  });
  it("saves a draft to its original session when navigating before the autosave delay", async () => {
    const backend = fixture();
    const page = await backend.client.sessions(100);
    vi.mocked(backend.client.sessions).mockResolvedValue({
      ...page,
      sessions: [
        ...page.sessions,
        { ...page.sessions[0], id: "other", title: "Other" },
      ],
    });
    const editor = await open(backend.client);
    fireEvent.change(editor, { target: { value: "Before switching" } });
    fireEvent.click(screen.getByRole("button", { name: "Other" }));
    await waitFor(() => expect(backend.client.saveNote).toHaveBeenCalledOnce());
    expect(backend.client.saveNote.mock.calls[0]).toMatchObject([
      "same",
      { markdown: "Before switching", expected_revision: "a" },
      undefined,
    ]);
  });

  it("finishes newer typing after leaving a note with an earlier save pending", async () => {
    const backend = fixture();
    const page = await backend.client.sessions(100);
    vi.mocked(backend.client.sessions).mockResolvedValue({
      ...page,
      sessions: [
        ...page.sessions,
        { ...page.sessions[0], id: "other", title: "Other" },
      ],
    });
    let finish!: (value: unknown) => void;
    backend.client.saveNote.mockReturnValueOnce(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    const editor = await open(backend.client);
    fireEvent.change(editor, { target: { value: "First edit" } });
    await waitFor(
      () => expect(backend.client.saveNote).toHaveBeenCalledOnce(),
      { timeout: 3000 },
    );
    fireEvent.change(editor, {
      target: { value: "Later typing before switching" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Other" }));
    finish({ kind: "saved", note: note("First edit", "b") });
    await waitFor(() =>
      expect(backend.client.saveNote).toHaveBeenCalledTimes(2),
    );
    expect(backend.client.saveNote.mock.calls[1]).toMatchObject([
      "same",
      { markdown: "Later typing before switching", expected_revision: "b" },
      undefined,
    ]);
  });

  it("reads existing outputs, transcripts and tasks through the same scoped client", async () => {
    const backend = fixture();
    await open({
      ...backend.client,
      features: ["notes", "search", "content_read"],
    });
    fireEvent.click(screen.getByRole("button", { name: "Outputs" }));
    await screen.findByText("Canonical summary");
    fireEvent.click(screen.getByRole("button", { name: "Transcript" }));
    await screen.findByText("Canonical transcript");
    fireEvent.click(screen.getByRole("button", { name: "Tasks" }));
    await screen.findByText("Canonical task", { exact: false });
    for (const operation of [
      backend.client.documents,
      backend.client.transcripts,
      backend.client.tasks,
    ])
      expect(operation).toHaveBeenCalledWith("same", expect.anything());
    expect(backend.client.saveNote).not.toHaveBeenCalled();
  });
  it("searches through the scoped client and revalidates after search readiness", async () => {
    const backend = fixture();
    backend.client.search.mockResolvedValue({
      count: 1,
      hits: [
        {
          document: {
            id: "found",
            title: "Found note",
            doc_type: "session",
            content: "",
            language: null,
            created_at: 0,
            facets: [],
          },
          score: 1,
          title_snippet: null,
          content_snippet: {
            fragment: '<img src="asset://localhost/private.png">',
            highlights: [],
          },
        },
      ],
    });
    await open(backend.client);
    fireEvent.change(screen.getByRole("textbox", { name: "Search notes" }), {
      target: { value: "find this" },
    });
    expect(backend.client.search).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    const hit = await screen.findByRole("button", { name: /Found note/ });
    expect(backend.client.search.mock.calls[0][0]).toMatchObject({
      query: "find this",
      limit: 50,
    });
    expect(document.querySelector("img")).toBeNull();
    fireEvent.click(hit);
    await waitFor(() =>
      expect(backend.client.readNote).toHaveBeenCalledWith(
        "found",
        expect.anything(),
      ),
    );
    backend.client.search.mockResolvedValue({ count: 0, hits: [] });
    backend.emit({ kind: "change", change: { kind: "search_ready" } });
    await screen.findByText("No matching notes");
    fireEvent.click(screen.getByRole("button", { name: "Clear" }));
    await screen.findByRole("button", { name: "Seed" });
  });

  it("hides recovery inventory and its private preview when vault authorization is lost", async () => {
    const backend = fixture();
    await open(backend.client);
    const entry = {
      id: "a".repeat(64),
      sessionId: "deleted-note",
      title: "Orphaned note",
      uncertain: false,
    };
    native.draftCacheInventory.mockResolvedValue({
      status: "ok",
      data: [entry],
    });
    native.draftCacheRecovery.mockResolvedValue({
      status: "ok",
      data: {
        scope: backend.client.scope,
        sessionId: entry.sessionId,
        baseline: note("Original", "a"),
        draft: { title: "Orphaned note", markdown: "Private orphaned draft" },
        uncertain: false,
      },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Device recovery drafts" }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: /Orphaned note/ }),
    );
    const preview = await screen.findByRole("textbox", {
      name: "Recovered Markdown",
    });
    expect((preview as HTMLTextAreaElement).value).toBe(
      "Private orphaned draft",
    );
    backend.emit({ kind: "connection", state: "access_denied", error: null });
    await screen.findByText("This connection is no longer authorized.");
    expect(
      screen.queryByRole("textbox", { name: "Recovered Markdown" }),
    ).toBeNull();
    expect(
      screen.queryByRole("button", { name: "Device recovery drafts" }),
    ).toBeNull();
    expect(backend.client.saveNote).not.toHaveBeenCalled();
    expect(backend.client.saveDocument).not.toHaveBeenCalled();
  });

  it("keeps a deleted note's draft visible and read-only without retrying a save", async () => {
    const backend = fixture();
    const editor = await open(backend.client);
    fireEvent.change(editor, { target: { value: "Keep this deleted draft" } });
    vi.mocked(backend.client.readNote).mockRejectedValue(
      new ApplicationRequestError({
        code: "not_found",
        message: "This note no longer exists",
        retryable: false,
        request_id: null,
        current_revision: null,
        mutation_outcome: "not_committed",
      }),
    );
    backend.emit({
      kind: "change",
      change: { kind: "index_changed", entity: "sessions", ids: ["same"] },
    });
    await screen.findByText("This note no longer exists");
    expect((editor as HTMLTextAreaElement).readOnly).toBe(true);
    expect((editor as HTMLTextAreaElement).value).toBe(
      "Keep this deleted draft",
    );
    await waitFor(() => expect(native.draftCachePut).toHaveBeenCalled());
    expect(backend.client.saveNote).not.toHaveBeenCalled();
  });

  it("opens a recovered draft read-only when the authoritative note is missing", async () => {
    const backend = fixture();
    native.draftCacheGet.mockResolvedValue({
      status: "ok",
      data: {
        scope: backend.client.scope,
        sessionId: "same",
        baseline: note("Original", "a"),
        draft: { markdown: "Recovered after restart", title: "Seed" },
        uncertain: false,
      },
    });
    vi.mocked(backend.client.readNote).mockRejectedValue(
      new ApplicationRequestError({
        code: "not_found",
        message: "This note no longer exists",
        retryable: false,
        request_id: null,
        current_revision: null,
        mutation_outcome: "not_committed",
      }),
    );
    const editor = await open(backend.client);
    await screen.findByText("This note no longer exists");
    expect((editor as HTMLTextAreaElement).readOnly).toBe(true);
    expect((editor as HTMLTextAreaElement).value).toBe(
      "Recovered after restart",
    );
    expect(backend.client.saveNote).not.toHaveBeenCalled();
  });

  it("autosaves through the supplied client and keeps newer typing during an earlier request", async () => {
    const backend = fixture();
    let finish!: (value: unknown) => void;
    backend.client.saveNote.mockReturnValueOnce(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    const editor = await open(backend.client);
    fireEvent.change(editor, { target: { value: "First edit" } });
    await waitFor(
      () => expect(backend.client.saveNote).toHaveBeenCalledTimes(1),
      { timeout: 3000 },
    );
    expect(backend.client.saveNote.mock.calls[0][1].expected_revision).toBe(
      "a",
    );
    fireEvent.change(editor, { target: { value: "Later typing" } });
    finish({ kind: "saved", note: note("First edit", "b") });
    await waitFor(() =>
      expect((editor as HTMLTextAreaElement).value).toBe("Later typing"),
    );
    await waitFor(
      () => expect(backend.client.saveNote).toHaveBeenCalledTimes(2),
      { timeout: 3000 },
    );
    expect(backend.client.saveNote.mock.calls[1][1]).toMatchObject({
      markdown: "Later typing",
      expected_revision: "b",
    });
    expect(native.draftCachePut).toHaveBeenCalled();
  });

  it("retains the original revision when another client changes a dirty note", async () => {
    const backend = fixture();
    const editor = await open(backend.client);
    fireEvent.change(editor, { target: { value: "My text" } });
    backend.set(note("Another client", "c"));
    backend.emit({
      kind: "change",
      change: { kind: "index_changed", entity: "sessions", ids: ["same"] },
    });
    await screen.findByRole("button", { name: "Use saved version" });
    expect((editor as HTMLTextAreaElement).value).toBe("My text");
    expect(backend.client.saveNote).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Save my version" }));
    await waitFor(() => expect(backend.client.saveNote).toHaveBeenCalledOnce());
    expect(backend.client.saveNote.mock.calls[0][1].expected_revision).toBe(
      "c",
    );
  });

  it("disables editing during a disconnect while persisting the unsaved text", async () => {
    const backend = fixture();
    const editor = await open(backend.client);
    fireEvent.change(editor, { target: { value: "Text before outage" } });
    backend.emit({ kind: "connection", state: "reconnecting", error: null });
    await waitFor(() =>
      expect((editor as HTMLTextAreaElement).readOnly).toBe(true),
    );
    await waitFor(() => expect(native.draftCachePut).toHaveBeenCalled(), {
      timeout: 3000,
    });
    expect(
      native.draftCachePut.mock.calls[
        native.draftCachePut.mock.calls.length - 1
      ]?.[0],
    ).toMatchObject({
      scope: backend.client.scope,
      draft: { markdown: "Text before outage" },
      baseline: { revision: "a" },
    });
    expect(backend.client.saveNote).not.toHaveBeenCalled();
  });

  it("clears the private view after access loss and renders asset-like text without loading it", async () => {
    const backend = fixture();
    backend.set(
      note("![private](asset://localhost/Users/private/file.png)", "a"),
    );
    const editor = await open(backend.client);
    expect(document.querySelector("img")).toBeNull();
    fireEvent.change(editor, {
      target: { value: "Unsaved text before access loss" },
    });
    backend.emit({ kind: "connection", state: "access_denied", error: null });
    await screen.findByText("This connection is no longer authorized.");
    expect(screen.queryByRole("textbox", { name: "Markdown" })).toBeNull();
    expect(backend.client.saveNote).not.toHaveBeenCalled();
  });

  it("preserves recovery data for readers and does not send an unauthorized mutation", async () => {
    const backend = fixture();
    const reader = { ...backend.client, canEdit: false };
    const editor = await open(reader);
    expect((editor as HTMLTextAreaElement).readOnly).toBe(true);
    expect(
      (screen.getByRole("button", { name: "New note" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    expect(backend.client.saveNote).not.toHaveBeenCalled();
  });

  it("stops automatic retries after a rejected save until the user edits again", async () => {
    const backend = fixture();
    backend.client.saveNote.mockRejectedValue(
      new ApplicationRequestError({
        code: "resource_limit",
        message: "Note too large",
        retryable: false,
        request_id: null,
        current_revision: null,
        mutation_outcome: "not_committed",
      }),
    );
    const editor = await open(backend.client);
    fireEvent.change(editor, { target: { value: "Rejected" } });
    await screen.findByText("Note too large", {}, { timeout: 3000 });
    await new Promise((resolve) => setTimeout(resolve, 700));
    expect(backend.client.saveNote).toHaveBeenCalledOnce();
  });
});

it("mounts owned attachments through the actual shared note host and saves portable references", async () => {
  const backend = fixture(true);
  backend.set(note("# Seed\n\n![Owned image](attachments/chart.png)", "a"));
  const close = vi.fn().mockResolvedValue(undefined);
  backend.client.openAttachment.mockResolvedValue({
    id: "opaque-chart",
    info: { filename: "chart.png", contentType: "image/png", sizeBytes: 42 },
    src: "loofah-media://localhost/opaque-chart",
    labels: {
      open: { id: "open", message: "Open downloaded copy" },
      reveal: { id: "reveal", message: "Show downloaded copy" },
    },
    open: vi.fn(),
    reveal: vi.fn(),
    close,
  });
  const { view, unmount } = await openRich(backend.client);
  await waitFor(() =>
    expect(screen.getByAltText("Owned image").getAttribute("src")).toBe(
      "loofah-media://localhost/opaque-chart",
    ),
  );
  expect(backend.client.openAttachment).toHaveBeenCalledWith(
    "same",
    "chart.png",
    expect.objectContaining({ signal: expect.any(AbortSignal) }),
  );
  act(() => view.dispatch(view.state.tr.insertText(" edited", 5)));
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() =>
    expect(backend.client.saveNoteEditor).toHaveBeenCalledTimes(1),
  );
  const saved = backend.client.saveNoteEditor.mock.calls[0][1];
  expect(saved.markdown).toContain("attachments/chart.png");
  expect(saved.markdown).not.toContain("loofah-media:");
  unmount();
  await waitFor(() => expect(close).toHaveBeenCalled());
});

it("opens the attachment panel through the shared workspace and defers file actions until expanded", async () => {
  const backend = fixture(true);
  Object.assign(backend.client, {
    features: [...backend.client.features, "content_read", "attachment_list"],
  });
  backend.client.attachments.mockResolvedValue({
    files: [
      {
        filename: "report.pdf",
        contentType: "application/pdf",
        sizeBytes: 2048,
      },
    ],
    next_cursor: null,
  });
  const open = vi.fn().mockResolvedValue(undefined);
  const closed: ReturnType<typeof vi.fn>[] = [];
  backend.client.openAttachment.mockImplementation(async () => {
    const close = vi.fn().mockResolvedValue(undefined);
    closed.push(close);
    return {
      id: crypto.randomUUID(),
      info: {
        filename: "report.pdf",
        contentType: "application/pdf",
        sizeBytes: 2048,
      },
      labels: {
        open: { id: "open", message: "Open downloaded copy" },
        reveal: { id: "show", message: "Show downloaded copy" },
      },
      open,
      reveal: vi.fn(),
      close,
    };
  });
  const { unmount } = await openRich(backend.client);
  fireEvent.click(screen.getByRole("button", { name: "Attachments" }));
  const row = await screen.findByRole("button", {
    name: "Actions for report.pdf",
  });
  expect(backend.client.attachments).toHaveBeenCalledWith(
    "same",
    100,
    undefined,
    expect.objectContaining({ signal: expect.any(AbortSignal) }),
  );
  expect(backend.client.openAttachment).not.toHaveBeenCalled();
  fireEvent.click(row);
  fireEvent.click(
    await screen.findByRole("button", { name: "Open downloaded copy" }),
  );
  await waitFor(() => expect(open).toHaveBeenCalledTimes(1));
  unmount();
  await waitFor(() => {
    for (const close of closed) expect(close).toHaveBeenCalledTimes(1);
  });
});
