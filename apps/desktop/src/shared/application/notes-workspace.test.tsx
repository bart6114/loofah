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

import { ApplicationRequestError } from "./errors";
import { VaultNotesWorkspace } from "./notes-workspace";
import type { NoteSnapshot } from "./protocol.gen";
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
  const client = {
    scope: crypto.randomUUID(),
    networkMode: "always",
    canEdit: true,
    features: ["notes", "search"],
    documents: vi.fn(async () => [
      {
        id: "output",
        session_id: "same",
        kind: "summary",
        title: "Summary",
        template_id: "",
        sort_order: 0,
        markdown: "Canonical summary",
      },
    ]),
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
    search: ReturnType<typeof vi.fn>;
  };
  return {
    client,
    set: (value: NoteSnapshot) => {
      current = value;
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
