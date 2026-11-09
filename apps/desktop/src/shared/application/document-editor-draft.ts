import type { TaskSource } from "@hypr/editor/tasks";

import { DraftRegistry } from "./draft-registry";
import { ApplicationRequestError } from "./errors";
import {
  editorTaskIntentKey,
  editorText,
  type EditorText,
} from "./note-editor-draft";
import type {
  DocumentEditorSnapshot,
  DocumentTarget,
  MetadataSnapshot,
  NoteEditorSnapshot,
} from "./protocol.gen";
import {
  DraftConflictError,
  RevisionDraft,
  type DraftTransport,
} from "./revision-draft";
import type { VaultClient } from "./vault-client";

import type { NoteDraftRecovery } from "~/types/tauri.gen";

export type DocumentEditorBaseline = NoteEditorSnapshot & {
  metadata: MetadataSnapshot;
};
type EditorClient = Pick<
  VaultClient,
  "scope" | "canEdit" | "saveDocumentEditor"
>;

export function documentEditorDraftId(
  sessionId: string,
  target: DocumentTarget,
): string {
  return JSON.stringify(
    target.kind === "summary"
      ? ["document_editor", sessionId, "summary"]
      : ["document_editor", sessionId, "template_output", target.document_id],
  );
}
export function documentEditorSource(
  sessionId: string,
  target: DocumentTarget,
): TaskSource {
  return target.kind === "summary"
    ? { type: "session_summary", id: sessionId }
    : { type: "enhanced_note", id: target.document_id };
}
function invalid(
  message: string,
  outcome: "not_committed" | "unknown" = "not_committed",
) {
  return new ApplicationRequestError({
    code: "incompatible_version",
    message,
    request_id: null,
    retryable: false,
    current_revision: null,
    mutation_outcome: outcome,
  });
}
function representableTasks(tasks: readonly { body: unknown }[]) {
  if (
    tasks.some(
      (task) =>
        task.body !== null &&
        task.body !== undefined &&
        !Array.isArray(task.body),
    )
  )
    throw invalid(
      "This document contains task body data the current editor cannot safely edit",
    );
}
function validateBaseline(
  sessionId: string,
  target: DocumentTarget,
  baseline: DocumentEditorBaseline,
) {
  const source = documentEditorSource(sessionId, target);
  representableTasks(baseline.tasks.tasks);
  if (
    baseline.metadata.meta.id !== sessionId ||
    baseline.note.title !== baseline.metadata.meta.title ||
    baseline.tasks.tasks.some(
      (task) =>
        task.source_type !== source.type || task.source_id !== source.id,
    )
  )
    throw invalid(
      "This document editor baseline belongs to a different session or task source",
    );
}
export function documentEditorBaseline(
  sessionId: string,
  target: DocumentTarget,
  value: DocumentEditorSnapshot,
): DocumentEditorBaseline {
  const doc = value.document.document;
  if (!doc && target.kind === "template_output")
    throw new ApplicationRequestError({
      code: "not_found",
      message: "This output no longer exists",
      request_id: null,
      retryable: false,
      current_revision: value.document.revision,
      mutation_outcome: "not_committed",
    });
  if (
    doc &&
    (doc.session_id !== sessionId ||
      doc.kind !== target.kind ||
      doc.id !== (target.kind === "summary" ? sessionId : target.document_id))
  )
    throw invalid("The server returned a different document");
  const baseline = {
    note: {
      markdown: doc?.markdown ?? null,
      title: value.metadata.meta.title,
      revision: value.document.revision,
    },
    metadata: value.metadata,
    tasks: value.tasks,
  };
  validateBaseline(sessionId, target, baseline);
  return structuredClone(baseline);
}
export function documentEditorRevision(value: DocumentEditorBaseline): string {
  return JSON.stringify([
    value.note.revision,
    value.metadata.revision,
    value.tasks.revision,
  ]);
}
function sameText(draft: EditorText, value: DocumentEditorBaseline): boolean {
  return (
    draft.markdown === (value.note.markdown ?? "") &&
    draft.title === value.metadata.meta.title &&
    editorTaskIntentKey(draft.tasks) === editorTaskIntentKey(value.tasks.tasks)
  );
}
function transport(
  client: EditorClient,
  sessionId: string,
  target: DocumentTarget,
): DraftTransport<DocumentEditorBaseline, EditorText> {
  const captured = Object.freeze(structuredClone(target));
  const key = documentEditorDraftId(sessionId, captured);
  return {
    scope: client.scope,
    get canEdit() {
      return client.canEdit;
    },
    save: async (id, draft, baseline, options) => {
      if (id !== key)
        throw invalid("This draft belongs to a different document");
      const result = await client.saveDocumentEditor(
        sessionId,
        captured,
        {
          ...draft,
          expected_revision: baseline.note.revision,
          expected_metadata_revision: baseline.metadata.revision,
          expected_tasks_revision: baseline.tasks.revision,
        },
        options,
      );
      try {
        if (result.kind === "saved" && !result.snapshot.document.document)
          throw invalid("The server omitted the saved document", "unknown");
        return {
          kind: result.kind,
          snapshot: documentEditorBaseline(
            sessionId,
            captured,
            result.snapshot,
          ),
        };
      } catch (error) {
        if (error instanceof ApplicationRequestError)
          throw new ApplicationRequestError({
            ...error.detail,
            mutation_outcome: "unknown",
          });
        throw error;
      }
    },
  };
}
export class DocumentEditorConflictError extends DraftConflictError<DocumentEditorBaseline> {
  constructor(current: DocumentEditorBaseline) {
    super(current, documentEditorRevision(current));
  }
}
export class DocumentEditorDraft extends RevisionDraft<
  DocumentEditorBaseline,
  EditorText
