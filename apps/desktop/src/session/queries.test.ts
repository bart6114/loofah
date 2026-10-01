import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  sessionGet: vi.fn(
    (): Promise<
      | { status: "ok"; data: Record<string, unknown> | null }
      | { status: "error"; error: string }
    > => Promise.resolve({ status: "ok", data: null }),
  ),
  sessionIsEmpty: vi.fn(
    (): Promise<
      { status: "ok"; data: boolean } | { status: "error"; error: string }
    > => Promise.resolve({ status: "ok", data: true }),
  ),
  sessionWriteMeta: vi.fn(
    (): Promise<
      { status: "ok"; data: null } | { status: "error"; error: string }
    > => Promise.resolve({ status: "ok", data: null }),
  ),
  sessionUpdateMeta: vi.fn(
    (): Promise<
      { status: "ok"; data: null } | { status: "error"; error: string }
    > => Promise.resolve({ status: "ok", data: null }),
  ),
  sessionSaveNote: vi.fn(
    (): Promise<
      { status: "ok"; data: null } | { status: "error"; error: string }
    > => Promise.resolve({ status: "ok", data: null }),
  ),
  sessionWriteNote: vi.fn(
    (): Promise<
      { status: "ok"; data: null } | { status: "error"; error: string }
    > => Promise.resolve({ status: "ok", data: null }),
  ),
  sessionDelete: vi.fn(
    (): Promise<
      { status: "ok"; data: null } | { status: "error"; error: string }
    > => Promise.resolve({ status: "ok", data: null }),
  ),
  sessionRestore: vi.fn(
    (): Promise<
      { status: "ok"; data: boolean } | { status: "error"; error: string }
    > => Promise.resolve({ status: "ok", data: true }),
  ),
  sessionSaveEnhancedDoc: vi.fn(
    (): Promise<
      { status: "ok"; data: null } | { status: "error"; error: string }
    > => Promise.resolve({ status: "ok", data: null }),
  ),
  sessionDeleteEnhancedDoc: vi.fn(
    (): Promise<
      { status: "ok"; data: null } | { status: "error"; error: string }
    > => Promise.resolve({ status: "ok", data: null }),
  ),
  sessionSaveSummary: vi.fn(async () => ({ status: "ok", data: null })),
  sessionDeleteSummary: vi.fn(async () => ({ status: "ok", data: null })),
  waitForPendingSoftDelete: vi.fn(() => Promise.resolve()),
}));

vi.mock("~/session/pending-soft-deletes", () => ({
  sessionSaveSummary: vi.fn(async () => ({ status: "ok", data: null })),
  sessionDeleteSummary: vi.fn(async () => ({ status: "ok", data: null })),
  waitForPendingSoftDelete: mocks.waitForPendingSoftDelete,
}));

vi.mock("~/types/tauri.gen", () => ({
  commands: {
    sessionGet: mocks.sessionGet,
    sessionSaveSummary: mocks.sessionSaveSummary,
    sessionDeleteSummary: mocks.sessionDeleteSummary,
    sessionIsEmpty: mocks.sessionIsEmpty,
    sessionWriteMeta: mocks.sessionWriteMeta,
    sessionUpdateMeta: mocks.sessionUpdateMeta,
    sessionWriteNote: mocks.sessionWriteNote,
    sessionSaveNote: mocks.sessionSaveNote,
    sessionDelete: mocks.sessionDelete,
    sessionRestore: mocks.sessionRestore,
    sessionSaveEnhancedDoc: mocks.sessionSaveEnhancedDoc,
    sessionDeleteEnhancedDoc: mocks.sessionDeleteEnhancedDoc,
  },
}));

import {
  createSession,
  deleteEnhancedNote,
  isSessionEmpty,
  restoreDeletedSession,
  softDeleteSession,
  saveSessionNote,
  updateEnhancedNoteContent,
  updateSession,
} from "./queries";

