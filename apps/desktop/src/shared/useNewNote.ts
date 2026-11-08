import { useLingui } from "@lingui/react/macro";
import { useMutation } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef } from "react";
import { useShallow } from "zustand/shallow";

import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { useLegacySessionActivity } from "~/main/use-legacy-activity";
import { createSession } from "~/session/queries";
import {
  captureNoteCreation,
  noteCreationAvailable,
  type NoteCreationIntent,
} from "~/shared/application/note-creation";
import { useVaultHost } from "~/shared/application/vault-provider";
import { useTabs, useTabsStore } from "~/store/zustand/tabs";

export function useNewNote({
  behavior = "new",
}: {
  behavior?: "new" | "current";
} = {}) {
  const { t } = useLingui();
  const { scope } = useVaultHost();
  const store = useTabsStore();
  if (!scope) throw new Error("Note creation requires an immutable vault host");
  const owner = useMemo(
    () => ({ scope, store, mounted: false, toasts: new Set<string>() }),
    [scope, store],
  );
  const currentOwner = useRef(owner);
  currentOwner.current = owner;
  const toasts = owner.toasts;
  useEffect(() => {
    owner.mounted = true;
    const stop = scope.subscribe(() => {
      if (
        ["access_denied", "login_required", "closed", "incompatible"].includes(
          scope.getSnapshot().state,
        )
      ) {
        for (const id of toasts) sonnerToast.dismiss(id);
        toasts.clear();
      }
    });
    return () => {
      owner.mounted = false;
      stop();
      for (const id of toasts) sonnerToast.dismiss(id);
      toasts.clear();
    };
  }, [owner, scope, toasts]);
  const mutation = useMutation(
    {
      mutationKey: ["vault", scope.client.scope, "note_creation"],
      networkMode: "always",
      retry: false,
      gcTime: Infinity,
      mutationFn: (intent: NoteCreationIntent) => intent.submit(),
    },
    scope.queryClient,
  );
  const mutate = mutation.mutateAsync;
  return useMemo(() => {
    const active = () => owner.mounted && currentOwner.current === owner;
    const key = ["vault", scope.client.scope, "note_creation"] as const;
    const cache = scope.queryClient.getMutationCache();
    const attempts = () =>
      cache
        .findAll({ mutationKey: key, exact: true })
        .filter(
          (entry) =>
            (entry.state.variables as NoteCreationIntent | undefined)?.store ===
            store,
        );
    const remove = (intent: NoteCreationIntent) => {
      for (const entry of attempts())
        if (
          entry.state.variables === intent &&
          entry.state.status !== "pending"
        )
          cache.remove(entry);
      intent.release();
    };
    const run = (intent: NoteCreationIntent) => {
      const id = `new-note:${intent.requestId}`;
      toasts.add(id);
      sonnerToast.loading(t`Creating note...`, { id });
      void mutate(intent).then(
        (sessionId) => {
          if (intent.isCurrent()) {
            for (const kind of [
              "session_page",
              "session_summaries",
              "search",
              "tag_suggestions",
            ])
              void scope.queryClient.invalidateQueries({
                queryKey: ["vault", scope.client.scope, kind],
              });
            if (active()) intent.open(sessionId);
          }
          sonnerToast.dismiss(id);
          toasts.delete(id);
          remove(intent);
        },
        (error: unknown) => {
          sonnerToast.dismiss(id);
          toasts.delete(id);
          if (intent.uncertain(error)) {
            if (active() && intent.isCurrent()) review(intent);
          } else {
            remove(intent);
            if (active() && intent.isCurrent())
              sonnerToast.error(t`Could not create note.`);
          }
        },
      );
    };
    const review = (intent: NoteCreationIntent) => {
      const id = `new-note:${intent.requestId}`;
      toasts.add(id);
      sonnerToast.error(
        t`The note may already exist. Retry the same request before creating another.`,
        {
          id,
          duration: Infinity,
          action: {
            label: t`Retry`,
            onClick: (event) => {
              event.preventDefault();
              if (
                !active() ||
                !intent.isCurrent() ||
                !noteCreationAvailable(scope, store)
              )
                return;
              const previous = attempts().find(
                (entry) =>
                  entry.state.variables === intent &&
                  entry.state.status === "error",
              );
              if (
                !previous ||
                attempts().some((entry) => entry.state.status === "pending")
              )
                return;
              cache.remove(previous);
              run(intent);
            },
          },
        },
      );
    };
    return () => {
      if (!active() || !noteCreationAvailable(scope, store)) {
        if (active()) sonnerToast.error(t`Could not create note.`);
        return;
      }
      for (const entry of attempts()) {
        if (entry.state.status === "pending") return;
        const intent = entry.state.variables as NoteCreationIntent;
        if (
          entry.state.status === "error" &&
          intent.uncertain(entry.state.error)
        ) {
          review(intent);
          return;
        }
      }
      run(captureNoteCreation(scope, store, behavior));
    };
  }, [owner, scope, store, toasts, mutate, t, behavior]);
}

export function useNewNoteAndListen({
  behavior = "new",
}: {
  behavior?: "new" | "current";
} = {}) {
  const { openNew, openCurrent } = useTabs(
    useShallow((state) => ({
      openNew: state.openNew,
      openCurrent: state.openCurrent,
    })),
  );
  const legacy = useLegacySessionActivity("");
  const { isAvailable } = legacy;
  const store = useTabsStore();
  const status = legacy.liveStatus,
    liveSessionId = legacy.liveSessionId;

  const handler = useCallback(() => {
    if (!isAvailable()) {
      sonnerToast.error("Recording is not available for this connection.");
      return;
    }
    if (status === "active" && liveSessionId) {
      const ff = behavior === "new" ? openNew : openCurrent;
      ff({ type: "sessions", id: liveSessionId });
      return;
    }

    const ff = behavior === "new" ? openNew : openCurrent;
    const slot = store.getState().currentTab?.slotId;
    void createSession()
      .then((sessionId) => {
        if (
          !isAvailable() ||
          (behavior === "current" &&
            store.getState().currentTab?.slotId !== slot)
        )
          return;
        ff({
          type: "sessions",
          id: sessionId,
          state: { view: null, autoStart: true },
        });
      })
      .catch((error) => {
        console.error("[session] failed to create listening note", error);
      });
  }, [
    status,
    liveSessionId,
    openNew,
    openCurrent,
    behavior,
    isAvailable,
    store,
  ]);

  return handler;
}
