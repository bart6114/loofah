import { type CSSProperties, useCallback, useMemo } from "react";

import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { useSessionDeletionAccess } from "~/session/hooks/useDeleteSession";
import { useMountEffect } from "~/shared/hooks/useMountEffect";
import { useTabs } from "~/store/zustand/tabs";
import {
  UNDO_TIMEOUT_MS,
  useUndoDelete,
  useScopedPendingDeletions,
} from "~/store/zustand/undo-delete";

type ToastGroup = {
  scope: string;
  key: string;
  sessionIds: string[];
  tokens: string[];
  isBatch: boolean;
  addedAt: number;
};

function useToastGroups(scope: string, owner: object): ToastGroup[] {
  const pendingDeletions = useScopedPendingDeletions(scope, owner);

  return useMemo(() => {
    const batchMap = new Map<string, string[]>();
    const singles: { sessionId: string; addedAt: number }[] = [];

    for (const [sessionId, pending] of Object.entries(pendingDeletions)) {
      if (pending.batchId) {
        const existing = batchMap.get(pending.batchId) ?? [];
        existing.push(sessionId);
        batchMap.set(pending.batchId, existing);
      } else {
        singles.push({ sessionId, addedAt: pending.addedAt });
      }
    }

    const groups: ToastGroup[] = singles.map(({ sessionId, addedAt }) => ({
      scope,
      key: JSON.stringify([scope, "session", sessionId]),
      sessionIds: [sessionId],
      tokens: [pendingDeletions[sessionId].token],
      isBatch: false,
      addedAt,
    }));

    for (const [batchId, sessionIds] of batchMap) {
      groups.push({
        scope,
        key: JSON.stringify([scope, "batch", batchId]),
        sessionIds,
        tokens: sessionIds.map((id) => pendingDeletions[id].token),
        isBatch: true,
        addedAt: Math.min(
          ...sessionIds.map((id) => pendingDeletions[id].addedAt),
        ),
      });
    }

    return groups.sort((a, b) => a.addedAt - b.addedAt);
  }, [pendingDeletions, scope]);
}

function useRestoreGroup() {
  const access = useSessionDeletionAccess();
  const { isAvailable } = access;
  const scope = access.scope.client.scope;
  const queryClient = access.scope.queryClient;
  const pendingDeletions = useScopedPendingDeletions(scope, access.scope);
  const addDeletion = useUndoDelete((state) => state.addDeletion);
  const clearDeletion = useUndoDelete((state) => state.clearDeletion);
  const openCurrent = useTabs((state) => state.openCurrent);
  const invalidateResource = useTabs((state) => state.invalidateResource);

  return useCallback(
    (group: ToastGroup) => {
      if (group.scope !== scope || !isAvailable()) return;
      const assertCurrent = () => {
        if (!isAvailable() || group.scope !== scope)
          throw new Error("Session controls are unavailable.");
      };
      const currentEntries = useUndoDelete.getState().pendingDeletions;
      const entries = group.sessionIds.map((id) => pendingDeletions[id]);
      if (
        entries.some(
          (entry, index) =>
            !entry ||
            entry.token !== group.tokens[index] ||
            currentEntries[JSON.stringify([scope, entry.data.session.id])] !==
              entry,
        )
      )
        return;

      const superseded = new Set<string>();
      const stop = useUndoDelete.subscribe((next) => {
        for (const entry of entries) {
          const key = JSON.stringify([scope, entry.data.session.id]);
          const replacement = next.pendingDeletions[key];
          if (replacement && replacement !== entry) superseded.add(key);
        }
      });

      // Optimistic: dismiss the toast (also cancelling the finalize timers)
      // and reopen the tab before the restore writes commit.
      for (const entry of entries) clearDeletion(scope, entry.data.session.id);
      if (group.sessionIds.length > 0) {
        openCurrent({ type: "sessions", id: group.sessionIds[0] });
      }

      void (async () => {
        const remaining = [...entries];
        try {
          while (remaining.length > 0) {
            const pending = remaining[0];
            const key = JSON.stringify([scope, pending.data.session.id]);
            const assertOwned = () => {
              assertCurrent();
              if (superseded.has(key))
                throw new Error(
                  "This undo was superseded by a newer deletion.",
                );
            };
            assertOwned();
            await pending.restore(assertOwned);
            remaining.shift();
            const sessionId = pending.data.session.id;
            if (!isAvailable() || superseded.has(key)) continue;
            for (const kind of [
              "audio_resource",
              "audio_peaks",
              "audio_metadata",
            ])
              void queryClient.invalidateQueries({
                queryKey: ["vault", scope, kind, sessionId],
              });
          }
        } catch (error) {
          console.error("[undo-delete] failed to restore session", error);
          if (
            isAvailable() &&
            remaining.some(
              (entry) =>
                !superseded.has(JSON.stringify([scope, entry.data.session.id])),
            )
          )
            sonnerToast.error("Could not restore deleted note");
          // Re-add the unrestored deletions so their undo toast (and the
          // finalize path) comes back instead of leaving them tombstoned,
          // and close the optimistically reopened tab — it still points at
          // a tombstoned note.
          for (const pending of remaining) {
            const existing =
              useUndoDelete.getState().pendingDeletions[
                JSON.stringify([scope, pending.data.session.id])
              ];
            if (
              !existing &&
              !superseded.has(JSON.stringify([scope, pending.data.session.id]))
            )
              addDeletion(scope, pending.data, {
                owner: pending.owner,
                restore: pending.restore,
                onConfirm: pending.onDeleteConfirm ?? undefined,
                batchId: pending.batchId ?? undefined,
              });
            if (
              isAvailable() &&
              !pending.data.receipt &&
              !superseded.has(JSON.stringify([scope, pending.data.session.id]))
            )
              invalidateResource("sessions", pending.data.session.id);
          }
        } finally {
          stop();
        }
      })();
    },
    [
      pendingDeletions,
      openCurrent,
      invalidateResource,
      addDeletion,
      clearDeletion,
      queryClient,
      isAvailable,
      scope,
    ],
  );
}