> {
  readonly target: DocumentTarget;
  readonly source: TaskSource;
  constructor(
    client: EditorClient,
    readonly ownerSessionId: string,
    target: DocumentTarget,
    baseline: DocumentEditorBaseline,
    draft?: EditorText,
    uncertain = false,
  ) {
    const captured = Object.freeze(structuredClone(target));
    validateBaseline(ownerSessionId, captured, baseline);
    if (draft) representableTasks(draft.tasks);
    super(
      transport(client, ownerSessionId, captured),
      documentEditorDraftId(ownerSessionId, captured),
      {
        revision: documentEditorRevision,
        project: editorText,
        matches: (draft, value) =>
          value.note.markdown === draft.markdown && sameText(draft, value),
        isDirty: (draft, value) => !sameText(draft, value),
        canAdvance: (before, current) =>
          before.note.revision === current.note.revision &&
          before.metadata.revision === current.metadata.revision &&
          sameText(editorText(before), current),
        conflict: (value) => new DocumentEditorConflictError(value),
      },
      structuredClone(baseline),
      draft ? structuredClone(draft) : undefined,
      uncertain,
    );
    this.target = captured;
    this.source = Object.freeze(documentEditorSource(ownerSessionId, captured));
  }
  rebind(client: EditorClient) {
    this.bind(transport(client, this.ownerSessionId, this.target));
  }
  override receive(value: DocumentEditorBaseline) {
    validateBaseline(this.ownerSessionId, this.target, value);
    super.receive(value);
  }
  override capture(getDraft: () => EditorText) {
    super.capture(() => {
      const value = getDraft();
      representableTasks(value.tasks);
      return value;
    });
  }
  edit(value: EditorText) {
    representableTasks(value.tasks);
    this.setDraft(structuredClone(value));
  }
  recovery(): NoteDraftRecovery {
    const value = this.recoveryState();
    return structuredClone({
      scope: this.scope,
      sessionId: this.sessionId,
      baseline: value.baseline.note,
      draft: { markdown: value.draft.markdown, title: value.draft.title },
      uncertain: value.uncertain,
      editorTasks: { baseline: value.baseline.tasks, draft: value.draft.tasks },
      documentEditor: {
        sessionId: this.ownerSessionId,
        target: this.target,
        metadata: value.baseline.metadata,
      },
    });
  }
  static restore(
    client: EditorClient,
    sessionId: string,
    target: DocumentTarget,
    recovery: NoteDraftRecovery,
  ) {
    const document = recovery.documentEditor;
    if (
      client.scope !== recovery.scope ||
      recovery.sessionId !== documentEditorDraftId(sessionId, target) ||
      !document ||
      document.sessionId !== sessionId ||
      documentEditorDraftId(document.sessionId, document.target) !==
        recovery.sessionId ||
      !recovery.editorTasks
    )
      throw invalid(
        "This recovery copy belongs to a different account, vault, session or document",
      );
    return new DocumentEditorDraft(
      client,
      sessionId,
      target,
      {
        note: recovery.baseline,
        metadata: document.metadata,
        tasks: recovery.editorTasks.baseline,
      },
      { ...recovery.draft, tasks: recovery.editorTasks.draft },
      recovery.uncertain,
    );
  }
}
export class DocumentEditorDrafts extends DraftRegistry<DocumentEditorDraft> {
  open(
    client: EditorClient,
    sessionId: string,
    target: DocumentTarget,
    baseline: DocumentEditorBaseline,
  ) {
    const key = documentEditorDraftId(sessionId, target);
    const existing = this.get(client.scope, key);
    if (existing) {
      existing.rebind(client);
      existing.receive(baseline);
      return existing;
    }
    return this.set(
      client.scope,
      key,
      new DocumentEditorDraft(client, sessionId, target, baseline),
    );
  }
  restore(
    client: EditorClient,
    sessionId: string,
    target: DocumentTarget,
    recovery: NoteDraftRecovery,
  ) {
    return this.set(
      client.scope,
      documentEditorDraftId(sessionId, target),
      DocumentEditorDraft.restore(client, sessionId, target, recovery),
    );
  }
  forSession(scope: string, sessionId: string) {
    return [...this.values(scope)].filter(
      (draft) => draft.ownerSessionId === sessionId,
    );
  }
}
export const documentEditorDrafts = new DocumentEditorDrafts();
