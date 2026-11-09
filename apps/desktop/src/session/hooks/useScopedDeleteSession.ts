import { useMutation } from "@tanstack/react-query";
import { emitTo } from "@tauri-apps/api/event";
import { useCallback, useMemo } from "react";

import { getCurrentWebviewWindowLabel } from "@hypr/plugin-windows";
import { sonnerToast } from "@hypr/ui/components/ui/toast";

import {
  closeSessionNoteWindows,
  SESSION_DELETED_FOR_UNDO_EVENT,
  type SessionDeletedForUndoPayload,
} from "./useLegacyDeleteSession";

import { trackPendingSoftDelete } from "~/session/pending-soft-deletes";
import { ApplicationRequestError } from "~/shared/application/errors";
import {
  captureSessionDeletion,
  createSessionLifecycleQueue,
  sessionDeletionAvailable,
  SessionLifecycleReviewError,
  type SessionDeletionIntent,
} from "~/shared/application/session-lifecycle";
import type { VaultScope } from "~/shared/application/vault-scope";
import { useTabsStore } from "~/store/zustand/tabs";
import {
  deletionKey,
  useUndoDelete,
  type DeletedSessionData,
} from "~/store/zustand/undo-delete";

export function invalidateSessionLifecycle(
  scope: VaultScope,
  sessionId: string,
) {
  for (const kind of [
    "session_page",
    "session_summaries",
    "timeline_headers",
    "timeline_facets",
    "search",
    "tag_suggestions",
  ])
    void scope.queryClient.invalidateQueries({
      queryKey: ["vault", scope.client.scope, kind],
    });
  for (const kind of [
    "session",
    "metadata",
    "note",
    "note_editor",
    "documents",
    "document",
    "document_text",
    "document_editor",
    "tasks",
    "tasks_snapshot",
    "transcripts",
    "transcript_overview",
    "attachments",
    "audio_resource",
    "audio_metadata",
    "audio_peaks",
  ])
    void scope.queryClient.invalidateQueries({
      queryKey: ["vault", scope.client.scope, kind, sessionId],
    });
}

