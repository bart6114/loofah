import { create } from "zustand";
import { useShallow } from "zustand/shallow";

type DeletedSession = {
  id: string;
  title: string;
};

export type DeletedSessionData = {
  session: DeletedSession;
  tombstone: string;
  deletedAt: number;
};

export const UNDO_TIMEOUT_MS = 5000;

export type PendingDeletion = {
  token: string;
  scope: string;
  restore: (assertCurrent: () => void) => Promise<void>;
  data: DeletedSessionData;
  timeoutId: ReturnType<typeof setTimeout> | null;
  onDeleteConfirm: (() => void | Promise<unknown>) | null;
  addedAt: number;
  batchId: string | null;
};

interface UndoDeleteState {
  pendingDeletions: Record<string, PendingDeletion>;
  addDeletion: (
    scope: string,
    data: DeletedSessionData,
    options: {
      restore: PendingDeletion["restore"];
      onConfirm?: () => void | Promise<unknown>;
      batchId?: string;
    },
  ) => void;
  clearDeletion: (scope: string, sessionId: string) => void;
  confirmDeletion: (
    scope: string,
    sessionId: string,
  ) => void | Promise<unknown>;
  clearBatch: (scope: string, batchId: string) => void;
  confirmBatch: (scope: string, batchId: string) => void;
}

export function deletionKey(scope: string, sessionId: string) {
  return JSON.stringify([scope, sessionId]);
}

export function pendingDeletionsForScope(
  entries: Record<string, PendingDeletion>,
  scope: string,
) {
  return Object.fromEntries(
    Object.values(entries)
      .filter((entry) => entry.scope === scope)
      .map((entry) => [entry.data.session.id, entry]),
  );
}

export function useScopedPendingDeletions(scope: string) {
  return useUndoDelete(
    useShallow((state) =>
      pendingDeletionsForScope(state.pendingDeletions, scope),
    ),
  );
}

export const useUndoDelete = create<UndoDeleteState>((set, get) => ({
  pendingDeletions: {},

  addDeletion: (scope, data, options) => {
    const sessionId = data.session.id;
    const key = deletionKey(scope, sessionId);

    const existing = get().pendingDeletions[key];
    if (existing?.timeoutId) {
      clearTimeout(existing.timeoutId);
    }

    const entry: PendingDeletion = {
      token: crypto.randomUUID(),
      scope,
      restore: options.restore,
      data: { ...data, session: { ...data.session } },
      timeoutId: null,
      onDeleteConfirm: options.onConfirm ?? null,
      addedAt: Date.now(),
      batchId: options.batchId ?? null,
    };
    entry.timeoutId = setTimeout(() => {
      if (get().pendingDeletions[key] === entry)
        void get().confirmDeletion(scope, sessionId);
    }, UNDO_TIMEOUT_MS);
    set((state) => ({
      pendingDeletions: { ...state.pendingDeletions, [key]: entry },
    }));
  },

  clearDeletion: (scope, sessionId) => {
    const key = deletionKey(scope, sessionId);
    const pending = get().pendingDeletions[key];
    if (!pending) return;

    if (pending.timeoutId) {
      clearTimeout(pending.timeoutId);
    }

    set((state) => {
      const { [key]: _, ...rest } = state.pendingDeletions;
      return { pendingDeletions: rest };
    });
  },

  confirmDeletion: (scope, sessionId) => {
    const key = deletionKey(scope, sessionId);
    const pending = get().pendingDeletions[key];
    if (!pending) return;

    try {
      return pending.onDeleteConfirm?.();
    } finally {
      if (get().pendingDeletions[key] === pending)
        get().clearDeletion(scope, sessionId);
    }
  },

  clearBatch: (scope, batchId) => {
    const entries = Object.entries(get().pendingDeletions).filter(
      ([_, p]) => p.scope === scope && p.batchId === batchId,
    );
    for (const [, entry] of entries) {
      get().clearDeletion(scope, entry.data.session.id);
    }
  },

  confirmBatch: (scope, batchId) => {
    const entries = Object.entries(get().pendingDeletions).filter(
      ([_, p]) => p.scope === scope && p.batchId === batchId,
    );
    for (const [, entry] of entries) {
      void get().confirmDeletion(scope, entry.data.session.id);
    }
  },
}));

// App exit must not strand notes soft-deleted with live shared links: confirm
// every pending deletion now and let the caller await the finalize work.
export function confirmAllPendingDeletions(): Promise<void> {
  const { pendingDeletions, confirmDeletion } = useUndoDelete.getState();
  return Promise.allSettled(
    Object.values(pendingDeletions).map((entry) =>
      Promise.resolve().then(() => {
        if (
          useUndoDelete.getState().pendingDeletions[
            deletionKey(entry.scope, entry.data.session.id)
          ] === entry
        )
          return confirmDeletion(entry.scope, entry.data.session.id);
      }),
    ),
  ).then(() => undefined);
}
