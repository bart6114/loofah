import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  draftCacheGet: vi.fn(),
  draftCachePut: vi.fn(),
  draftCacheRemove: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({ commands: native }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));

import { writeText } from "@tauri-apps/plugin-clipboard-manager";

import { documentDraftId } from "./document-draft";
import { ApplicationRequestError } from "./errors";
import { VaultNotesWorkspace } from "./notes-workspace";
import type { NoteSnapshot, DocumentSnapshot } from "./protocol.gen";
import type { VaultClient, VaultClientEvent } from "./vault-client";

import { flushDatabaseWrites } from "~/shared/write-queue";

const note = (markdown: string, revision: string): NoteSnapshot => ({
  markdown,
  title: "Seed",
  revision,
});
function fixture() {
  const listeners = new Set<(event: VaultClientEvent) => void>();
  let current = note("Original", "a");
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
    features: ["notes", "search"],
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
    saveDocument: ReturnType<typeof vi.fn>;
    search: ReturnType<typeof vi.fn>;
  };
  return {
    client,
    set: (value: NoteSnapshot) => {
      current = value;
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
  native.draftCacheGet.mockResolvedValue({ status: "ok", data: null });
  native.draftCachePut.mockResolvedValue({ status: "ok", data: undefined });
  native.draftCacheRemove.mockResolvedValue({ status: "ok", data: undefined });
});
afterEach(async () => {
  cleanup();
  await flushDatabaseWrites();
});

describe("shared note workspace", () => {
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
    await screen.findByRole("button", { name: "Use server version" });
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
