import { onlineManager } from "@tanstack/react-query";

import { documentDraftId } from "./document-draft";
import {
  documentEditorDraftId,
  documentEditorDrafts,
} from "./document-editor-draft";
import { NativeDraftStore } from "./draft-store";
import { ApplicationRequestError } from "./errors";
import { noteDrafts } from "./note-draft";
import { noteEditorDraftId, noteEditorDrafts } from "./note-editor-draft";
import type { DocumentTarget } from "./protocol.gen";
import {
  draftSessionId,
  draftWriteQueueKey,
  sessionEditorFlushKey,
} from "./session-cleanup-guard";
import type { VaultScope } from "./vault-scope";

import { hasUnsavedSessionNoteDrafts } from "~/session/pending-note-drafts";
import { flushDatabaseWrites } from "~/shared/write-queue";

export function captureSessionGenerationGuard(
  scope: VaultScope,
  sessionId: string,
  target: DocumentTarget,
  signal?: AbortSignal,
) {
  const client = scope.client;
  const documentId = documentDraftId(sessionId, target);
  const captured = [
    { read: () => noteDrafts.get(client.scope, sessionId) },
    { read: () => noteDrafts.get(client.scope, documentId) },
    {
      read: () =>
        documentEditorDrafts.get(
          client.scope,
          documentEditorDraftId(sessionId, target),
        ),
    },
    {
      read: () =>
        noteEditorDrafts.get(client.scope, noteEditorDraftId(sessionId)),
    },
  ].map(({ read }) => ({ read, draft: read(), value: "" }));
  let invalidated = false;
  let prepared = false;
  const refused = () =>
    new ApplicationRequestError({
      code: "conflict",
      message:
        "Confirm or recover your saved edits before generating. Your draft is kept on this device.",
      retryable: false,
      request_id: null,
      current_revision: null,
      mutation_outcome: "not_committed",
    });
  const check = () => {
    if (
      invalidated ||
      signal?.aborted ||
      scope.client !== client ||
      scope.getSnapshot().state !== "connected" ||
      !client.canEdit ||
      (client.networkMode === "online" && !onlineManager.isOnline())
    ) {
      invalidated = true;
      throw refused();
    }
  };
  const clean = () => {
    check();
    for (const entry of captured) {
      const latest = entry.read();
      if (latest && latest !== entry.draft) throw refused();
      const draft = entry.draft;
      if (!draft) continue;
      const state = draft.getSnapshot();
      if (
        state.dirty ||
        state.conflict ||
        draft.recovery().uncertain ||
        (prepared && JSON.stringify(state.draft) !== entry.value)
      )
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
    ...captured.flatMap(({ draft }) =>
      draft
        ? [
            draft.subscribe(() => {
              if (prepared && draft.getSnapshot().dirty) invalidated = true;
            }),
          ]
        : [],
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
          draftWriteQueueKey(client, documentId),
          draftWriteQueueKey(client, documentEditorDraftId(sessionId, target)),
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
        for (const entry of captured)
          entry.value = JSON.stringify(entry.draft?.getSnapshot().draft);
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
