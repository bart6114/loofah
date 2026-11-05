import { ApplicationRequestError } from "./errors";
import type { NoteSnapshot } from "./protocol.gen";
import type { RequestOptions, VaultClient } from "./vault-client";

import type { NoteDraftRecovery } from "~/types/tauri.gen";

export class NoteConflictError extends ApplicationRequestError {
  constructor(readonly current: NoteSnapshot) {
    super({
      code: "conflict",
      message:
        "This content changed in another client. Review both versions before saving.",
      request_id: null,
      retryable: false,
      current_revision: current.revision,
      mutation_outcome: "not_committed",
    });
  }
}

export type { NoteDraftRecovery } from "~/types/tauri.gen";

export type NoteDraftClient = Pick<
  VaultClient,
  "scope" | "canEdit" | "readNote" | "saveNote"
>;

export class NoteDraft {
  private client: NoteDraftClient;
  private snapshot: {
    baseline: NoteSnapshot;
    draft: { markdown: string; title: string };
    conflict: NoteSnapshot | null;
    dirty: boolean;
  };
  private tail: Promise<unknown> = Promise.resolve();
  private epoch = 0;
  private pending = 0;
  private uncertain = false;
  private captured?: () => { markdown: string; title: string };
  private readonly listeners = new Set<() => void>();

  constructor(
    client: NoteDraftClient,
    readonly sessionId: string,
    note: NoteSnapshot,
    draft?: { markdown: string; title: string },
    uncertain = false,
  ) {
    this.client = client;
    this.uncertain = uncertain;
    this.snapshot = {
      baseline: note,
      draft: draft ?? { markdown: note.markdown ?? "", title: note.title },
      conflict: null,
      dirty: false,
    };
    this.update();
  }

  get scope(): string {
    return this.client.scope;
  }
  getSnapshot = () => this.snapshot;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  rebind(client: NoteDraftClient) {
    if (client.scope !== this.scope)
      throw new Error("Cannot move a draft to a different account or vault");
    this.client = client;
  }

  capture(getDraft: () => { markdown: string; title: string }) {
    this.captured = getDraft;
    if (!this.snapshot.dirty) this.update();
  }

  edit(markdown: string, title: string) {
    this.captured = undefined;
    this.snapshot = { ...this.snapshot, draft: { markdown, title } };
    this.update();
  }

  receive(note: NoteSnapshot) {
    if (
      this.uncertain &&
      !this.captured &&
      this.pending === 0 &&
      note.markdown === this.snapshot.draft.markdown &&
      note.title === this.snapshot.draft.title
    ) {
      this.uncertain = false;
      this.snapshot = { ...this.snapshot, baseline: note, conflict: null };
      this.update();
      return;
    }
    if (this.snapshot.dirty || this.pending > 0) {
      if (note.revision !== this.snapshot.baseline.revision) {
        this.snapshot = { ...this.snapshot, conflict: note };
        this.update();
      }
    } else {
      this.snapshot = {
        baseline: note,
        draft: { markdown: note.markdown ?? "", title: note.title },
        conflict: null,
        dirty: false,
      };
      this.update();
    }
  }

  useServerVersion(reviewedRevision: string) {
    const current = this.reviewed(reviewedRevision);
    this.epoch += 1;
    this.uncertain = false;
    this.captured = undefined;
    this.snapshot = {
      baseline: current,
      draft: { markdown: current.markdown ?? "", title: current.title },
      conflict: null,
      dirty: false,
    };
    this.update();
  }

  keepDraft(reviewedRevision: string) {
    const current = this.reviewed(reviewedRevision);
    this.materialize();
    this.epoch += 1;
    this.uncertain = false;
    this.snapshot = { ...this.snapshot, baseline: current, conflict: null };
    this.update();
  }

  save(options?: RequestOptions): Promise<NoteSnapshot> {
    this.materialize();
    const client = this.client;
    const draft = { ...this.snapshot.draft };
    const epoch = this.epoch;
    this.pending += 1;
    const superseded = () =>
      new ApplicationRequestError({
        code: "cancelled",
        message: "This save belongs to an earlier draft",
        request_id: options?.requestId ?? null,
        retryable: false,
        current_revision: null,
        mutation_outcome: "unknown",
      });
    const operation = this.tail
      .catch(() => {})
      .then(async () => {
        if (epoch !== this.epoch) throw superseded();
        if (!client.canEdit)
          throw new ApplicationRequestError({
            code: "forbidden",
            message: "This vault is read-only",
            request_id: null,
            retryable: false,
            current_revision: null,
            mutation_outcome: "not_committed",
          });
        if (this.snapshot.conflict)
          throw new NoteConflictError(this.snapshot.conflict);
        const outcome = await client.saveNote(
          this.sessionId,
          { ...draft, expected_revision: this.snapshot.baseline.revision },
          options,
        );
        if (epoch !== this.epoch) throw superseded();
        if (
          outcome.kind === "conflict" &&
          !(
            outcome.note.markdown === draft.markdown &&
            outcome.note.title === draft.title
          )
        ) {
          this.snapshot = { ...this.snapshot, conflict: outcome.note };
          this.update();
          throw new NoteConflictError(outcome.note);
        }
        const observedConflict = this.getSnapshot().conflict;
        const conflict =
          observedConflict?.revision === outcome.note.revision
            ? null
            : observedConflict;
        this.uncertain = false;
        this.snapshot = { ...this.snapshot, baseline: outcome.note, conflict };
        this.update();
        return outcome.note;
      });
    const completed = operation
      .catch((error: unknown) => {
        if (
          epoch === this.epoch &&
          !(error instanceof NoteConflictError) &&
          (!(error instanceof ApplicationRequestError) ||
            error.detail.mutation_outcome !== "not_committed")
        ) {
          this.uncertain = true;
          this.update();
        }
        throw error;
      })
      .finally(() => {
        this.pending -= 1;
      });
    this.tail = completed;
    return completed;
  }

  recovery(): NoteDraftRecovery {
    this.materialize();
    return {
      scope: this.scope,
      sessionId: this.sessionId,
      baseline: this.snapshot.baseline,
      draft: this.snapshot.draft,
      uncertain: this.uncertain || this.pending > 0,
    };
  }

  private materialize() {
    const captured = this.captured;
    if (captured) {
      const draft = captured();
      this.captured = undefined;
      this.snapshot = { ...this.snapshot, draft };
      this.update();
    }
  }

  private reviewed(revision: string): NoteSnapshot {
    const current = this.snapshot.conflict;
    if (!current || current.revision !== revision)
      throw new Error(
        "Review the latest server version before resolving this conflict",
      );
    return current;
  }

  private update() {
    const { draft, baseline } = this.snapshot;
    this.snapshot = {
      ...this.snapshot,
      dirty:
        Boolean(this.captured) ||
        this.uncertain ||
        draft.markdown !== (baseline.markdown ?? "") ||
        draft.title !== baseline.title,
    };
    for (const listener of this.listeners) listener();
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
