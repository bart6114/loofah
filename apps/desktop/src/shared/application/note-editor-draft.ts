import { hashKey } from "@tanstack/react-query";

import { DraftRegistry } from "./draft-registry";
import type { NoteEditorSnapshot, TaskInput } from "./protocol.gen";
import {
  DraftConflictError,
  RevisionDraft,
  type DraftTransport,
} from "./revision-draft";
import type { VaultClient } from "./vault-client";

import type { NoteDraftRecovery } from "~/types/tauri.gen";

export type EditorText = {
  markdown: string;
  title: string;
  tasks: TaskInput[];
};

type EditorClient = Pick<VaultClient, "scope" | "canEdit" | "saveNoteEditor">;

export function noteEditorDraftId(sessionId: string): string {
  return JSON.stringify(["note_editor", sessionId]);
}

export function editorRevision(snapshot: NoteEditorSnapshot): string {
  return JSON.stringify([snapshot.note.revision, snapshot.tasks.revision]);
}

export function editorText(snapshot: NoteEditorSnapshot): EditorText {
  return {
    markdown: snapshot.note.markdown ?? "",
    title: snapshot.note.title,
    tasks: snapshot.tasks.tasks.map(
      ({ id, source_order, status, text, body, due_at }) => ({
        id,
        source_order,
        status,
        text,
        body: structuredClone(body),
        due_at,
      }),
    ),
  };
}

function taskIntentKey(tasks: TaskInput[]): string {
  return hashKey([
    tasks
      .map(({ id, source_order, status, text, body, due_at }) => ({
        id,
        source_order,
        status,
        text,
        body,
        due_at,
      }))
      .sort(
        (a, b) => a.source_order - b.source_order || a.id.localeCompare(b.id),
      ),
  ]);
}

function sameText(draft: EditorText, snapshot: NoteEditorSnapshot): boolean {
  return (
    draft.markdown === (snapshot.note.markdown ?? "") &&
    draft.title === snapshot.note.title &&
    taskIntentKey(draft.tasks) === taskIntentKey(snapshot.tasks.tasks)
  );
}

function transport(
  client: EditorClient,
): DraftTransport<NoteEditorSnapshot, EditorText> {
  return {
    scope: client.scope,
    get canEdit() {
      return client.canEdit;
    },
    save: (id, draft, baseline, options) =>
      client.saveNoteEditor(
        id,
        {
          ...draft,
          expected_revision: baseline.note.revision,
          expected_tasks_revision: baseline.tasks.revision,
        },
        options,
      ),
  };
}

export class EditorConflictError extends DraftConflictError<NoteEditorSnapshot> {
  constructor(current: NoteEditorSnapshot) {
    super(current, editorRevision(current));
  }
}

export class NoteEditorDraft extends RevisionDraft<
  NoteEditorSnapshot,
  EditorText
> {
  constructor(
    client: EditorClient,
    sessionId: string,
    baseline: NoteEditorSnapshot,
    draft?: EditorText,
    uncertain = false,
  ) {
    super(
      transport(client),
      sessionId,
      {
        revision: editorRevision,
        project: editorText,
        matches: (draft, value) =>
          value.note.markdown === draft.markdown && sameText(draft, value),
        isDirty: (draft, value) => !sameText(draft, value),
        canAdvance: (baseline, value) =>
          baseline.note.revision === value.note.revision &&
          sameText(editorText(baseline), value),
        conflict: (value) => new EditorConflictError(value),
      },
      structuredClone(baseline),
      draft ? structuredClone(draft) : undefined,
      uncertain,
    );
  }

  rebind(client: EditorClient) {
    this.bind(transport(client));
  }

  edit(value: EditorText) {
    this.setDraft(structuredClone(value));
  }

  recovery(): NoteDraftRecovery {
    const value = this.recoveryState();
    return structuredClone({
      scope: this.scope,
      sessionId: noteEditorDraftId(this.sessionId),
      baseline: value.baseline.note,
      draft: { markdown: value.draft.markdown, title: value.draft.title },
      uncertain: value.uncertain,
      editorTasks: {
        baseline: value.baseline.tasks,
        draft: value.draft.tasks,
      },
    });
  }

  static restore(
    client: EditorClient,
    sessionId: string,
    recovery: NoteDraftRecovery,
  ): NoteEditorDraft {
    if (
      client.scope !== recovery.scope ||
      recovery.sessionId !== noteEditorDraftId(sessionId)
    )
      throw new Error(
        "This draft belongs to a different account, vault or note",
      );
    if (!recovery.editorTasks)
      throw new Error("This recovery copy does not contain editor task state");
    return new NoteEditorDraft(
      client,
      sessionId,
      { note: recovery.baseline, tasks: recovery.editorTasks.baseline },
      { ...recovery.draft, tasks: recovery.editorTasks.draft },
      recovery.uncertain,
    );
  }
}

export class NoteEditorDrafts extends DraftRegistry<NoteEditorDraft> {
  open(client: EditorClient, sessionId: string, snapshot: NoteEditorSnapshot) {
    const key = noteEditorDraftId(sessionId);
    const existing = this.get(client.scope, key);
    if (existing) {
      existing.rebind(client);
      existing.receive(snapshot);
      return existing;
    }
    return this.set(
      client.scope,
      key,
      new NoteEditorDraft(client, sessionId, snapshot),
    );
  }

  restore(
    client: EditorClient,
    sessionId: string,
    recovery: NoteDraftRecovery,
  ) {
    return this.set(
      client.scope,
      noteEditorDraftId(sessionId),
      NoteEditorDraft.restore(client, sessionId, recovery),
    );
  }
}

export const noteEditorDrafts = new NoteEditorDrafts();
