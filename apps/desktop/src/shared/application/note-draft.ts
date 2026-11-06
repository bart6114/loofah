import type { NoteSnapshot } from "./protocol.gen";
import {
  DraftConflictError,
  RevisionDraft,
  type DraftTransport,
} from "./revision-draft";
import type { VaultClient } from "./vault-client";

import type { NoteDraftRecovery } from "~/types/tauri.gen";

export class NoteConflictError extends DraftConflictError<NoteSnapshot> {
  constructor(current: NoteSnapshot) {
    super(current, current.revision);
  }
}

export type { NoteDraftRecovery } from "~/types/tauri.gen";

export type NoteDraftClient = Pick<
  VaultClient,
  "scope" | "canEdit" | "readNote" | "saveNote"
>;

type NoteText = NoteDraftRecovery["draft"];

function transport(
  client: NoteDraftClient,
): DraftTransport<NoteSnapshot, NoteText> {
  return {
    scope: client.scope,
    get canEdit() {
      return client.canEdit;
    },
    save: async (id, draft, baseline, options) => {
      const result = await client.saveNote(
        id,
        {
          ...draft,
          expected_revision: baseline.revision,
        },
        options,
      );
      return { kind: result.kind, snapshot: result.note };
    },
  };
}

export class NoteDraft extends RevisionDraft<NoteSnapshot, NoteText> {
  constructor(
    client: NoteDraftClient,
    sessionId: string,
    note: NoteSnapshot,
    draft?: NoteText,
    uncertain = false,
  ) {
    super(
      transport(client),
      sessionId,
      {
        revision: (value) => value.revision,
        project: (value) => ({
          markdown: value.markdown ?? "",
          title: value.title,
        }),
        matches: (draft, value) =>
          draft.markdown === value.markdown && draft.title === value.title,
        isDirty: (draft, value) =>
          draft.markdown !== (value.markdown ?? "") ||
          draft.title !== value.title,
        conflict: (value) => new NoteConflictError(value),
      },
      note,
      draft,
      uncertain,
    );
  }

  rebind(client: NoteDraftClient) {
    this.bind(transport(client));
  }

  edit(markdown: string, title: string) {
    this.setDraft({ markdown, title });
  }

  recovery(): NoteDraftRecovery {
    return {
      scope: this.scope,
      sessionId: this.sessionId,
      ...this.recoveryState(),
    };
  }
}

export class NoteDrafts {
  private readonly drafts = new Map<string, NoteDraft>();
  private readonly scopes = new Map<string, number>();

  retainScope(scope: string) {
    this.scopes.set(scope, (this.scopes.get(scope) ?? 0) + 1);
  }
  releaseScope(scope: string) {
    const remaining = Math.max(0, (this.scopes.get(scope) ?? 0) - 1);
    if (remaining) this.scopes.set(scope, remaining);
    else {
      this.scopes.delete(scope);
      this.sweep(scope);
    }
  }
  clearSaved(scope: string) {
    for (const [key, draft] of this.drafts) {
      if (
        draft.scope === scope &&
        !draft.getSnapshot().dirty &&
        !draft.recovery().uncertain
      )
        this.drafts.delete(key);
    }
  }
  sweep(scope: string) {
    if (!this.scopes.has(scope)) this.clearSaved(scope);
  }

  get(scope: string, sessionId: string): NoteDraft | undefined {
    return this.drafts.get(JSON.stringify([scope, sessionId]));
  }

  open(
    client: NoteDraftClient,
    sessionId: string,
    note: NoteSnapshot,
  ): NoteDraft {
    const key = JSON.stringify([client.scope, sessionId]);
    let draft = this.drafts.get(key);
    if (draft) {
      draft.rebind(client);
      draft.receive(note);
    } else {
      draft = new NoteDraft(client, sessionId, note);
      this.drafts.set(key, draft);
    }
    return draft;
  }

  restore(client: NoteDraftClient, recovery: NoteDraftRecovery): NoteDraft {
    if (recovery.editorTasks)
      throw new Error(
        "This recovery copy requires the editor with task support",
      );
    if (client.scope !== recovery.scope)
      throw new Error("This draft belongs to a different account or vault");
    const draft = new NoteDraft(
      client,
      recovery.sessionId,
      recovery.baseline,
      recovery.draft,
      recovery.uncertain,
    );
    this.drafts.set(JSON.stringify([client.scope, recovery.sessionId]), draft);
    return draft;
  }
}