describe("session store operations", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.useRealTimers();
    mocks.sessionGet.mockResolvedValue({ status: "ok", data: null });
    mocks.sessionIsEmpty.mockResolvedValue({ status: "ok", data: true });
    mocks.sessionWriteMeta.mockResolvedValue({ status: "ok", data: null });
    mocks.sessionUpdateMeta.mockResolvedValue({ status: "ok", data: null });
    mocks.sessionWriteNote.mockResolvedValue({ status: "ok", data: null });
    mocks.sessionDelete.mockResolvedValue({ status: "ok", data: null });
    mocks.sessionRestore.mockResolvedValue({ status: "ok", data: true });
    mocks.waitForPendingSoftDelete.mockResolvedValue(undefined);
  });

  it("routes title changes through the store's meta patch, never raw SQL", async () => {
    await updateSession("session-1", { title: "Updated title" });

    expect(mocks.sessionUpdateMeta).toHaveBeenCalledWith("session-1", {
      title: "Updated title",
    });
  });

  it("saves raw note content and its title with one store command", async () => {
    await saveSessionNote(
      "session-1",
      "# Edited title\n\nBody",
      "Edited title",
    );
    expect(mocks.sessionSaveNote).toHaveBeenCalledWith(
      "session-1",
      "# Edited title\n\nBody",
      "Edited title",
    );
    expect(mocks.sessionWriteNote).not.toHaveBeenCalled();
    expect(mocks.sessionUpdateMeta).not.toHaveBeenCalled();
  });

  it("preserves metadata when no derived raw note title is supplied", async () => {
    await saveSessionNote("session-1", "Body");
    expect(mocks.sessionSaveNote).toHaveBeenCalledWith(
      "session-1",
      "Body",
      null,
    );
  });

  it("propagates a rejected coherent raw note save", async () => {
    mocks.sessionSaveNote.mockResolvedValueOnce({
      status: "error",
      error: "disk full",
    });
    await expect(saveSessionNote("session-1", "Body")).rejects.toThrow(
      "disk full",
    );
  });

  it("maps folder changes onto the store patch shape", async () => {
    await updateSession("session-1", {
      folder_id: "work",
    });

    expect(mocks.sessionUpdateMeta).toHaveBeenCalledWith("session-1", {
      folder: "work",
    });
  });

  it("throws when the store rejects the meta patch", async () => {
    mocks.sessionUpdateMeta.mockResolvedValueOnce({
      status: "error",
      error: "no _meta.json",
    });

    await expect(
      updateSession("session-1", { title: "Updated title" }),
    ).rejects.toThrow("no _meta.json");
  });

  it("is a no-op when there is nothing to change", async () => {
    await updateSession("session-1", {});
    expect(mocks.sessionUpdateMeta).not.toHaveBeenCalled();
  });

  it("creates a session via the store, then seeds initial content as markdown", async () => {
    await createSession("Welcome", "user-1", {
      tracking_id: "welcome",
      raw_md: JSON.stringify({
        type: "doc",
        content: [
          { type: "paragraph", content: [{ type: "text", text: "Hi" }] },
        ],
      }),
    });

    // The marker rides the store write itself, on the meta directly.
    expect(mocks.sessionWriteMeta).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "Welcome",
        tracking_id: "welcome",
      }),
    );

    // Real content lands in the file-canonical store as markdown, not SQL.
    expect(mocks.sessionWriteNote).toHaveBeenCalledWith(
      expect.any(String),
      "Hi",
    );
  });

  it("creates a session with no initial content and does not touch the note store", async () => {
    await createSession("Untitled");

    expect(mocks.sessionWriteMeta).toHaveBeenCalled();
    expect(mocks.sessionWriteNote).not.toHaveBeenCalled();
  });

  it("throws when the store fails to create the session", async () => {
    mocks.sessionWriteMeta.mockResolvedValueOnce({
      status: "error",
      error: "disk full",
    });

    await expect(createSession("Untitled")).rejects.toThrow("disk full");
    expect(mocks.sessionWriteNote).not.toHaveBeenCalled();
  });

  it("commits enhanced note content through the store as markdown, plus the derived title", async () => {
    await updateEnhancedNoteContent(
      "enhanced-note-1",
      "session-1",
      JSON.stringify({
        type: "doc",
        content: [
          { type: "paragraph", content: [{ type: "text", text: "Hi" }] },
        ],
      }),
      "Edited title",
    );

    // The doc body is file-canonical (`enhanced/<doc-id>.md`), so the editor's
    // prosemirror JSON is converted to markdown and written through the store -- never a
    // raw `UPDATE session_documents`.
    expect(mocks.sessionSaveEnhancedDoc).toHaveBeenCalledWith(
      "session-1",
      "enhanced-note-1",
      { markdown: "Hi" },
      "Edited title",
    );
    expect(mocks.sessionUpdateMeta).not.toHaveBeenCalled();
  });

  it("does not touch session meta when no derived title accompanies the note content", async () => {
    await updateEnhancedNoteContent(
      "enhanced-note-1",
      "session-1",
      '{"type":"doc"}',
    );

    expect(mocks.sessionSaveEnhancedDoc).toHaveBeenCalledWith(
      "session-1",
      "enhanced-note-1",
      { markdown: expect.any(String) },
      null,
    );
    expect(mocks.sessionUpdateMeta).not.toHaveBeenCalled();
  });

  it("throws when the store rejects the enhanced note update", async () => {
    mocks.sessionSaveEnhancedDoc.mockResolvedValueOnce({
      status: "error",
      error: "enhanced doc enhanced-note-1 in session session-1 has no file",
    });

    await expect(
      updateEnhancedNoteContent("enhanced-note-1", "session-1", "# md"),
    ).rejects.toThrow("has no file");
  });

  it("deletes an enhanced note through the store (file to trash, row hard-deleted)", async () => {
    await deleteEnhancedNote("enhanced-note-1", "session-1");

    expect(mocks.sessionDeleteEnhancedDoc).toHaveBeenCalledWith(
      "session-1",
      "enhanced-note-1",
    );
  });

  it("throws when the store rejects the enhanced note delete", async () => {
    mocks.sessionDeleteEnhancedDoc.mockResolvedValueOnce({
      status: "error",
      error: "boom",
    });

    await expect(
      deleteEnhancedNote("enhanced-note-1", "session-1"),
    ).rejects.toThrow("boom");
  });

  it("deletes the session through the store, capturing its title first for the undo toast", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-07-10T12:00:00.000Z"));
    mocks.sessionGet.mockResolvedValueOnce({
      status: "ok",
      data: {
        meta: { id: "session-1", title: "Planning" },
        note_markdown: null,
      },
    });

    const deleted = await softDeleteSession("session-1");

    expect(deleted).toEqual({
      session: { id: "session-1", title: "Planning" },
      tombstone: "2026-07-10T12:00:00.000Z",
      deletedAt: Date.parse("2026-07-10T12:00:00.000Z"),
    });
    expect(mocks.sessionDelete).toHaveBeenCalledWith("session-1");
  });

  it("does not call session_delete when the session no longer exists", async () => {
    mocks.sessionGet.mockResolvedValueOnce({ status: "ok", data: null });

    await expect(softDeleteSession("session-1")).resolves.toBeNull();
    expect(mocks.sessionDelete).not.toHaveBeenCalled();
  });

  it("throws (does not swallow) a genuine session_delete command failure", async () => {
    mocks.sessionGet.mockResolvedValueOnce({
      status: "ok",
      data: {
        meta: { id: "session-1", title: "Planning" },
        note_markdown: null,
      },
    });
    mocks.sessionDelete.mockResolvedValueOnce({
      status: "error",
      error: "boom",
    });

    // Must reject, not resolve to null -- useDeleteSession distinguishes "already deleted"
    // (benign null) from a real failure (must surface to its catch block's error toast) purely
    // by whether the promise rejects.
    await expect(softDeleteSession("session-1")).rejects.toThrow("boom");
  });

  // The emptiness semantics themselves (title/event interplay, &nbsp; placeholder,
  // transcript/doc/tag counts, and attachments) are covered by the Rust store's
  // session_is_empty tests; the frontend is a passthrough now.
  it("delegates emptiness to the store command", async () => {
    mocks.sessionIsEmpty.mockResolvedValueOnce({ status: "ok", data: true });
    await expect(isSessionEmpty("session-1")).resolves.toBe(true);
    expect(mocks.sessionIsEmpty).toHaveBeenCalledWith("session-1");

    mocks.sessionIsEmpty.mockResolvedValueOnce({ status: "ok", data: false });
    await expect(isSessionEmpty("session-1")).resolves.toBe(false);
  });

  it("throws when the emptiness command fails", async () => {
    mocks.sessionIsEmpty.mockResolvedValueOnce({
      status: "error",
      error: "store gone",
    });
    await expect(isSessionEmpty("session-1")).rejects.toThrow("store gone");
  });

  it("waits for a pending delete to settle, then restores through the store", async () => {
    await restoreDeletedSession({
      session: { id: "session-1", title: "Planning" },
      tombstone: "2026-07-10T12:00:00.000Z",
      deletedAt: 1,
    });

    expect(mocks.waitForPendingSoftDelete).toHaveBeenCalledWith("session-1");
    expect(mocks.sessionRestore).toHaveBeenCalledWith("session-1");
  });

  it("throws when nothing was trashed to restore", async () => {
    mocks.sessionRestore.mockResolvedValueOnce({ status: "ok", data: false });

    await expect(
      restoreDeletedSession({
        session: { id: "session-1", title: "Planning" },
        tombstone: "2026-07-10T12:00:00.000Z",
        deletedAt: 1,
      }),
    ).rejects.toThrow("was never soft-deleted");
  });

  it("throws when the store restore call fails", async () => {
    mocks.sessionRestore.mockResolvedValueOnce({
      status: "error",
      error: "boom",
    });

    await expect(
      restoreDeletedSession({
        session: { id: "session-1", title: "Planning" },
        tombstone: "2026-07-10T12:00:00.000Z",
        deletedAt: 1,
      }),
    ).rejects.toThrow("boom");
  });
});

it("writes and deletes the summary using only the session ID", async () => {
  await updateEnhancedNoteContent("session-1", "session-1", "# Plain summary");
  expect(mocks.sessionSaveSummary).toHaveBeenCalledWith(
    "session-1",
    "# Plain summary",
    null,
    null,
  );
  await deleteEnhancedNote("session-1", "session-1");
  expect(mocks.sessionDeleteSummary).toHaveBeenCalledWith("session-1");
});
