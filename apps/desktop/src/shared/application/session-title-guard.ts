import { onlineManager } from "@tanstack/react-query";

import { documentEditorDrafts } from "./document-editor-draft";
import { NativeDraftStore } from "./draft-store";
import { ApplicationRequestError } from "./errors";
import { noteDrafts } from "./note-draft";
import { noteEditorDrafts } from "./note-editor-draft";
import {
  draftSessionId,
  draftWriteQueueKey,
  sessionEditorFlushKey,
} from "./session-cleanup-guard";
import type { VaultScope } from "./vault-scope";

import { hasUnsavedSessionNoteDrafts } from "~/session/pending-note-drafts";
import { flushDatabaseWrites } from "~/shared/write-queue";

export function captureSessionTitleGuard(
  scope: VaultScope,
  sessionId: string,
  current: () => boolean,
) {
  const client = scope.client;
  const entries = () => [
    ...[...noteDrafts.values(client.scope)].filter(
      (draft) => draftSessionId(draft.sessionId) === sessionId,
    ),
    ...[...noteEditorDrafts.values(client.scope)].filter(
      (draft) => draftSessionId(draft.sessionId) === sessionId,
    ),
    ...documentEditorDrafts.forSession(client.scope, sessionId),
  ];
  const captured = entries();
  let invalidated = false;
  let prepared = false;
  const refused = () =>
    new ApplicationRequestError({
      code: "conflict",
      message:
        "Confirm or recover your saved edits before changing the title. Your draft is kept on this device.",
      request_id: null,
      retryable: false,
      current_revision: null,
      mutation_outcome: "not_committed",
    });
  const check = () => {
    if (
      invalidated ||
      !current() ||
      scope.client !== client ||
      scope.getSnapshot().state !== "connected" ||
      !client.canEdit ||
      !client.features.includes("metadata_edit") ||
      (client.networkMode === "online" && !onlineManager.isOnline())
    ) {
      invalidated = true;
      throw refused();
    }
  };
  const clean = () => {
    check();
    const latest = entries();
    if (latest.some((draft) => !captured.includes(draft))) throw refused();
    for (const draft of captured) {
      const value = draft.getSnapshot();
      if (value.dirty || value.conflict || draft.recovery().uncertain)
        throw refused();
    }
    if (hasUnsavedSessionNoteDrafts(client.scope, sessionId)) throw refused();
  };
  const stops = [
    scope.subscribe(() => {
      try {
        check();
      } catch {}
    }),
    onlineManager.subscribe(() => {
      try {
        check();
      } catch {}
    }),
    ...captured.map((draft) =>
      draft.subscribe(() => {
        if (prepared && draft.getSnapshot().dirty) invalidated = true;
      }),
    ),
  ];
  return {
    async prepare() {
      check();
      let timer: ReturnType<typeof setTimeout> | undefined;
      const operation = (async () => {
        await flushDatabaseWrites([
          sessionEditorFlushKey(client.scope, sessionId),
          draftWriteQueueKey(client, sessionId),
          ...captured.map((draft) =>
            draftWriteQueueKey(client, draft.sessionId),
          ),
        ]);
        clean();
        const inventory = await new NativeDraftStore(client.scope).inventory();
        clean();
        if (
          inventory.some(
            (entry) => draftSessionId(entry.sessionId) === sessionId,
          )
        )
          throw refused();
        prepared = true;
      })();
      try {
        await Promise.race([
          operation,
          new Promise<never>((_resolve, reject) => {
            timer = setTimeout(() => {
              invalidated = true;
              reject(refused());
            }, 3000);
          }),
        ]);
      } finally {
        clearTimeout(timer);
      }
    },
    assertReady() {
      if (!prepared) throw refused();
      clean();
    },
    dispose() {
      invalidated = true;
      for (const stop of stops) stop();
    },
  };
}
