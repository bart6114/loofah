import { describe, expect, it, vi } from "vitest";

import { ApplicationRequestError } from "./errors";
import { NoteDrafts } from "./note-draft";
import {
  EditorConflictError,
  NoteEditorDraft,
  editorRevision,
  editorText,
  noteEditorDraftId,
} from "./note-editor-draft";
import type { NoteEditorSnapshot } from "./protocol.gen";
import type { VaultClient } from "./vault-client";

function snapshot(
  noteRevision = "n0",
  tasksRevision = "t0",
  dueAt = "",
): NoteEditorSnapshot {
  return {
    note: { markdown: "- [ ] Task", title: "Title", revision: noteRevision },
    tasks: {
      revision: tasksRevision,
      tasks: [
        {
          id: "task",
          source_type: "session_raw_note",
          source_id: "note",
          source_order: 0,
          status: "todo",
          text: "Task",
          body: [
            { type: "paragraph", content: [{ type: "text", text: "Task" }] },
          ],
          due_at: dueAt,
          assignee: "alice",
          created_at: "2026-11-01T10:00:00Z",
          updated_at: "2026-11-01T10:00:00Z",
        },
      ],
    },
  };
}
function client(scope = "https:alice:vault") {
  return { scope, canEdit: true, saveNoteEditor: vi.fn() };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

describe("compound editor drafts", () => {
  it("saves task-only changes with both baselines while preserving newer local task input", async () => {
    const backend = client();
    const first = deferred<unknown>();
    backend.saveNoteEditor
      .mockReturnValueOnce(first.promise)
      .mockResolvedValueOnce({
        kind: "saved",
        snapshot: snapshot("n0", "t2", "2027-02-01"),
      });
    const draft = new NoteEditorDraft(backend, "note", snapshot());
    const input = editorText(snapshot("n0", "t0", "2027-01-01"));
    draft.edit(input);
    expect(draft.getSnapshot().dirty).toBe(true);
    const saving = draft.save();
    input.tasks[0].due_at =
      "Mutating editor objects cannot alter an admitted save";
    await vi.waitFor(() =>
      expect(backend.saveNoteEditor).toHaveBeenCalledOnce(),
    );
    expect(backend.saveNoteEditor.mock.calls[0][1]).toMatchObject({
      expected_revision: "n0",
      expected_tasks_revision: "t0",
      tasks: [{ id: "task", due_at: "2027-01-01" }],
    });
    expect(backend.saveNoteEditor.mock.calls[0][1].tasks[0]).not.toHaveProperty(
      "assignee",
    );
    draft.edit(editorText(snapshot("n0", "t0", "2027-02-01")));
    const queued = draft.save();
    first.resolve({
      kind: "saved",
      snapshot: snapshot("n0", "t1", "2027-01-01"),
    });
    await saving;
    expect(draft.getSnapshot().draft.tasks[0].due_at).toBe("2027-02-01");
    await queued;
    expect(backend.saveNoteEditor.mock.calls[1][1]).toMatchObject({
      expected_revision: "n0",
      expected_tasks_revision: "t1",
    });
    expect(draft.getSnapshot().dirty).toBe(false);
  });

  it("detects task-only conflicts and requires review of both revisions", async () => {
    const backend = client();
    const draft = new NoteEditorDraft(backend, "note", snapshot());
    draft.edit(editorText(snapshot("n0", "t0", "Mine")));
    draft.receive(snapshot("n0", "t1", "Other client"));
    const reviewed = editorRevision(draft.getSnapshot().conflict!);
    draft.receive(snapshot("n0", "t2", "Latest other client"));
    expect(() => draft.keepDraft(reviewed)).toThrow("Review the latest");
    expect(() => draft.useServerVersion("n0")).toThrow("Review the latest");
    await expect(draft.save()).rejects.toBeInstanceOf(EditorConflictError);
    expect(backend.saveNoteEditor).not.toHaveBeenCalled();
    draft.keepDraft(editorRevision(draft.getSnapshot().conflict!));
    backend.saveNoteEditor.mockResolvedValue({
      kind: "saved",
      snapshot: snapshot("n0", "t3", "Mine"),
    });
    await draft.save();
    expect(backend.saveNoteEditor.mock.calls[0][1]).toMatchObject({
      expected_revision: "n0",
      expected_tasks_revision: "t2",
    });
    expect(draft.getSnapshot().dirty).toBe(false);
  });

  it("never acknowledges a lost task-only write from matching Markdown alone", async () => {
    const backend = client();
    backend.saveNoteEditor.mockRejectedValue(new Error("lost response"));
    const draft = new NoteEditorDraft(backend, "note", snapshot());
    draft.edit(editorText(snapshot("n0", "t0", "2027-01-01")));
    await expect(draft.save()).rejects.toThrow("lost response");
    draft.receive(snapshot());
    expect(draft.recovery().uncertain).toBe(true);
    expect(draft.getSnapshot().dirty).toBe(true);
    draft.receive(snapshot("n0", "t1", "Different due date"));
    expect(draft.getSnapshot().conflict?.tasks.revision).toBe("t1");
    expect(draft.recovery().uncertain).toBe(true);
    draft.receive(snapshot("n0", "t2", "2027-01-01"));
    expect(draft.getSnapshot()).toMatchObject({
      dirty: false,
      conflict: null,
      baseline: { tasks: { revision: "t2" } },
    });
    expect(draft.recovery().uncertain).toBe(false);
  });

  it("retains both baselines and exact task state in a device recovery copy", async () => {
    const backend = client();
    backend.saveNoteEditor.mockRejectedValue(new Error("Disconnected"));
    const draft = new NoteEditorDraft(backend, "note", snapshot());
    const input = editorText(snapshot("n0", "t0", "2027-01-01"));
    input.tasks[0].status = "in_progress";
    input.tasks[0].body = [{ type: "session", attrs: { id: "linked" } }];
    draft.edit(input);
    await expect(draft.save()).rejects.toThrow("Disconnected");
    const recovery = draft.recovery();
    expect(recovery.sessionId).toBe(noteEditorDraftId("note"));
    expect(recovery.editorTasks?.baseline.revision).toBe("t0");
    expect(recovery.editorTasks?.draft).toEqual(input.tasks);
    const restored = NoteEditorDraft.restore(backend, "note", recovery);
    recovery.editorTasks!.draft[0].due_at = "Cannot mutate restored state";
    expect(restored.recovery().editorTasks?.draft[0].due_at).toBe("2027-01-01");
    expect(restored.recovery().uncertain).toBe(true);
    expect(() =>
      NoteEditorDraft.restore(client("bob"), "note", restored.recovery()),
    ).toThrow("different account");
    expect(() =>
      NoteEditorDraft.restore(backend, "other", restored.recovery()),
    ).toThrow("different account");
    expect(() =>
      new NoteDrafts().restore(
        backend as unknown as VaultClient,
        restored.recovery(),
      ),
    ).toThrow("task support");
    const textOnly = restored.recovery();
    delete textOnly.editorTasks;
    expect(() => NoteEditorDraft.restore(backend, "note", textOnly)).toThrow(
      "does not contain",
    );
  });

  it("keeps newer server task state when its read arrives before an older save acknowledgement", async () => {
    const backend = client();
    const result = deferred<unknown>();
    backend.saveNoteEditor.mockReturnValue(result.promise);
    const draft = new NoteEditorDraft(backend, "note", snapshot());
    draft.edit(editorText(snapshot("n0", "t0", "Mine")));
    const saving = draft.save();
    await vi.waitFor(() =>
      expect(backend.saveNoteEditor).toHaveBeenCalledOnce(),
    );
    draft.receive(snapshot("n0", "t2", "Newer writer"));
    result.resolve({ kind: "saved", snapshot: snapshot("n0", "t1", "Mine") });
    await saving;
    expect(draft.getSnapshot().conflict?.tasks.revision).toBe("t2");
    await expect(draft.save()).rejects.toBeInstanceOf(EditorConflictError);
    expect(backend.saveNoteEditor).toHaveBeenCalledOnce();
  });

  it("discards queued task saves and ignores an old in-flight reply after explicit resolution", async () => {
    const backend = client();
    const result = deferred<unknown>();
    backend.saveNoteEditor.mockReturnValue(result.promise);
    const draft = new NoteEditorDraft(backend, "note", snapshot());
    draft.edit(editorText(snapshot("n0", "t0", "Mine")));
    const saving = draft.save();
    await vi.waitFor(() =>
      expect(backend.saveNoteEditor).toHaveBeenCalledOnce(),
    );
    draft.edit(editorText(snapshot("n0", "t0", "Queued")));
    const queued = draft.save();
    const checked = Promise.allSettled([saving, queued]);
    draft.receive(snapshot("n0", "t2", "Other writer"));
    draft.useServerVersion(editorRevision(draft.getSnapshot().conflict!));
    result.resolve({ kind: "saved", snapshot: snapshot("n0", "t1", "Mine") });
    expect((await checked).every((item) => item.status === "rejected")).toBe(
      true,
    );
    expect(backend.saveNoteEditor).toHaveBeenCalledOnce();
    expect(draft.getSnapshot()).toMatchObject({
      dirty: false,
      draft: { tasks: [{ due_at: "Other writer" }] },
    });
  });

  it("treats a conflict as an acknowledged retry only when all explicit task intent matches", async () => {
    const backend = client();
    const current = snapshot("n0", "t1", "Mine");
    backend.saveNoteEditor.mockResolvedValue({
      kind: "conflict",
      snapshot: current,
    });
    const draft = new NoteEditorDraft(backend, "note", snapshot());
    draft.edit(editorText(current));
    await draft.save();
    expect(draft.getSnapshot().dirty).toBe(false);
    const differentStatus = snapshot("n0", "t2", "Other");
    differentStatus.tasks.tasks[0].status = "done";
    backend.saveNoteEditor.mockResolvedValue({
      kind: "conflict",
      snapshot: differentStatus,
    });
    draft.edit(editorText(snapshot("n0", "t1", "Other")));
    await expect(draft.save()).rejects.toBeInstanceOf(EditorConflictError);
    expect(draft.getSnapshot().draft.tasks[0].status).toBe("todo");
  });

  it("compares rich bodies without depending on JSON property order and captures edits lazily", async () => {
    const original = snapshot();
    const draft = new NoteEditorDraft(client(), "note", original);
    const same = editorText(original);
    same.tasks[0].body = [
      { content: [{ text: "Task", type: "text" }], type: "paragraph" },
    ];
    draft.edit(same);
    expect(draft.getSnapshot().dirty).toBe(false);
    const capture = vi.fn(() => ({
      ...same,
      tasks: [{ ...same.tasks[0], status: "in_progress" }],
    }));
    draft.capture(capture);
    expect(draft.getSnapshot().dirty).toBe(true);
    expect(capture).not.toHaveBeenCalled();
    expect(draft.recovery().editorTasks?.draft[0].status).toBe("in_progress");
    expect(capture).toHaveBeenCalledOnce();
  });

  it("enforces read-only access and identity binding before a task save", async () => {
    const backend = client();
    backend.canEdit = false;
    const draft = new NoteEditorDraft(backend, "note", snapshot());
    draft.edit(editorText(snapshot("n0", "t0", "Mine")));
    await expect(draft.save()).rejects.toMatchObject({
      detail: { code: "forbidden", mutation_outcome: "not_committed" },
    });
    expect(backend.saveNoteEditor).not.toHaveBeenCalled();
    expect(draft.recovery().uncertain).toBe(false);
    expect(() => draft.rebind(client("bob:vault"))).toThrow(
      "different account",
    );
  });

  it("retains task intent after explicit precommit errors without marking it as an uncertain write", async () => {
    const backend = client();
    backend.saveNoteEditor.mockRejectedValue(
      new ApplicationRequestError({
        code: "resource_limit",
        message: "Too large",
        request_id: "request",
        retryable: false,
        current_revision: null,
        mutation_outcome: "not_committed",
      }),
    );
    const draft = new NoteEditorDraft(backend, "note", snapshot());
    draft.edit(editorText(snapshot("n0", "t0", "Mine")));
    await expect(draft.save()).rejects.toThrow("Too large");
    expect(draft.recovery().uncertain).toBe(false);
    expect(draft.getSnapshot().draft.tasks[0].due_at).toBe("Mine");
  });
});