export function useScopedDeleteSession(scope: VaultScope) {
  const store = useTabsStore();
  const key = useMemo(
    () => ["vault", scope.client.scope, "session_deletion"] as const,
    [scope],
  );
  const cache = scope.queryClient.getMutationCache();
  const mutation = useMutation(
    {
      mutationKey: key,
      networkMode: "always",
      retry: false,
      gcTime: Infinity,
      mutationFn: (intent: SessionDeletionIntent) => intent.submit(),
    },
    scope.queryClient,
  );
  const mutate = mutation.mutateAsync;
  return useCallback(
    (sessionId: string, options?: { batchId?: string; title?: string }) => {
      if (!sessionDeletionAvailable(scope, store)) return;
      const attempts = () =>
        cache.findAll({ mutationKey: key, exact: true }).filter((entry) => {
          const intent = entry.state.variables as
            | SessionDeletionIntent
            | undefined;
          return intent?.scope === scope && intent.store === store;
        });
      const remove = (intent: SessionDeletionIntent) => {
        for (const entry of attempts())
          if (
            entry.state.variables === intent &&
            entry.state.status !== "pending"
          )
            cache.remove(entry);
        intent.release();
      };
      const currentEntry = () =>
        useUndoDelete.getState().pendingDeletions[
          deletionKey(scope.client.scope, sessionId)
        ];
      if (currentEntry()?.owner === scope) return;
      const existing = attempts().find(
        (entry) =>
          (entry.state.variables as SessionDeletionIntent).sessionId ===
          sessionId,
      );
      if (existing?.state.status === "pending") return;
      const previous = existing?.state.variables as
        | SessionDeletionIntent
        | undefined;
      const queue =
        previous?.queue ??
        (attempts()[0]?.state.variables as SessionDeletionIntent | undefined)
          ?.queue ??
        createSessionLifecycleQueue();
      const intent =
        previous ?? captureSessionDeletion(scope, store, sessionId, queue);
      const toastId = `session-delete:${intent.requestId}`;
      intent.onRelease(() => sonnerToast.dismiss(toastId));
      const title = options?.title ?? "";
      const scopeId = scope.client.scope;
      const device = scope.client.features.includes(
        "legacy_desktop_session_controls",
      );
      const mainWindow = !device || getCurrentWebviewWindowLabel() === "main";
      const owned = () =>
        intent.isCurrent() && sessionDeletionAvailable(scope, store);
      const reviewUndo = () => {
        if (!intent.isCurrent()) return;
        if (intent.uncertain()) {
          review();
          return;
        }
        sonnerToast.error(
          "Could not confirm undo. Retry the original restore request.",
          {
            id: toastId,
            duration: Infinity,
            action: {
              label: "Retry undo",
              onClick: (event) => {
                event.preventDefault();
                if (owned()) undo();
              },
            },
          },
        );
      };
      const undo = () => {
        if (!owned()) return;
        void intent.restore().then(
          () => {
            if (intent.isCurrent()) {
              invalidateSessionLifecycle(scope, sessionId);
              intent.rollback();
              sonnerToast.dismiss(toastId);
            }
            remove(intent);
          },
          (error) => {
            if (!intent.isCurrent()) return;
            if (error instanceof SessionLifecycleReviewError) {
              sonnerToast.error(error.message, { id: toastId });
              remove(intent);
              return;
            }
            reviewUndo();
          },
        );
      };
      const review = () => {
        if (!owned()) return;
        sonnerToast.error(
          "The deletion may have completed. Retry the original request before deleting again.",
          {
            id: toastId,
            duration: Infinity,
            action: {
              label: "Retry",
              onClick: (event) => {
                event.preventDefault();
                if (
                  owned() &&
                  !attempts().some(
                    (entry) =>
                      entry.state.status === "pending" &&
                      entry.state.variables === intent,
                  )
                )
                  run();
              },
            },
          },
        );
      };
      const run = () => {
        if (!owned()) return;
        for (const entry of attempts())
          if (
            entry.state.variables === intent &&
            entry.state.status === "error"
          )
            cache.remove(entry);
        intent.hide();
        sonnerToast.loading("Deleting note…", {
          id: toastId,
          action: {
            label: "Cancel",
            onClick: (event) => {
              event.preventDefault();
              undo();
            },
          },
        });
        const work = mutate(intent);
        trackPendingSoftDelete(scopeId, sessionId, work);
        void work
          .then(
            async (outcome) => {
              sonnerToast.dismiss(toastId);
              if (!intent.isCurrent()) {
                remove(intent);
                return;
              }
              invalidateSessionLifecycle(scope, sessionId);
              if (outcome.kind !== "trashed") {
                if (!intent.undoRequested()) intent.rollback();
                if (outcome.kind === "conflict")
                  sonnerToast.error(
                    "The note changed. Review its current state before deleting it.",
                  );
                remove(intent);
                return;
              }
              if (intent.undoRequested()) {
                undo();
                return;
              }
              let absent = false;
              try {
                absent = await intent.confirmDeleted();
              } catch {
                /* The receipt remains undoable while canonical presence is unknown. */
              }
              if (!intent.isCurrent()) {
                remove(intent);
                return;
              }
              if (!absent) intent.rollback();
              const data: DeletedSessionData = {
                session: { id: sessionId, title },
                tombstone: intent.requestId,
                deletedAt: Date.now(),
                receipt: { requestId: intent.requestId, outcome },
                confirmedAbsent: absent,
              };
              if (mainWindow) {
                useUndoDelete.getState().addDeletion(scopeId, data, {
                  owner: scope,
                  batchId: options?.batchId,
                  onConfirm: () => {
                    if (intent.unresolved()) reviewUndo();
                    else remove(intent);
                  },
                  restore: async (assertCurrent) => {
                    assertCurrent();
                    await intent.restore();
                    assertCurrent();
                    invalidateSessionLifecycle(scope, sessionId);
                    remove(intent);
                  },
                });
              } else {
                await emitTo("main", SESSION_DELETED_FOR_UNDO_EVENT, {
                  scope: scopeId,
                  sessionId,
                  data,
                } satisfies SessionDeletedForUndoPayload);
              }
              if (absent && device && intent.canCloseWindows())
                await closeSessionNoteWindows(
                  sessionId,
                  () => intent.canCloseWindows() && owned(),
                );
              if (!mainWindow) remove(intent);
            },
            (error) => {
              sonnerToast.dismiss(toastId);
              if (intent.uncertain()) {
                review();
                return;
              }
              if (intent.isCurrent()) {
                invalidateSessionLifecycle(scope, sessionId);
                if (
                  !(
                    error instanceof ApplicationRequestError &&
                    error.detail.code === "not_found"
                  )
                ) {
                  intent.rollback();
                  sonnerToast.error(
                    error instanceof SessionLifecycleReviewError
                      ? error.message
                      : "Could not delete this note. Please try again.",
                  );
                }
              }
              remove(intent);
            },
          )
          .catch((error) => {
            if (owned())
              sonnerToast.error(
                "Could not finish deletion feedback. The original request is retained for review.",
              );
            console.error("[delete-session] feedback failed", error);
          });
      };
      if (existing?.state.status === "success") {
        if (intent.undoRequested()) reviewUndo();
        return;
      }
      if (previous?.uncertain()) review();
      else run();
    },
    [scope, store, cache, mutate, key],
  );
}
