import { describe, expect, it, vi } from "vitest";

import { ApplicationRequestError } from "./errors";
import { NoteConflictError, NoteDraft, NoteDrafts } from "./note-draft";
import type { NoteSnapshot } from "./protocol.gen";
import type { VaultClient } from "./vault-client";

const note = (markdown: string, revision: string): NoteSnapshot => ({
  markdown,
  title: "Title",
  revision,
});
const client = (scope = "server:alice:vault") =>
  ({ scope, canEdit: true, saveNote: vi.fn() }) as unknown as VaultClient & {
    saveNote: ReturnType<typeof vi.fn>;
  };
function deferred<T>() {
  let resolve!: (result: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

describe("revision-aware note drafts", () => {
  it("pins the edit baseline when a background query observes another writer", async () => {
    const backend = client();
    const draft = new NoteDraft(backend, "note", note("Original", "a"));
    draft.edit("My changes", "Title");
    draft.receive(note("Another writer", "b"));
    expect(draft.getSnapshot().baseline.revision).toBe("a");
    expect(draft.getSnapshot().draft.markdown).toBe("My changes");
    await expect(draft.save()).rejects.toBeInstanceOf(NoteConflictError);
    expect(backend.saveNote).not.toHaveBeenCalled();
  });

  it("serializes saves and advances revisions only after its own successful write", async () => {
    const backend = client();
    const first = deferred<unknown>();
    backend.saveNote
      .mockReturnValueOnce(first.promise)
      .mockResolvedValueOnce({ kind: "saved", note: note("Second", "c") });
    const draft = new NoteDraft(backend, "note", note("Original", "a"));
    draft.edit("First", "Title");
    const saving = draft.save();
    draft.edit("Second", "Title");
    const queued = draft.save();
    await vi.waitFor(() => expect(backend.saveNote).toHaveBeenCalledTimes(1));
    expect(backend.saveNote.mock.calls[0][1].expected_revision).toBe("a");
    first.resolve({ kind: "saved", note: note("First", "b") });
    await saving;
    await queued;
    expect(backend.saveNote.mock.calls[1][1]).toMatchObject({
      markdown: "Second",
      expected_revision: "b",
    });
    expect(draft.getSnapshot()).toMatchObject({
      dirty: false,
      baseline: { revision: "c" },
      draft: { markdown: "Second" },
    });
  });

  it("preserves newer typing while an earlier save is in flight", async () => {
    const backend = client();
    const result = deferred<unknown>();
    backend.saveNote.mockReturnValueOnce(result.promise);
    const draft = new NoteDraft(backend, "note", note("Original", "a"));
    draft.edit("First", "Title");
    const saving = draft.save();
    await vi.waitFor(() => expect(backend.saveNote).toHaveBeenCalledTimes(1));
    draft.edit("Later typing", "Title");
    result.resolve({ kind: "saved", note: note("First", "b") });
    await saving;
    expect(draft.getSnapshot()).toMatchObject({
      dirty: true,
      baseline: { revision: "b" },
      draft: { markdown: "Later typing" },
    });
  });

  it("does not let queued autosaves replace the conflicting server document", async () => {
    const backend = client();
    backend.saveNote.mockResolvedValue({
      kind: "conflict",
      note: note("Someone else", "b"),
    });
    const draft = new NoteDraft(backend, "note", note("Original", "a"));
    draft.edit("Mine", "Title");
    const first = draft.save();
    draft.edit("More typing", "Title");
    const next = draft.save();
    const results = await Promise.allSettled([first, next]);
    expect(results.every((result) => result.status === "rejected")).toBe(true);
    expect(backend.saveNote).toHaveBeenCalledTimes(1);
    expect(draft.getSnapshot()).toMatchObject({
      baseline: { revision: "a" },
      draft: { markdown: "More typing" },
      conflict: { revision: "b" },
    });
  });

  it("requires the reviewed version for an explicit keep-draft replacement and still uses CAS", async () => {
    const backend = client();
    backend.saveNote.mockResolvedValue({
      kind: "saved",
      note: note("Mine", "c"),
    });
    const draft = new NoteDraft(backend, "note", note("Original", "a"));
    draft.edit("Mine", "Title");
    draft.receive(note("Other", "b"));
    expect(() => draft.keepDraft("stale-review")).toThrow("latest server");
    draft.keepDraft("b");
    await draft.save();
    expect(backend.saveNote.mock.calls[0][1].expected_revision).toBe("b");
    expect(draft.getSnapshot().dirty).toBe(false);
  });

  it("can explicitly replace the draft with the reviewed server document", () => {
    const draft = new NoteDraft(client(), "note", note("Original", "a"));
    draft.edit("Mine", "Title");
    draft.receive(note("Other", "b"));
    draft.useServerVersion("b");
    expect(draft.getSnapshot()).toMatchObject({
      dirty: false,
      draft: { markdown: "Other" },
      conflict: null,
    });
  });

  it("keeps the baseline after a lost response and recognizes an exact retry match", async () => {
    const backend = client();
    backend.saveNote
      .mockRejectedValueOnce(
        new ApplicationRequestError({
          code: "unavailable",
          message: "Connection lost",
          retryable: true,
          request_id: "request",
          current_revision: null,
          mutation_outcome: "unknown",
        }),
      )
      .mockResolvedValueOnce({ kind: "conflict", note: note("Mine", "b") });
    const draft = new NoteDraft(backend, "note", note("Original", "a"));
    draft.edit("Mine", "Title");
    await expect(draft.save()).rejects.toThrow("Connection lost");
    expect(draft.getSnapshot()).toMatchObject({
      dirty: true,
      baseline: { revision: "a" },
    });
    await draft.save();
    expect(backend.saveNote.mock.calls[1][1].expected_revision).toBe("a");
    expect(draft.getSnapshot()).toMatchObject({
      dirty: false,
      baseline: { revision: "b" },
    });
  });

  it("does not acknowledge an absent note as an exact retry of an explicit empty file", async () => {
    const backend = client();
    backend.saveNote.mockResolvedValue({
      kind: "conflict",
      note: { markdown: null, title: "Title", revision: "b" },
    });
    const draft = new NoteDraft(backend, "note", note("Original", "a"));
    draft.edit("", "Title");
    await expect(draft.save()).rejects.toBeInstanceOf(NoteConflictError);
  });

  it("keeps pending writes pinned to their original client while a scope reconnects", async () => {
    const first = client();
    const replacement = client();
    const result = deferred<unknown>();
    first.saveNote
      .mockReturnValueOnce(result.promise)
      .mockResolvedValueOnce({ kind: "saved", note: note("Second", "c") });
    const draft = new NoteDraft(first, "note", note("Original", "a"));
    draft.edit("First", "Title");
    const saving = draft.save();
    draft.edit("Second", "Title");
    const queued = draft.save();
    draft.rebind(replacement);
    result.resolve({ kind: "saved", note: note("First", "b") });
    await saving;
    await queued;
    expect(first.saveNote).toHaveBeenCalledTimes(2);
    expect(replacement.saveNote).not.toHaveBeenCalled();
    expect(() => draft.rebind(client("server:bob:vault"))).toThrow(
      "different account",
    );
  });

  it("retains drafts separately for identical session IDs in different vaults and accounts", () => {
    const drafts = new NoteDrafts();
    const alice = client();
    const bob = client("server:bob:vault");
    const other = client("server:alice:other");
    const first = drafts.open(alice, "same", note("Original", "a"));
    first.edit("Private draft", "Title");
    expect(
      drafts.open(bob, "same", note("Bob", "b")).getSnapshot().draft.markdown,
    ).toBe("Bob");
    expect(
      drafts.open(other, "same", note("Other", "c")).getSnapshot().draft
        .markdown,
    ).toBe("Other");
    expect(
      drafts.open(alice, "same", note("Changed", "d")).getSnapshot().draft
        .markdown,
    ).toBe("Private draft");
    const restored = new NoteDrafts().restore(
      alice,
      JSON.parse(JSON.stringify(first.recovery())),
    );
    expect(restored.getSnapshot()).toMatchObject({
      dirty: true,
      baseline: { revision: "a" },
      draft: { markdown: "Private draft" },
    });
    expect(() => drafts.restore(bob, first.recovery())).toThrow(
      "different account",
    );
  });
  it("retains an undo to the old baseline while the earlier save is still in flight", async () => {
    const backend = client();
    const first = deferred<unknown>();
    backend.saveNote
      .mockReturnValueOnce(first.promise)
      .mockResolvedValueOnce({ kind: "saved", note: note("Original", "c") });
    const draft = new NoteDraft(backend, "note", note("Original", "a"));
    draft.edit("Changed", "Title");
    const saving = draft.save();
    await vi.waitFor(() => expect(backend.saveNote).toHaveBeenCalledTimes(1));
    draft.edit("Original", "Title");
    const undo = draft.save();
    draft.receive(note("Changed", "b"));
    expect(draft.getSnapshot().draft.markdown).toBe("Original");
    first.resolve({ kind: "saved", note: note("Changed", "b") });
    await saving;
    await undo;
    expect(backend.saveNote.mock.calls[1][1]).toMatchObject({
      markdown: "Original",
      expected_revision: "b",
    });
  });

  it("does not revive discarded drafts through an old response or queued autosave", async () => {
    const backend = client();
    const response = deferred<unknown>();
    backend.saveNote.mockReturnValueOnce(response.promise);
    const draft = new NoteDraft(backend, "note", note("Original", "a"));
    draft.edit("Mine", "Title");
    const saving = draft.save();
    await vi.waitFor(() => expect(backend.saveNote).toHaveBeenCalledTimes(1));
    draft.edit("More typing", "Title");
    const queued = draft.save();
    draft.receive(note("Server", "c"));
    draft.useServerVersion("c");
    const settling = Promise.allSettled([saving, queued]);
    response.resolve({ kind: "saved", note: note("Mine", "b") });
    await settling;
    expect(backend.saveNote).toHaveBeenCalledTimes(1);
    expect(draft.getSnapshot()).toMatchObject({
      dirty: false,
      baseline: { revision: "c" },
      draft: { markdown: "Server" },
    });
  });

  it("retains uncertain intent even when a user undid to the prior baseline", async () => {
    const backend = client();
    const response = deferred<unknown>();
    backend.saveNote.mockReturnValueOnce(response.promise);
    const draft = new NoteDraft(backend, "note", note("Original", "a"));
    draft.edit("Changed", "Title");
    const saving = draft.save();
    await vi.waitFor(() => expect(backend.saveNote).toHaveBeenCalledTimes(1));
    draft.edit("Original", "Title");
    response.reject(new Error("Connection interrupted"));
    await expect(saving).rejects.toThrow();
    const restored = new NoteDrafts().restore(backend, draft.recovery());
    restored.receive(note("Changed", "b"));
    expect(restored.getSnapshot()).toMatchObject({
      dirty: true,
      baseline: { revision: "a" },
      draft: { markdown: "Original" },
      conflict: { revision: "b" },
    });
  });

  it("refuses a reader mutation while keeping their recovery draft intact", async () => {
    const backend = { ...client(), canEdit: false };
    const draft = new NoteDraft(backend, "note", note("Original", "a"));
    draft.edit("Recovered text", "Title");
    await expect(draft.save()).rejects.toMatchObject({
      detail: { code: "forbidden", mutation_outcome: "not_committed" },
    });
    expect(backend.saveNote).not.toHaveBeenCalled();
    expect(draft.getSnapshot().draft.markdown).toBe("Recovered text");
  });
  it("pins the baseline on the first keystroke while deferring Markdown serialization", async () => {
    const backend = client();
    backend.saveNote.mockResolvedValue({
      kind: "saved",
      note: note("Latest typed text", "b"),
    });
    const draft = new NoteDraft(backend, "note", note("Original", "a"));
    const serialize = vi.fn(() => ({
      markdown: "Latest typed text",
      title: "Title",
    }));
    draft.capture(serialize);
    expect(serialize).not.toHaveBeenCalled();
    expect(draft.getSnapshot().dirty).toBe(true);
    draft.receive(note("Another writer", "c"));
    expect(draft.getSnapshot().baseline.revision).toBe("a");
    expect(serialize).not.toHaveBeenCalled();
    expect(draft.recovery().draft.markdown).toBe("Latest typed text");
    expect(serialize).toHaveBeenCalledOnce();
    await expect(draft.save()).rejects.toBeInstanceOf(NoteConflictError);
    expect(backend.saveNote).not.toHaveBeenCalled();
  });
  it("acknowledges a lost response when an authoritative reconnect snapshot exactly matches", async () => {
    const backend = client();
    backend.saveNote.mockRejectedValueOnce(new Error("Response lost"));
    const draft = new NoteDraft(backend, "note", note("Original", "a"));
    draft.edit("Mine", "Title");
    await expect(draft.save()).rejects.toThrow("Response lost");
    const restored = new NoteDrafts().restore(backend, draft.recovery());
    restored.receive(note("Mine", "b"));
    expect(restored.getSnapshot()).toMatchObject({
      dirty: false,
      baseline: { revision: "b" },
      conflict: null,
    });
    expect(backend.saveNote).toHaveBeenCalledOnce();
  });
});