export function UndoDeleteToast() {
  const access = useSessionDeletionAccess();
  const { isAvailable } = access;
  if (!isAvailable()) return null;
  return <ScopedUndoDeleteToast />;
}

function ScopedUndoDeleteToast() {
  const access = useSessionDeletionAccess();
  const groups = useToastGroups(access.scope.client.scope, access.scope);

  return groups.map((group) => (
    <UndoDeleteSonnerToast
      key={JSON.stringify([group.key, group.tokens])}
      group={group}
    />
  ));
}

function UndoDeleteSonnerToast({ group }: { group: ToastGroup }) {
  const restoreGroup = useRestoreGroup();
  const access = useSessionDeletionAccess();
  const pendingDeletions = useScopedPendingDeletions(group.scope, access.scope);
  const title = group.isBatch
    ? null
    : pendingDeletions[group.sessionIds[0]]?.data.session.title || "Untitled";
  const noteLabel = group.sessionIds.length === 1 ? "note" : "notes";
  const confirmed = group.sessionIds.every(
    (id) => pendingDeletions[id]?.data.confirmedAbsent !== false,
  );
  const label = !confirmed
    ? "Deletion request completed"
    : group.isBatch
      ? `${group.sessionIds.length} ${noteLabel} deleted`
      : `${title} deleted`;
  const remainingDuration = Math.max(
    0,
    group.addedAt + UNDO_TIMEOUT_MS - Date.now(),
  );
  const progress = remainingDuration / UNDO_TIMEOUT_MS;

  useMountEffect(() => {
    const toastId = `undo-delete:${group.key}`;

    sonnerToast.message(label, {
      id: toastId,
      duration: Infinity,
      description: (
        <span
          aria-hidden="true"
          className="undo-delete-toast-gauge bg-primary block h-full w-full"
          style={
            {
              "--undo-delete-duration": `${remainingDuration}ms`,
              "--undo-delete-progress": progress,
            } as CSSProperties
          }
        />
      ),
      descriptionClassName:
        "bg-muted absolute inset-x-0 bottom-0 h-1 overflow-hidden",
      action: {
        label: "Undo",
        onClick: () => restoreGroup(group),
      },
    });

    return () => sonnerToast.dismiss(toastId);
  });

  return null;
}
