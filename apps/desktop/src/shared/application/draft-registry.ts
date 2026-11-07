import type { NoteDraftRecovery } from "~/types/tauri.gen";

export class DraftRegistry<
  D extends {
    scope: string;
    getSnapshot: () => { dirty: boolean };
    recovery: () => NoteDraftRecovery;
  },
> {
  private readonly drafts = new Map<string, D>();
  private readonly scopes = new Map<string, number>();
  private readonly mounted = new Map<string, number>();

  retainDraft(scope: string, id: string, draft: D) {
    if (draft.scope !== scope)
      throw new Error("Cannot retain a draft in a different account or vault");
    const key = JSON.stringify([scope, id]);
    this.set(scope, id, draft);
    this.mounted.set(key, (this.mounted.get(key) ?? 0) + 1);
    let released = false;
    return () => {
      if (released) return;
      released = true;
      const remaining = (this.mounted.get(key) ?? 1) - 1;
      if (remaining) this.mounted.set(key, remaining);
      else this.mounted.delete(key);
      this.sweepDraft(scope, id);
    };
  }

  sweepDraft(scope: string, id: string) {
    const key = JSON.stringify([scope, id]);
    const draft = this.drafts.get(key);
    if (
      draft &&
      !this.mounted.has(key) &&
      !draft.getSnapshot().dirty &&
      !draft.recovery().uncertain
    )
      this.drafts.delete(key);
  }

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
  get(scope: string, id: string): D | undefined {
    return this.drafts.get(JSON.stringify([scope, id]));
  }
  protected set(scope: string, id: string, draft: D) {
    this.drafts.set(JSON.stringify([scope, id]), draft);
    return draft;
  }
}
