import { DraftRegistry } from "./draft-registry";
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

export class NoteDrafts extends DraftRegistry<NoteDraft> {
  open(
    client: NoteDraftClient,
    sessionId: string,
    note: NoteSnapshot,
  ): NoteDraft {
    let draft = this.get(client.scope, sessionId);
    if (draft) {
      draft.rebind(client);
      draft.receive(note);
    } else {
      draft = new NoteDraft(client, sessionId, note);
      this.set(client.scope, sessionId, draft);
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
    this.set(client.scope, recovery.sessionId, draft);
    return draft;
  }
}

export const noteDrafts = new NoteDrafts();
