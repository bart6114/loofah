import { emitTo, listen } from "@tauri-apps/api/event";
import { getAllWebviewWindows } from "@tauri-apps/api/webviewWindow";
import { useCallback, useEffect } from "react";

import { getCurrentWebviewWindowLabel } from "@hypr/plugin-windows";
import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { trackPendingSoftDelete } from "~/session/pending-soft-deletes";
import { softDeleteSession } from "~/session/queries";
import { listenerStore } from "~/store/zustand/listener/instance";
import { useTabs, useTabsStore } from "~/store/zustand/tabs";
import {
  type DeletedSessionData,
  useUndoDelete,
} from "~/store/zustand/undo-delete";

const SESSION_DELETED_FOR_UNDO_EVENT = "hypr://session-deleted-for-undo";

type SessionDeletedForUndoPayload = {
  sessionId: string;
  data: DeletedSessionData;
};

async function closeSessionNoteWindows(sessionId: string) {
  try {
    const noteWindowLabel = `note-${sessionId}`;
    const windows = await getAllWebviewWindows();
    await Promise.all(
      windows
        .filter((window) => window.label === noteWindowLabel)
        .map((window) => window.close().catch(() => undefined)),
    );
  } catch {
    // Closing note windows should not block the deletion path.
  }
}

function isSessionDeletedForUndoPayload(
  payload: unknown,
): payload is SessionDeletedForUndoPayload {
  return (
    typeof payload === "object" &&
    payload !== null &&
    "sessionId" in payload &&
    typeof payload.sessionId === "string" &&
    "data" in payload &&
    typeof payload.data === "object" &&
    payload.data !== null
  );
}

export function useDeleteSession() {
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
      const { batchId, title } = options ?? {};
      // A repeat delete would replace the pending tombstone and its finalize
      // callback, then no-op in softDeleteSession and clear the undo toast —
      // leaving the note soft-deleted with no undo.
      if (useUndoDelete.getState().pendingDeletions[sessionId]) {
        return;
      }
      const windowLabel = getCurrentWebviewWindowLabel();
      const isMainWindow = windowLabel === "main";
      const listenerState = listenerStore.getState();
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

      const commit = softDeleteSession(sessionId, tombstone);
      trackPendingSoftDelete(sessionId, commit);

      const clearOptimisticDeletion = () => {
        const pending = useUndoDelete.getState().pendingDeletions[sessionId];
        if (pending?.data.tombstone === tombstone) {
          clearDeletion(sessionId);
        }
      };

      if (isMainWindow) {
        // No confirm callback: `session_delete` has already trashed the folder by the
        // time `commit` resolves, so letting the undo window lapse just dismisses the
        // toast. There is nothing to finalize (see session/queries.ts).
        addDeletion(
          {
            session: { id: sessionId, title: title ?? "" },
            tombstone,
            deletedAt: Date.now(),
          },
          undefined,
          batchId,
        );
      }

      void (async () => {
        let didDelete = false;
        try {
          const deletedData = await commit;
          if (!deletedData) {
            // The session was already deleted; drop the optimistic toast.
            if (isMainWindow) clearOptimisticDeletion();
            return;
          }
          didDelete = true;

          if (!isMainWindow) {
            await emitTo("main", SESSION_DELETED_FOR_UNDO_EVENT, {
              sessionId,
              data: deletedData,
            } satisfies SessionDeletedForUndoPayload);
          }
        } catch (error) {
          console.error("[delete-session] failed to finish deletion", error);
          if (!didDelete) {
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
          if (didDelete) {
            await closeSessionNoteWindows(sessionId);
          }
        }
      })();
    },
    [invalidateResource, addDeletion, clearDeletion, tabsStore],
  );
}

export function useRemoteSessionDeletionUndoListener(active: boolean) {
  const invalidateResource = useTabs((state) => state.invalidateResource);
  const addDeletion = useUndoDelete((state) => state.addDeletion);

  useEffect(() => {
    if (!active) {
      return;
    }

    let unlisten: (() => void) | undefined;

    void listen(SESSION_DELETED_FOR_UNDO_EVENT, (event) => {
      const payload = event.payload;
      if (!isSessionDeletedForUndoPayload(payload)) {
        return;
      }

      invalidateResource("sessions", payload.sessionId);
      addDeletion(payload.data);
      void closeSessionNoteWindows(payload.sessionId);
    }).then((fn) => {
      unlisten = fn;
    });

    return () => {
      unlisten?.();
    };
  }, [active, invalidateResource, addDeletion]);
}
