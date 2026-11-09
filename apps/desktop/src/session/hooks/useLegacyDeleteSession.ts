import { emitTo, listen } from "@tauri-apps/api/event";
import { getAllWebviewWindows } from "@tauri-apps/api/webviewWindow";
import { useCallback, useEffect } from "react";

import { getCurrentWebviewWindowLabel } from "@hypr/plugin-windows";
import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { useLegacyDesktopAccess } from "~/main/use-legacy-activity";
import { trackPendingSoftDelete } from "~/session/pending-soft-deletes";
import { softDeleteSession, captureSessionRestore } from "~/session/queries";
import {
  captureSessionDeletion,
  createSessionLifecycleQueue,
} from "~/shared/application/session-lifecycle";
import { useTabs, useTabsStore } from "~/store/zustand/tabs";
import {
  type DeletedSessionData,
  type PendingDeletion,
  useUndoDelete,
  deletionKey,
} from "~/store/zustand/undo-delete";

export const SESSION_DELETED_FOR_UNDO_EVENT = "hypr://session-deleted-for-undo";

export type SessionDeletedForUndoPayload = {
  scope: string;
  sessionId: string;
  data: DeletedSessionData;
};

export async function closeSessionNoteWindows(
  sessionId: string,
  current: () => boolean,
) {
  try {
    const noteWindowLabel = `note-${sessionId}`;
    if (!current()) return;
    const windows = await getAllWebviewWindows();
    if (!current()) return;
    await Promise.all(
      windows
        .filter((window) => window.label === noteWindowLabel)
        .map((window) => window.close().catch(() => undefined)),
    );
  } catch {
    // Closing note windows should not block the deletion path.
  }
}

export function isSessionDeletedForUndoPayload(
  payload: unknown,
): payload is SessionDeletedForUndoPayload {
  if (
    !payload ||
    typeof payload !== "object" ||
    !("scope" in payload) ||
    typeof payload.scope !== "string" ||
    !("sessionId" in payload) ||
    typeof payload.sessionId !== "string" ||
    !("data" in payload) ||
    !payload.data ||
    typeof payload.data !== "object"
  )
    return false;
  const data = payload.data;
  if ("receipt" in data && data.receipt !== undefined) {
    const receipt = data.receipt;
    if (
      !receipt ||
      typeof receipt !== "object" ||
      !("requestId" in receipt) ||
      typeof receipt.requestId !== "string" ||
      !/^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/u.test(
        receipt.requestId,
      ) ||
      !("outcome" in receipt) ||
      !receipt.outcome ||
      typeof receipt.outcome !== "object"
    )
      return false;
    const outcome = receipt.outcome;
    if (
      !("kind" in outcome) ||
      outcome.kind !== "trashed" ||
      !("trash_token" in outcome) ||
      typeof outcome.trash_token !== "string" ||
      !/^[0-9a-f]{64}$/u.test(outcome.trash_token) ||
      !("revision" in outcome) ||
      typeof outcome.revision !== "string" ||
      !/^[0-9a-f]{64}$/u.test(outcome.revision)
    )
      return false;
  }
  return (
    "session" in data &&
    !!data.session &&
    typeof data.session === "object" &&
    "id" in data.session &&
    data.session.id === payload.sessionId &&
    "title" in data.session &&
    typeof data.session.title === "string" &&
    "tombstone" in data &&
    typeof data.tombstone === "string" &&
    "deletedAt" in data &&
    typeof data.deletedAt === "number" &&
    Number.isFinite(data.deletedAt)
  );
}

