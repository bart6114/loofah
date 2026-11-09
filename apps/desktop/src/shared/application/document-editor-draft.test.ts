import { expect, it, vi } from "vitest";

import {
  DocumentEditorDraft,
  DocumentEditorDrafts,
  DocumentEditorConflictError,
  documentEditorBaseline,
  documentEditorDraftId,
  documentEditorRevision,
} from "./document-editor-draft";
import { ApplicationRequestError } from "./errors";
import { NoteDrafts } from "./note-draft";
import { editorText, NoteEditorDraft } from "./note-editor-draft";
import type { DocumentEditorSnapshot, DocumentTarget } from "./protocol.gen";

const summary: DocumentTarget = { kind: "summary" };
const output: DocumentTarget = {
  kind: "template_output",
  document_id: "manual-output",
};
function value(
  target = summary,
  revision = "d0",
  metadataRevision = "m0",
  tasksRevision = "t0",
): DocumentEditorSnapshot {
  return {
    document: {
      document: {
        id: target.kind === "summary" ? "note" : target.document_id,
        session_id: "note",
        kind: target.kind,
        title: "Output frontmatter title",
        template_id: "template",
        sort_order: 3,
        markdown: "Original document",
      },
      revision,
    },
    metadata: {
      meta: {
        id: "note",
        title: "SESSION title",
        created_at: "2026-11-08T10:00:00Z",
        started_at: null,
        ended_at: null,
        tags: ["keep"],
        author: null,
      },
      revision: metadataRevision,
    },
    tasks: {
      tasks: [
        {
          id: "stable",
          source_type:
            target.kind === "summary" ? "session_summary" : "enhanced_note",
          source_id: target.kind === "summary" ? "note" : target.document_id,
          source_order: 0,
          status: "todo",
          text: "Task",
          body: [
            {
              type: "paragraph",
              content: [{ type: "text", text: "Keep body" }],
            },
          ],
          due_at: "2027-01-01",
          assignee: "person",
          created_at: "2026-11-08T10:00:00Z",
          updated_at: "2026-11-08T10:00:00Z",
        },
      ],
      revision: tasksRevision,
    },
  };
}
function client(scope = "scope") {
  return { scope, canEdit: true, saveDocumentEditor: vi.fn() };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

it.each([summary, output])(
  "saves body/session title/rich task intent with all three captured revisions for $kind",
  async (target) => {
    const backend = client();
    const baseline = documentEditorBaseline("note", target, value(target));
    const draft = new DocumentEditorDraft(backend, "note", target, baseline);
    const changed = {
      ...editorText(baseline),
      markdown: "Changed body",
      title: "Changed SESSION title",
      tasks: editorText(baseline).tasks.map((task) => ({
        ...task,
        status: "in_progress",
        due_at: "2028-01-01",
      })),
    };
    draft.edit(changed);
    const saved = value(target, "d1", "m1", "t1");
    saved.document.document!.markdown = changed.markdown;
    saved.metadata.meta.title = changed.title;
    saved.tasks.tasks[0] = { ...saved.tasks.tasks[0], ...changed.tasks[0] };
    backend.saveDocumentEditor.mockResolvedValue({
      kind: "saved",
      snapshot: saved,
    });
    await draft.save();
    expect(backend.saveDocumentEditor).toHaveBeenCalledWith(
      "note",
      target,
      {
        ...changed,
        expected_revision: "d0",
        expected_metadata_revision: "m0",
        expected_tasks_revision: "t0",
      },
      undefined,
    );
    expect(draft.getSnapshot().dirty).toBe(false);
    expect(draft.getSnapshot().baseline.note.title).toBe(
      "Changed SESSION title",
    );
    expect(saved.document.document!.title).toBe("Output frontmatter title");
  },
);

it("pins target/session coordinates and never redirects queued saves after client rebind", async () => {
  const target = {
    kind: "template_output" as const,
    document_id: "manual-output",
  };
  const first = client(),
    second = client();
  const baseline = documentEditorBaseline("note", target, value(target));
  const draft = new DocumentEditorDraft(first, "note", target, baseline);
  const pending = deferred<unknown>();
  first.saveDocumentEditor.mockReturnValue(pending.promise);
  draft.edit({ ...editorText(baseline), markdown: "First" });
  const one = draft.save();
  draft.edit({ ...editorText(baseline), markdown: "Second" });
  const two = draft.save();
  draft.rebind(second);
  target.document_id = "foreign";
  const saved = value(output, "d1", "m1", "t1");
  saved.document.document!.markdown = "First";
  await vi.waitFor(() =>
    expect(first.saveDocumentEditor).toHaveBeenCalledOnce(),
  );
  pending.resolve({ kind: "saved", snapshot: saved });
  await one;
  const final = value(output, "d2", "m2", "t2");
  final.document.document!.markdown = "Second";
  first.saveDocumentEditor.mockResolvedValue({
    kind: "saved",
    snapshot: final,
  });
  await two;
  expect(first.saveDocumentEditor).toHaveBeenCalledTimes(2);
  expect(second.saveDocumentEditor).not.toHaveBeenCalled();
  expect(
    first.saveDocumentEditor.mock.calls.every(
      ([, target]) => target.document_id === "manual-output",
    ),
  ).toBe(true);
  expect(Object.isFrozen(draft.target)).toBe(true);
  expect(draft.sessionId).toBe(documentEditorDraftId("note", output));
});

it("preserves later typing and reports a meaningful canonical conflict without replaying stale title/tasks", async () => {
  const backend = client();
  const baseline = documentEditorBaseline("note", summary, value());
  const draft = new DocumentEditorDraft(backend, "note", summary, baseline);
  const pending = deferred<unknown>();
  backend.saveDocumentEditor.mockReturnValue(pending.promise);
  draft.edit({ ...editorText(baseline), markdown: "First" });
  const saving = draft.save();
  await vi.waitFor(() =>
    expect(backend.saveDocumentEditor).toHaveBeenCalledOnce(),
  );
  draft.edit({
    ...editorText(baseline),
    markdown: "Later typing",
    title: "Later title",
  });
  const current = value(summary, "someone-else", "m2", "t2");
  current.metadata.meta.title = "Other title";
  draft.receive(documentEditorBaseline("note", summary, current));
  pending.resolve({ kind: "conflict", snapshot: current });
  await expect(saving).rejects.toBeInstanceOf(DocumentEditorConflictError);
  expect(draft.getSnapshot().draft.markdown).toBe("Later typing");
  expect(draft.getSnapshot().draft.title).toBe("Later title");
  expect(backend.saveDocumentEditor).toHaveBeenCalledOnce();
  const reviewed = documentEditorRevision(draft.getSnapshot().conflict!);
  draft.keepDraft(reviewed);
  expect(draft.recovery().documentEditor?.metadata.revision).toBe("m2");
});

it("can advance only unrelated task-file revisions, never metadata/body or meaningful task changes", async () => {
  const backend = client();
  const baseline = documentEditorBaseline("note", summary, value());
  const draft = new DocumentEditorDraft(backend, "note", summary, baseline);
  draft.edit({ ...editorText(baseline), markdown: "Mine" });
  draft.receive(
    documentEditorBaseline(
      "note",
      summary,
      value(summary, "d0", "m0", "unrelated-tasks"),
    ),
  );
  expect(draft.getSnapshot().conflict).toBeNull();
  expect(draft.getSnapshot().baseline.tasks.revision).toBe("unrelated-tasks");
  const metadata = value(summary, "d0", "changed-date", "unrelated-tasks");
  draft.receive(documentEditorBaseline("note", summary, metadata));
  expect(draft.getSnapshot().conflict?.metadata.revision).toBe("changed-date");
  await expect(draft.save()).rejects.toBeInstanceOf(
    DocumentEditorConflictError,
  );
  expect(backend.saveDocumentEditor).not.toHaveBeenCalled();
});

it.each(["missing", "foreign-session", "foreign-kind", "foreign-task-source"])(
  "keeps unknown save outcome and full recovery for a %s reply",
  async (kind) => {
    const backend = client();
    const baseline = documentEditorBaseline("note", output, value(output));
    const draft = new DocumentEditorDraft(backend, "note", output, baseline);
    draft.edit({ ...editorText(baseline), markdown: "Keep mine" });
    const response = value(output);
    if (kind === "missing") response.document.document = null;
    else if (kind === "foreign-session")
      response.document.document!.session_id = "foreign";
    else if (kind === "foreign-kind")
      response.document.document!.kind = "summary";
    else response.tasks.tasks[0].source_id = "foreign";
    backend.saveDocumentEditor.mockResolvedValue({
      kind: "saved",
      snapshot: response,
    });
    await expect(draft.save()).rejects.toBeInstanceOf(ApplicationRequestError);
    expect(draft.recovery().uncertain).toBe(true);
    expect(draft.recovery().draft.markdown).toBe("Keep mine");
    expect(draft.recovery().editorTasks?.draft[0].due_at).toBe("2027-01-01");
  },
);

it("round-trips compact v3 recovery and refuses wrong editors/scopes/sessions/targets", () => {
  const backend = client();
  const baseline = documentEditorBaseline("note", output, value(output));
  const draft = new DocumentEditorDraft(backend, "note", output, baseline);
  draft.edit({
    ...editorText(baseline),
    markdown: "Recovered",
    title: "Recovered session title",
  });
  const recovery = draft.recovery();
  const restored = DocumentEditorDraft.restore(
    backend,
    "note",
    output,
    recovery,
  );
  expect(restored.recovery()).toEqual(recovery);
  expect(recovery.documentEditor?.metadata.meta.title).toBe("SESSION title");
  expect(recovery.baseline.title).toBe("SESSION title");
  expect(() =>
    DocumentEditorDraft.restore(
      client("other-scope"),
      "note",
      output,
      recovery,
    ),
  ).toThrow();
  expect(() =>
    DocumentEditorDraft.restore(backend, "other-session", output, recovery),
  ).toThrow();
  expect(() =>
    DocumentEditorDraft.restore(backend, "note", summary, recovery),
  ).toThrow();
  expect(() =>
    NoteEditorDraft.restore(
      { scope: backend.scope, canEdit: true, saveNoteEditor: vi.fn() },
      "note",
      recovery,
    ),
  ).toThrow();
  expect(() =>
    new NoteDrafts().restore(
      {
        scope: backend.scope,
        canEdit: true,
        readNote: vi.fn(),
        saveNote: vi.fn(),
      },
      recovery,
    ),
  ).toThrow();
});

it("filters registries by owning session and scope, retaining dirty drafts through unmount", () => {
  const drafts = new DocumentEditorDrafts();
  const backend = client();
  const first = drafts.open(
    backend,
    "note",
    summary,
    documentEditorBaseline("note", summary, value()),
  );
  const second = drafts.open(
    backend,
    "note",
    output,
    documentEditorBaseline("note", output, value(output)),
  );
  expect(drafts.forSession(backend.scope, "note")).toEqual([first, second]);
  expect(drafts.forSession("other-scope", "note")).toEqual([]);
  first.edit({
    ...editorText(first.getSnapshot().baseline),
    markdown: "Dirty",
  });
  drafts.clearSaved(backend.scope);
  expect(drafts.forSession(backend.scope, "note")).toEqual([first]);
});

it("does not confirm an uncertain empty Summary save from a still-missing canonical document", () => {
  const backend = client();
  const missing = value();
  missing.document.document = null;
  missing.tasks.tasks = [];
  const baseline = documentEditorBaseline("note", summary, missing);
  const draft = new DocumentEditorDraft(
    backend,
    "note",
    summary,
    baseline,
    editorText(baseline),
    true,
  );
  draft.receive(baseline);
  expect(draft.recovery().uncertain).toBe(true);
  const saved = value(summary, "exists");
  saved.document.document!.markdown = "";
  saved.tasks.tasks = [];
  draft.receive(documentEditorBaseline("note", summary, saved));
  expect(draft.recovery().uncertain).toBe(false);
  expect(draft.getSnapshot().baseline.note.markdown).toBe("");
});

it.each([{ future_payload: ["keep"] }, "opaque body", 7, true])(
  "refuses opaque task bodies in canonical data and recovery without modifying the recovery",
  (body) => {
    const backend = client();
    const future = value();
    future.tasks.tasks[0].body = body;
    expect(() => documentEditorBaseline("note", summary, future)).toThrow(
      "cannot safely edit",
    );
    const baseline = documentEditorBaseline("note", summary, value());
    const draft = new DocumentEditorDraft(backend, "note", summary, baseline);
    const recovery = draft.recovery();
    recovery.editorTasks!.draft[0].body = body;
    const original = structuredClone(recovery);
    expect(() =>
      DocumentEditorDraft.restore(backend, "note", summary, recovery),
    ).toThrow("cannot safely edit");
    expect(recovery).toEqual(original);
    expect(backend.saveDocumentEditor).not.toHaveBeenCalled();
  },
);
it("keeps explicit legacy null task bodies readable without changing the original baseline", () => {
  const current = value();
  current.tasks.tasks[0].body = null;
  const baseline = documentEditorBaseline("note", summary, current);
  const draft = new DocumentEditorDraft(client(), "note", summary, baseline);
  expect(draft.recovery().editorTasks?.baseline.tasks[0].body).toBeNull();
});
