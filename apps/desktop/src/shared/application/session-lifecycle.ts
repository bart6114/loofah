import { onlineManager } from "@tanstack/react-query";

import { ApplicationRequestError } from "./errors";
import type {
  DeleteSessionOutcome,
  DeleteSessionRequest,
  RestoreSessionOutcome,
  RestoreSessionRequest,
} from "./protocol.gen";
import type { VaultScope } from "./vault-scope";

import { uniqueIdfromTab, type ScopedTabsStore } from "~/store/zustand/tabs";

const terminal = new Set([
  "access_denied",
  "login_required",
  "closed",
  "incompatible",
]);

function refused(
  message: string,
  outcome: "not_committed" | "unknown" = "not_committed",
) {
  return new ApplicationRequestError({
    code: "unavailable",
    message,
    request_id: null,
    retryable: false,
    current_revision: null,
    mutation_outcome: outcome,
  });
}
function uncertain(error: unknown) {
  return !(
    error instanceof ApplicationRequestError &&
    error.detail.mutation_outcome === "not_committed"
  );
}
export function sessionDeletionAvailable(
  scope: VaultScope,
  store: ScopedTabsStore,
) {
  return (
    store.scope === scope.client.scope &&
    scope.getSnapshot().state === "connected" &&
    scope.client.canEdit &&
    scope.client.features.includes("session_lifecycle") &&
    (scope.client.networkMode === "always" || onlineManager.isOnline())
  );
}
export function createSessionLifecycleQueue() {
  let active = 0;
  const waiting: Array<() => void> = [];
  const unresolved = new Set<string>();
  const pump = () => {
    while (active < 2 && waiting.length) waiting.shift()!();
  };
  return {
    unresolved,
    run<T>(requestId: string, operation: () => Promise<T>) {
      return new Promise<T>((resolve, reject) => {
        waiting.push(() => {
          if (unresolved.size >= 32 && !unresolved.has(requestId)) {
            reject(
              refused(
                "Review pending deletion requests before deleting more notes.",
              ),
            );
            return;
          }
          active++;
          void Promise.resolve()
            .then(operation)
            .then(resolve, reject)
            .finally(() => {
              active--;
              pump();
            });
        });
        pump();
      });
    },
  };
}
export type SessionDeletionReceipt = {
  requestId: string;
  outcome: Extract<DeleteSessionOutcome, { kind: "trashed" }>;
};