export function useLegacyDeleteSession() {
  const legacy = useLegacyDesktopAccess();
  const { isAvailable } = legacy;
  const tabsStore = useTabsStore();
  const invalidateResource = useTabs((state) => state.invalidateResource);
  const addDeletion = useUndoDelete((state) => state.addDeletion);
  const clearDeletion = useUndoDelete((state) => state.clearDeletion);

  return useCallback(
    (
      sessionId: string,
      options?: {
        batchId?: string;
        title?: string;
      },
    ) => {
      const current = () => isAvailable("legacy_desktop_session_controls");
      const captureState = legacy.activity?.captureState(sessionId);
      if (!current() || !captureState || captureState === "unknown") return;
      const assertCurrent = () => {
        const state = legacy.activity?.captureState(sessionId);
        if (!current() || !state || state === "unknown")
          throw new Error("Session controls are unavailable.");
      };
      const scope = legacy.scope.client.scope;
      const key = deletionKey(scope, sessionId);
      const restore = captureSessionRestore(scope);
      const { batchId, title } = options ?? {};
      // A repeat delete would replace the pending tombstone and its finalize
      // callback, then no-op in softDeleteSession and clear the undo toast —
      // leaving the note soft-deleted with no undo.
      if (useUndoDelete.getState().pendingDeletions[key]) {
        return;
      }
      const windowLabel = getCurrentWebviewWindowLabel();
      const isMainWindow = windowLabel === "main";
      const listenerState = legacy.listener!.getState();
      const live = listenerState.live;

      if (
        live.sessionId === sessionId &&
        (live.status === "active" || live.loading)
      ) {
        listenerState.stop();
      }

      // Optimistic path: hide the row, drop tab history, and show the undo
      // toast before the soft-delete commits; rolled back below on failure.
      const tombstone = new Date().toISOString();
      const hadOpenTab = tabsStore
        .getState()
        .tabs.some((tab) => tab.type === "sessions" && tab.id === sessionId);
      invalidateResource("sessions", sessionId);

      const commit = softDeleteSession(sessionId, tombstone, assertCurrent);
      trackPendingSoftDelete(scope, sessionId, commit);

      let optimistic: PendingDeletion | undefined;
      const clearOptimisticDeletion = () => {
        const pending = useUndoDelete.getState().pendingDeletions[key];
        if (pending && pending === optimistic) {
          clearDeletion(scope, sessionId);
        }
      };

      if (isMainWindow) {
        // No confirm callback: `session_delete` has already trashed the folder by the
        // time `commit` resolves, so letting the undo window lapse just dismisses the
        // toast. There is nothing to finalize (see session/queries.ts).
        addDeletion(
          scope,
          {
            session: { id: sessionId, title: title ?? "" },
            tombstone,
            deletedAt: Date.now(),
          },
          {
            batchId,
            owner: legacy.scope,
            restore: (assert) => restore(sessionId, assert),
          },
        );
        optimistic = useUndoDelete.getState().pendingDeletions[key];
      }

      let superseded = false;
      const stop = useUndoDelete.subscribe((next) => {
        const replacement = next.pendingDeletions[key];
        if (replacement && replacement !== optimistic) superseded = true;
      });
      void (async () => {
        let didDelete = false;
        try {
          const deletedData = await commit;
          if (!current() || superseded) return;
          if (!deletedData) {
            // The session was already deleted; drop the optimistic toast.
            if (isMainWindow) clearOptimisticDeletion();
            return;
          }
          didDelete = true;

          if (!isMainWindow) {
            await emitTo("main", SESSION_DELETED_FOR_UNDO_EVENT, {
              scope,
              sessionId,
              data: deletedData,
            } satisfies SessionDeletedForUndoPayload);
          }
        } catch (error) {
          console.error("[delete-session] failed to finish deletion", error);
          if (!didDelete && current() && !superseded) {
            if (isMainWindow) clearOptimisticDeletion();
            if (hadOpenTab) {
              tabsStore
                .getState()
                .openCurrent({ type: "sessions", id: sessionId });
            }
            sonnerToast.error("Could not delete this note. Please try again.");
          }
          // The `didDelete` branch needs no cleanup: the folder is already in
          // `.trash/`, whether or not main ever heard about this delete.
        } finally {
          try {
            if (didDelete && current() && !superseded)
              await closeSessionNoteWindows(
                sessionId,
                () => current() && !superseded,
              );
          } finally {
            stop();
          }
        }
      })();
    },
    [
      invalidateResource,
      addDeletion,
      clearDeletion,
      tabsStore,
      isAvailable,
      legacy.activity,
      legacy.listener,
      legacy.scope,
    ],
  );
}

export function useLegacySessionDeletionUndoListener(active: boolean) {
  const legacy = useLegacyDesktopAccess();
  const tabsStore = useTabsStore();
  const { isAvailable } = legacy;
  const available = isAvailable("legacy_desktop_session_controls");
  const invalidateResource = useTabs((state) => state.invalidateResource);
  const addDeletion = useUndoDelete((state) => state.addDeletion);

  useEffect(() => {
    if (!active || !available) {
      return;
    }

    const scope = legacy.scope.client.scope;
    const restore = captureSessionRestore(scope);
    const queue = createSessionLifecycleQueue();
    let disposed = false;
    const current = () =>
      !disposed && isAvailable("legacy_desktop_session_controls");
    let unlisten: (() => void) | undefined;

    void listen(SESSION_DELETED_FOR_UNDO_EVENT, (event) => {
      if (!current()) return;
      const payload = event.payload;
      if (!isSessionDeletedForUndoPayload(payload) || payload.scope !== scope) {
        return;
      }

      const existing =
        useUndoDelete.getState().pendingDeletions[
          deletionKey(scope, payload.sessionId)
        ];
      if (
        existing &&
        (existing.data.tombstone === payload.data.tombstone ||
          existing.data.deletedAt >= payload.data.deletedAt)
      )
        return;
      const data = payload.data;
      const sessionId = payload.sessionId;
      if (data.receipt) {
        if (!legacy.scope.client.features?.includes("session_lifecycle"))
          return;
        const intent = captureSessionDeletion(
          legacy.scope,
          tabsStore,
          sessionId,
          queue,
          data.receipt,
        );
        const review = () => {
          if (!current() || !intent.isCurrent()) return;
          sonnerToast.error(
            "Could not confirm undo. Retry the original restore request.",
            {
              duration: Infinity,
              action: {
                label: "Retry undo",
                onClick: (event) => {
                  event.preventDefault();
                  if (current())
                    void intent.restore().then(() => intent.release(), review);
                },
              },
            },
          );
        };
        void queue
          .run(data.receipt.requestId, () => intent.confirmDeleted())
          .then(
            (absent) => {
              const currentEntry =
                useUndoDelete.getState().pendingDeletions[
                  deletionKey(scope, sessionId)
                ];
              if (
                !absent ||
                !current() ||
                (currentEntry && currentEntry !== existing)
              ) {
                intent.release();
                return;
              }
              invalidateResource("sessions", sessionId);
              addDeletion(scope, data, {
                owner: legacy.scope,
                onConfirm: () => {
                  if (intent.unresolved()) review();
                  else intent.release();
                },
                restore: async (assertCurrent) => {
                  assertCurrent();
                  await intent.restore();
                  assertCurrent();
                  intent.release();
                },
              });
              void closeSessionNoteWindows(sessionId, current);
            },
            () => intent.release(),
          );
        return;
      }
      invalidateResource("sessions", payload.sessionId);
      addDeletion(scope, data, {
        owner: legacy.scope,
        restore: (assert) => restore(sessionId, assert),
      });
      void closeSessionNoteWindows(payload.sessionId, current);
    }).then((fn) => {
      if (!current()) fn();
      else unlisten = fn;
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [
    active,
    invalidateResource,
    addDeletion,
    isAvailable,
    available,
    legacy.scope,
    tabsStore,
  ]);
}
