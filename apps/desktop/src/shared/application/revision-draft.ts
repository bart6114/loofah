import { ApplicationRequestError } from "./errors";
import type { RequestOptions } from "./vault-client";

export class DraftConflictError<T> extends ApplicationRequestError {
  constructor(
    readonly current: T,
    revision: string,
  ) {
    super({
      code: "conflict",
      message:
        "This content changed in another client. Review both versions before saving.",
      request_id: null,
      retryable: false,
      current_revision: revision,
      mutation_outcome: "not_committed",
    });
  }
}

export type DraftTransport<S, D> = {
  scope: string;
  canEdit: boolean;
  save: (
    id: string,
    draft: D,
    baseline: S,
    options?: RequestOptions,
  ) => Promise<{ kind: "saved" | "conflict"; snapshot: S }>;
};

export class RevisionDraft<S, D> {
  private snapshot: {
    baseline: S;
    draft: D;
    conflict: S | null;
    dirty: boolean;
  };
  private tail: Promise<unknown> = Promise.resolve();
  private epoch = 0;
  private pending = 0;
  private uncertain: boolean;
  private captured?: () => D;
  private readonly listeners = new Set<() => void>();

  constructor(
    private client: DraftTransport<S, D>,
    readonly sessionId: string,
    private readonly contract: {
      revision: (snapshot: S) => string;
      project: (snapshot: S) => D;
      matches: (draft: D, snapshot: S) => boolean;
      isDirty?: (draft: D, snapshot: S) => boolean;
      conflict: (snapshot: S) => DraftConflictError<S>;
    },
    baseline: S,
    draft?: D,
    uncertain = false,
  ) {
    this.uncertain = uncertain;
    this.snapshot = {
      baseline,
      draft: draft ?? contract.project(baseline),
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

  protected bind(client: DraftTransport<S, D>) {
    if (client.scope !== this.scope)
      throw new Error("Cannot move a draft to a different account or vault");
    this.client = client;
  }

  capture(getDraft: () => D) {
    this.captured = getDraft;
    if (!this.snapshot.dirty) this.update();
  }

  protected setDraft(draft: D) {
    this.captured = undefined;
    this.snapshot = { ...this.snapshot, draft };
    this.update();
  }

  receive(value: S) {
    if (
      this.uncertain &&
      !this.captured &&
      this.pending === 0 &&
      this.contract.matches(this.snapshot.draft, value)
    ) {
      this.uncertain = false;
      this.snapshot = { ...this.snapshot, baseline: value, conflict: null };
      this.update();
      return;
    }
    if (this.snapshot.dirty || this.pending > 0) {
      if (
        this.contract.revision(value) !==
        this.contract.revision(this.snapshot.baseline)
      ) {
        this.snapshot = { ...this.snapshot, conflict: value };
        this.update();
      }
    } else {
      this.snapshot = {
        baseline: value,
        draft: this.contract.project(value),
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
      draft: this.contract.project(current),
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

  save(options?: RequestOptions): Promise<S> {
    this.materialize();
    const client = this.client;
    const draft = structuredClone(this.snapshot.draft);
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
          throw this.contract.conflict(this.snapshot.conflict);
        // A manual save and the editor's trailing flush can contain the same
        // revision. Reuse its confirmed result instead of writing it twice.
        if (
          !this.uncertain &&
          this.contract.matches(draft, this.snapshot.baseline)
        )
          return this.snapshot.baseline;
        const outcome = await client.save(
          this.sessionId,
          draft,
          this.snapshot.baseline,
          options,
        );
        if (epoch !== this.epoch) throw superseded();
        if (
          outcome.kind === "conflict" &&
          !this.contract.matches(draft, outcome.snapshot)
        ) {
          this.snapshot = { ...this.snapshot, conflict: outcome.snapshot };
          this.update();
          throw this.contract.conflict(outcome.snapshot);
        }
        const observed = this.snapshot.conflict;
        const conflict =
          observed &&
          this.contract.revision(observed) !==
            this.contract.revision(outcome.snapshot)
            ? observed
            : null;
        this.uncertain = false;
        this.snapshot = {
          ...this.snapshot,
          baseline: outcome.snapshot,
          conflict,
        };
        this.update();
        return outcome.snapshot;
      });
    const completed = operation
      .catch((error: unknown) => {
        if (
          epoch === this.epoch &&
          !(error instanceof DraftConflictError) &&
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

  protected recoveryState() {
    this.materialize();
    return {
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

  private reviewed(revision: string): S {
    const current = this.snapshot.conflict;
    if (!current || this.contract.revision(current) !== revision)
      throw new Error(
        "Review the latest server version before resolving this conflict",
      );
    return current;
  }

  private update() {
    this.snapshot = {
      ...this.snapshot,
      dirty:
        Boolean(this.captured) ||
        this.uncertain ||
        (this.contract.isDirty?.(this.snapshot.draft, this.snapshot.baseline) ??
          !this.contract.matches(this.snapshot.draft, this.snapshot.baseline)),
    };
    for (const listener of this.listeners) listener();
  }
}
