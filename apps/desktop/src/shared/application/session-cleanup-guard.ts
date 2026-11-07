import { onlineManager } from "@tanstack/react-query";

import { NativeDraftStore } from "./draft-store";
import { noteEditorDraftId, noteEditorDrafts } from "./note-editor-draft";
import type { VaultClient } from "./vault-client";
import type { VaultScope } from "./vault-scope";

import { hasUnsavedSessionNoteDrafts } from "~/session/pending-note-drafts";
import { flushDatabaseWrites } from "~/shared/write-queue";

const CLEANUP_PREPARATION_TIMEOUT_MS = 3000;

export function draftWriteQueueKey(
  client: Pick<VaultClient, "scope" | "noteWriteQueueKey">,
  id: string,
) {
  return (
    client.noteWriteQueueKey?.(id) ??
    `vault-write:${JSON.stringify([client.scope, id])}`
  );
}

export function sessionEditorFlushKey(scope: string, sessionId: string) {
  return `session-editor-flush:${JSON.stringify([scope, sessionId])}`;
}

export function draftSessionId(id: string): string {
  try {
    const resource: unknown = JSON.parse(id);
    if (
      Array.isArray(resource) &&
      ["summary", "template_output", "note_editor"].includes(resource[0]) &&
      typeof resource[1] === "string"
    )
      return resource[1];
  } catch {}
  return id;
}

export function captureSessionCleanupGuard(
  scope: VaultScope,
  sessionId: string,
  { isOpen }: { isOpen: () => boolean },
) {
  const client = scope.client;
  const key = noteEditorDraftId(sessionId);
  const draft = noteEditorDrafts.get(client.scope, key);
  const store = new NativeDraftStore(client.scope);
  let disposed = false;
  let prepared = false;
  let preparing: Promise<boolean> | null = null;
  let cancelPreparation: (() => void) | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let stopScope = () => {};
  let stopOnline = () => {};

  const dispose = () => {
    if (disposed) return;
    disposed = true;
    stopScope();
    stopOnline();
    clearTimeout(timer);
    cancelPreparation?.();
  };
  const current = () => {
    if (disposed) return false;
    if (
      scope.client !== client ||
      scope.getSnapshot().state !== "connected" ||
      !client.canEdit ||
      (client.networkMode === "online" && !onlineManager.isOnline()) ||
      isOpen()
    ) {
      dispose();
      return false;
    }
    return true;
  };
  const clean = () => {
    const latest = noteEditorDrafts.get(client.scope, key);
    if (latest && latest !== draft) return false;
    if (draft) {
      const snapshot = draft.getSnapshot();
      if (snapshot.dirty || snapshot.conflict || draft.recovery().uncertain)
        return false;
    }
    return !hasUnsavedSessionNoteDrafts(client.scope, sessionId);
  };
  const canDelete = () => {
    if (!prepared || !current()) return false;
    try {
      if (clean()) return true;
    } catch {}
    dispose();
    return false;
  };
  stopScope = scope.subscribe(() => {
    current();
  });
  stopOnline = onlineManager.subscribe(() => {
    current();
  });

  const prepare = () => {
    if (preparing) return preparing;
    if (!current()) return Promise.resolve(false);
    const abandoned = new Promise<boolean>((resolve) => {
      cancelPreparation = () => resolve(false);
    });
    timer = setTimeout(dispose, CLEANUP_PREPARATION_TIMEOUT_MS);
    const operation = (async () => {
      // Capture mounted flushers before closing React effects can unregister them.
      await flushDatabaseWrites([
        sessionEditorFlushKey(client.scope, sessionId),
        draftWriteQueueKey(client, sessionId),
      ]);
      if (!current() || !clean()) return false;
      const recovery = await store.inventory();
      if (
        !current() ||
        recovery.some((entry) => draftSessionId(entry.sessionId) === sessionId)
      )
        return false;
      prepared = true;
      return canDelete();
    })().catch(() => false);
    preparing = Promise.race([operation, abandoned]).then((ready) => {
      clearTimeout(timer);
      cancelPreparation = null;
      if (!ready) dispose();
      return ready;
    });
    return preparing;
  };
  return { prepare, canDelete, dispose };
}