export class SessionLifecycleReviewError extends Error {
  constructor(readonly outcome: DeleteSessionOutcome | RestoreSessionOutcome) {
    super("The note changed. Review its current state before trying again.");
  }
}
export function captureSessionDeletion(
  scope: VaultScope,
  store: ScopedTabsStore,
  sessionId: string,
  queue = createSessionLifecycleQueue(),
  receipt?: SessionDeletionReceipt,
) {
  const requestId = receipt?.requestId ?? crypto.randomUUID();
  let input: DeleteSessionRequest | undefined = receipt
    ? { session_id: sessionId, expected_revision: receipt.outcome.revision }
    : undefined;
  let outcome: DeleteSessionOutcome | undefined = receipt?.outcome;
  let pending: Promise<DeleteSessionOutcome> | undefined;
  let sent = !!receipt;
  let unknown = false;
  let cancelled = false;
  let undoRequested = false;
  let retired = terminal.has(scope.getSnapshot().state);
  let hidden = false;
  let reopened = false;
  let destinationChanged = false;
  let destination: { slotId: string; id: string } | null = null;
  let restoreInput: RestoreSessionRequest | undefined;
  let restoreId: string | undefined;
  let restorePending: Promise<void> | undefined;
  let restoreOutcome: RestoreSessionOutcome | undefined;
  let stopStore: (() => void) | undefined;
  let onRelease: (() => void) | undefined;
  const hadOpenTab = store
    .getState()
    .tabs.some((tab) => tab.type === "sessions" && tab.id === sessionId);
  const stopScope = scope.subscribe(() => {
    if (terminal.has(scope.getSnapshot().state)) {
      retired = true;
      release();
    }
  });
  function release() {
    retired = true;
    onRelease?.();
    stopStore?.();
    stopScope();
  }
  const isCurrent = () => !retired && !terminal.has(scope.getSnapshot().state);
  const writable = () => isCurrent() && sessionDeletionAvailable(scope, store);
  const check = () => {
    if (!writable()) throw refused("Session controls are unavailable.");
  };
  const finish = (value: DeleteSessionOutcome) => {
    outcome = value;
    unknown = false;
    queue.unresolved.delete(requestId);
    return value;
  };
  function submit(): Promise<DeleteSessionOutcome> {
    if (pending) return pending;
    if (outcome) return Promise.resolve(outcome);
    pending = queue
      .run(requestId, async () => {
        check();
        if (cancelled || (!sent && reopened))
          throw refused("Deletion was cancelled before being sent.");
        if (!input) {
          const snapshot = await scope.run(() =>
            scope.client.readSessionLifecycle(sessionId),
          );
          check();
          if (snapshot.session_id !== sessionId)
            throw refused("The note owner changed.");
          input = {
            session_id: sessionId,
            expected_revision: snapshot.revision,
          };
        }
        check();
        if (cancelled || (!sent && reopened))
          throw refused("Deletion was cancelled before being sent.");
        const prepared = input;
        sent = true;
        try {
          const result = await scope.run(() =>
            scope.client.deleteSession(prepared, { requestId }),
          );
          if (!isCurrent())
            throw refused("The original vault owner was retired.", "unknown");
          return finish(result);
        } catch (error) {
          if (unknown || !isCurrent() || uncertain(error)) {
            unknown = true;
            queue.unresolved.add(requestId);
          }
          throw error;
        }
      })
      .finally(() => {
        pending = undefined;
      });
    return pending;
  }
  async function confirmDeleted() {
    if (!isCurrent() || outcome?.kind !== "trashed") return false;
    try {
      await scope.run(() => scope.client.readSessionLifecycle(sessionId));
      return false;
    } catch (error) {
      return (
        isCurrent() &&
        error instanceof ApplicationRequestError &&
        error.detail.code === "not_found"
      );
    }
  }
  function restore(): Promise<void> {
    undoRequested = true;
    if (restorePending) return restorePending;
    if (!sent) cancelled = true;
    restorePending = (async () => {
      if (pending) {
        try {
          await pending;
        } catch (error) {
          if (sent) throw error;
        }
      }
      check();
      if (!sent) return;
      if (!outcome)
        throw refused(
          "Resolve the original deletion request before restoring this note.",
        );
      if (outcome.kind === "not_found") return;
      if (outcome.kind !== "trashed")
        throw new SessionLifecycleReviewError(outcome);
      restoreId ??= crypto.randomUUID();
      restoreInput ??= {
        trash_token: outcome.trash_token,
        expected_revision: outcome.revision,
      };
      const id = restoreId;
      const prepared = restoreInput;
      await queue.run(id, async () => {
        check();
        try {
          if (!restoreOutcome) {
            const result = await scope.run(() =>
              scope.client.restoreSession(prepared, { requestId: id }),
            );
            if (!isCurrent())
              throw refused("The original vault owner was retired.", "unknown");
            if (result.kind === "restored" && result.session_id !== sessionId)
              throw new ApplicationRequestError({
                code: "incompatible_version",
                message: "The restored note owner changed.",
                request_id: id,
                retryable: false,
                current_revision: null,
                mutation_outcome: "unknown",
              });
            restoreOutcome = result;
            queue.unresolved.delete(id);
          }
          if (restoreOutcome.kind === "conflict")
            throw new SessionLifecycleReviewError(restoreOutcome);
          if (restoreOutcome.kind !== "restored")
            throw new SessionLifecycleReviewError(restoreOutcome);
          check();
          const canonical = await scope.run(() =>
            scope.client.readSessionLifecycle(sessionId),
          );
          check();
          if (canonical.session_id !== sessionId)
            throw refused("The restored note is no longer available.");
        } catch (error) {
          if (!restoreOutcome && (!isCurrent() || uncertain(error)))
            queue.unresolved.add(id);
          throw error;
        }
      });
    })().finally(() => {
      restorePending = undefined;
    });
    return restorePending;
  }
  return {
    scope,
    store,
    queue,
    sessionId,
    requestId,
    submit,
    restore,
    confirmDeleted,
    release,
    isCurrent,
    uncertain: () => unknown,
    unresolved: () =>
      unknown || (!!restoreId && queue.unresolved.has(restoreId)),
    onRelease: (callback: () => void) => {
      onRelease = callback;
    },
    pending: () => !!pending || !!restorePending,
    undoRequested: () => undoRequested,
    canCloseWindows: () => isCurrent() && !reopened,
    hide() {
      if (hidden || !isCurrent()) return;
      hidden = true;
      store.getState().invalidateResource("sessions", sessionId);
      const current = store.getState().currentTab;
      destination = current
        ? { slotId: current.slotId, id: uniqueIdfromTab(current) }
        : null;
      stopStore = store.subscribe((state) => {
        const current = state.currentTab;
        if (
          destination
            ? current?.slotId !== destination.slotId ||
              uniqueIdfromTab(current) !== destination.id
            : current !== null
        )
          destinationChanged = true;
        if (
          state.tabs.some(
            (tab) => tab.type === "sessions" && tab.id === sessionId,
          )
        )
          reopened = true;
      });
    },
    rollback() {
      if (
        !isCurrent() ||
        !hadOpenTab ||
        destinationChanged ||
        reopened ||
        outcome?.kind === "not_found"
      )
        return;
      const current = store.getState().currentTab;
      if (
        destination
          ? current?.slotId === destination.slotId &&
            uniqueIdfromTab(current) === destination.id
          : current === null
      )
        store.getState().openCurrent({ type: "sessions", id: sessionId });
    },
  };
}
export type SessionDeletionIntent = ReturnType<typeof captureSessionDeletion>;
