import { useMutation } from "@tanstack/react-query";
import { useEffect, useRef, useSyncExternalStore } from "react";

import type { NativeDraftStore } from "./draft-store";
import { ApplicationRequestError } from "./errors";
import type { RevisionDraft } from "./revision-draft";
import type { VaultScope } from "./vault-scope";

import type { NoteDraftRecovery } from "~/types/tauri.gen";

export function useDraftPersistence<S, D>({
  scope,
  draft,
  queryKey,
  confirm,
  available,
  recovery,
  store,
  recoveryId,
  onSettled,
  autosave = true,
}: {
  scope: VaultScope;
  draft: RevisionDraft<S, D> & { recovery: () => NoteDraftRecovery };
  queryKey: readonly unknown[];
  confirm: (snapshot: S) => Promise<void>;
  available: boolean;
  recovery: NoteDraftRecovery | null;
  store: NativeDraftStore;
  recoveryId: string;
  onSettled: () => void;
  autosave?: boolean;
}) {
  const snapshot = useSyncExternalStore(draft.subscribe, draft.getSnapshot);
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  const persist = useMutation({
    networkMode: "always",
    mutationFn: async (beforeRequest: boolean) => {
      const value = draft.recovery();
      if (beforeRequest) value.uncertain = true;
      if (draft.getSnapshot().dirty || value.uncertain) await store.save(value);
      else {
        await store.remove(recoveryId);
        if (
          ![
            "closed",
            "access_denied",
            "login_required",
            "incompatible",
          ].includes(scope.getSnapshot().state)
        )
          scope.queryClient.setQueryData(
            ["device-draft", draft.scope, recoveryId],
            null,
          );
      }
    },
  });
  const lastSubmitted = useRef<D | null>(null);
  const save = useMutation({
    networkMode: "always",
    mutationFn: async () => {
      await persist.mutateAsync(true).catch(() => {});
      try {
        if (scope.getSnapshot().state !== "connected")
          throw new ApplicationRequestError({
            code: "unavailable",
            message:
              "Reconnect before saving. Your draft is kept on this device.",
            request_id: null,
            retryable: true,
            current_revision: null,
            mutation_outcome: "not_committed",
          });
        if (!available || !scope.client.canEdit)
          throw new ApplicationRequestError({
            code: "forbidden",
            message: "This content is not available for editing.",
            request_id: null,
            retryable: false,
            current_revision: null,
            mutation_outcome: "not_committed",
          });
        lastSubmitted.current = draft.getSnapshot().draft;
        return await scope.saveAndConfirm(
          queryKey,
          () => draft.save(),
          confirm,
        );
      } catch (error) {
        if (
          error instanceof ApplicationRequestError &&
          error.detail.code === "not_found"
        )
          await scope.queryClient.invalidateQueries({ queryKey, exact: true });
        throw error;
      } finally {
        await persist.mutateAsync(false).catch(() => {});
        onSettled();
      }
    },
  });
  const editable =
    available &&
    connection.state === "connected" &&
    scope.client.canEdit &&
    (!snapshot.conflict || save.isPending);
  const persistRef = useRef(persist.mutateAsync);
  persistRef.current = persist.mutateAsync;
  const saveRef = useRef(save.mutate);
  saveRef.current = save.mutate;
  const saveOnLeave = useRef(false);
  saveOnLeave.current =
    available &&
    scope.client.canEdit &&
    !(save.error && lastSubmitted.current === snapshot.draft);
  useEffect(() => {
    if (!snapshot.dirty && !recovery) return;
    const timer = setTimeout(() => {
      void persistRef.current(false).catch(() => {});
    }, 200);
    return () => clearTimeout(timer);
  }, [snapshot, recovery]);
  useEffect(() => {
    if (
      !autosave ||
      !snapshot.dirty ||
      !editable ||
      save.isPending ||
      (save.error && lastSubmitted.current === snapshot.draft)
    )
      return;
    const timer = setTimeout(() => saveRef.current(), 500);
    return () => clearTimeout(timer);
  }, [autosave, snapshot, editable, save.isPending, save.error]);
  useEffect(() => {
    if (connection.state === "connected") lastSubmitted.current = null;
  }, [connection.state]);
  useEffect(
    () => () => {
      void persistRef.current(false).catch(() => {});
      const current = draft.getSnapshot();
      if (
        saveOnLeave.current &&
        current.dirty &&
        !current.conflict &&
        scope.getSnapshot().state === "connected"
      )
        saveRef.current();
    },
    [draft, scope],
  );
  const resolve = useMutation({
    networkMode: "always",
    mutationFn: async ({
      keep,
      revision,
    }: {
      keep: boolean;
      revision: string;
    }) => {
      if (keep) draft.keepDraft(revision);
      else draft.useServerVersion(revision);
      if (keep) await save.mutateAsync();
      else {
        await confirm(draft.getSnapshot().baseline);
        await persist.mutateAsync(false);
      }
    },
  });
  return { snapshot, connection, editable, persist, save, resolve };
}
