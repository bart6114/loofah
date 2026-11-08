import { onlineManager } from "@tanstack/react-query";

import { ApplicationRequestError } from "./errors";
import type { VaultScope } from "./vault-scope";

import { uniqueIdfromTab, type ScopedTabsStore } from "~/store/zustand/tabs";

const terminal = new Set([
  "access_denied",
  "login_required",
  "closed",
  "incompatible",
]);

export function noteCreationAvailable(
  scope: VaultScope,
  store: ScopedTabsStore,
) {
  return (
    store.scope === scope.client.scope &&
    scope.getSnapshot().state === "connected" &&
    scope.client.canEdit &&
    scope.client.features.includes("notes") &&
    (scope.client.networkMode === "always" || onlineManager.isOnline())
  );
}

export function captureNoteCreation(
  scope: VaultScope,
  store: ScopedTabsStore,
  behavior: "new" | "current",
) {
  const current = store.getState().currentTab;
  const destination = current
    ? { slotId: current.slotId, id: uniqueIdfromTab(current) }
    : null;
  const requestId = crypto.randomUUID();
  let replaced = false;
  let retired = terminal.has(scope.getSnapshot().state);
  let sent = false;
  let unresolved = false;
  const stopStore = store.subscribe((state) => {
    if (
      destination &&
      !state.tabs.some(
        (tab) =>
          tab.slotId === destination.slotId &&
          uniqueIdfromTab(tab) === destination.id,
      )
    )
      replaced = true;
  });
  const stopScope = scope.subscribe(() => {
    if (terminal.has(scope.getSnapshot().state)) {
      retired = true;
      release();
    }
  });
  function release() {
    stopStore();
    stopScope();
  }
  const isCurrent = () => !retired && !terminal.has(scope.getSnapshot().state);
  return {
    scope,
    store,
    requestId,
    release,
    isCurrent,
    uncertain: (error: unknown) => {
      if (
        sent &&
        !(
          error instanceof ApplicationRequestError &&
          error.detail.mutation_outcome === "not_committed"
        )
      )
        unresolved = true;
      return unresolved;
    },
    async submit() {
      if (retired || !noteCreationAvailable(scope, store))
        throw new ApplicationRequestError({
          code: "unavailable",
          message: "Note creation is unavailable",
          request_id: null,
          retryable: false,
          current_revision: null,
          mutation_outcome: "not_committed",
        });
      sent = true;
      const session = await scope.run(() =>
        scope.client.createSession({ title: "", markdown: "" }, { requestId }),
      );
      return session.meta.id;
    },
    open(sessionId: string) {
      if (!isCurrent()) return;
      const state = store.getState();
      const active = state.currentTab;
      const sameDestination = destination
        ? active?.slotId === destination.slotId &&
          uniqueIdfromTab(active) === destination.id
        : active === null;
      const tab = { type: "sessions" as const, id: sessionId };
      if (behavior === "current" && !replaced && sameDestination)
        state.openCurrent(tab);
      else state.openNew(tab);
    },
  };
}

export type NoteCreationIntent = ReturnType<typeof captureNoteCreation>;
