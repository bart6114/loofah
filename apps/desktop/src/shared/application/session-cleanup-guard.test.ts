import { onlineManager } from "@tanstack/react-query";
import { waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { documentEditorDrafts } from "./document-editor-draft";
import { NativeDraftStore } from "./draft-store";
import { editorText } from "./note-editor-draft";
import { noteEditorDraftId, noteEditorDrafts } from "./note-editor-draft";
import type { NoteEditorSnapshot, SaveNoteEditorRequest } from "./protocol.gen";
import {
  captureSessionCleanupGuard,
  draftWriteQueueKey,
  draftSessionId,
  sessionEditorFlushKey,
} from "./session-cleanup-guard";
import type { VaultScope } from "./vault-scope";

import {
  captureNoteDraft,
  discardNoteDraft,
  enhancedNoteDraftKey,
} from "~/session/pending-note-drafts";
import {
  drainDatabaseWrites,
  enqueueDatabaseWrite,
  registerWriteFlusher,
} from "~/shared/write-queue";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
function baseline(markdown = "", revision = "n0"): NoteEditorSnapshot {
  return {
    note: { markdown, title: "", revision },
    tasks: { tasks: [], revision: "t0" },
  };
}
let sequence = 0;
const guards: ReturnType<typeof captureSessionCleanupGuard>[] = [];
const releases: (() => void)[] = [];
const pendingKeys: string[] = [];
const inventory = vi.spyOn(NativeDraftStore.prototype, "inventory");

function fixture(
  networkMode: "always" | "online" = "always",
  sessionId = "same-note",
) {
  let connection = { state: "connected", error: null };
  const listeners = new Set<() => void>();
  const save = vi.fn(async (_id: string, value: SaveNoteEditorRequest) => ({
    kind: "saved" as const,
    snapshot: baseline(value.markdown, "n1"),
  }));
  const client = {
    scope: `cleanup:${++sequence}`,
    canEdit: true,
    networkMode,
    saveNoteEditor: save,
  };
  const scope = {
    client,
    getSnapshot: () => connection,
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  } as unknown as VaultScope;
  const draft = noteEditorDrafts.open(client, sessionId, baseline());
  let reopened = false;
  const guard = captureSessionCleanupGuard(scope, sessionId, {
    isOpen: () => reopened,
  });
  guards.push(guard);
  return {
    scope,
    client,
    save,
    draft,
    guard,
    sessionId,
    listeners,
    reopen: () => {
      reopened = true;
    },
    state: (state: string) => {
      connection = { state, error: null };
      for (const listener of listeners) listener();
    },
    flusher: (flush: () => Promise<void>) => {
      const release = registerWriteFlusher(
        sessionEditorFlushKey(client.scope, sessionId),
        flush,
      );
      releases.push(release);
      return release;
    },
  };
}

beforeEach(() => {
  inventory.mockReset().mockResolvedValue([]);
  onlineManager.setOnline(true);
});
afterEach(async () => {
  for (const guard of guards.splice(0)) guard.dispose();
  for (const release of releases.splice(0)) release();
  for (const key of pendingKeys.splice(0)) discardNoteDraft(key);
  vi.useRealTimers();
  onlineManager.setOnline(true);
  await drainDatabaseWrites();
});

it("prepares a clean captured session and releases watches without changing its draft", async () => {
  const { guard, client, draft, listeners } = fixture();
  expect(guard.canDelete()).toBe(false);
  expect(await guard.prepare()).toBe(true);
  expect(inventory.mock.contexts[0]).toMatchObject({ scope: client.scope });
  expect(guard.canDelete()).toBe(true);
  expect(draft.getSnapshot().dirty).toBe(false);
  guard.dispose();
  expect(listeners.size).toBe(0);
  expect(guard.canDelete()).toBe(false);
});

it("flushes pre-debounce raw captures before considering cleanup without duplicating an emptiness predicate", async () => {
  const { guard, draft, flusher, client, save } = fixture();
  draft.capture(() => ({
    markdown: "First 500ms private edit",
    title: "",
    tasks: [],
  }));
  flusher(() =>
    enqueueDatabaseWrite(
      draftWriteQueueKey(client, draft.sessionId),
      async () => {
        await draft.save();
      },
    ),
  );
  expect(await guard.prepare()).toBe(true);
  expect(save).toHaveBeenCalledOnce();
  expect(draft.getSnapshot().baseline.note.markdown).toBe(
    "First 500ms private edit",
  );
  expect(draft.getSnapshot().dirty).toBe(false);
});

it("skips dirty or conflicted drafts if no safe registered flusher owns them", async () => {
  const { guard, draft, save } = fixture();
  draft.edit({ markdown: "Unsent", title: "", tasks: [] });
  draft.receive(baseline("Changed elsewhere", "n2"));
  expect(await guard.prepare()).toBe(false);
  expect(save).not.toHaveBeenCalled();
  expect(inventory).not.toHaveBeenCalled();
  expect(draft.getSnapshot().draft.markdown).toBe("Unsent");
});

it.each([
  "same-note",
  '["note_editor","same-note"]',
  '["summary","same-note"]',
  '["template_output","same-note","doc"]',
])("skips existing device recovery for resource %s", async (id) => {
  const { guard } = fixture();
  inventory.mockResolvedValueOnce([
    { id: "recovery", sessionId: id, title: "Private", uncertain: true },
  ]);
  expect(await guard.prepare()).toBe(false);
  expect(guard.canDelete()).toBe(false);
});

it("does not block a clean session on another session's recovery", async () => {
  const { guard } = fixture();
  inventory.mockResolvedValueOnce([
    {
      id: "other",
      sessionId: '["note_editor","other-note"]',
      title: "Other",
      uncertain: true,
    },
  ]);
  expect(await guard.prepare()).toBe(true);
});

it("keeps login-free IPC cleanup admission available offline and skips online-only clients", async () => {
  const local = fixture("always"),
    online = fixture("online");
  onlineManager.setOnline(false);
  expect(await local.guard.prepare()).toBe(true);
  expect(await online.guard.prepare()).toBe(false);
  expect(inventory).toHaveBeenCalledOnce();
});

it("skips read-only clients before flushing or reading device data", async () => {
  const { guard, client, flusher } = fixture();
  client.canEdit = false;
  const flush = vi.fn(async () => {});
  flusher(flush);
  expect(await guard.prepare()).toBe(false);
  expect(flush).not.toHaveBeenCalled();
  expect(inventory).not.toHaveBeenCalled();
});

it("does not revive after access loss and reconnect while a captured write is pending", async () => {
  const { guard, draft, flusher, client, save, state } = fixture();
  const write = deferred<{ kind: "saved"; snapshot: NoteEditorSnapshot }>();
  save.mockReturnValueOnce(write.promise);
  draft.edit({ markdown: "Owned durable save", title: "", tasks: [] });
  flusher(() =>
    enqueueDatabaseWrite(
      draftWriteQueueKey(client, draft.sessionId),
      async () => {
        await draft.save();
      },
    ),
  );
  const preparation = guard.prepare();
  await waitFor(() => expect(save).toHaveBeenCalledOnce());
  state("access_denied");
  state("connected");
  expect(await preparation).toBe(false);
  write.resolve({
    kind: "saved",
    snapshot: baseline("Owned durable save", "n1"),
  });
  await drainDatabaseWrites();
  expect(draft.getSnapshot().baseline.note.markdown).toBe("Owned durable save");
  expect(guard.canDelete()).toBe(false);
  expect(inventory).not.toHaveBeenCalled();
});

it("times out optional cleanup while allowing its durable write to finish", async () => {
  vi.useFakeTimers();
  const { guard, draft, flusher, client, save } = fixture();
  const write = deferred<{ kind: "saved"; snapshot: NoteEditorSnapshot }>();
  save.mockReturnValueOnce(write.promise);
  draft.edit({ markdown: "Still owned", title: "", tasks: [] });
  flusher(() =>
    enqueueDatabaseWrite(
      draftWriteQueueKey(client, draft.sessionId),
      async () => {
        await draft.save();
      },
    ),
  );
  const preparation = guard.prepare();
  await vi.advanceTimersByTimeAsync(3000);
  expect(await preparation).toBe(false);
  expect(save).toHaveBeenCalledOnce();
  write.resolve({ kind: "saved", snapshot: baseline("Still owned", "n1") });
  await drainDatabaseWrites();
  expect(draft.getSnapshot().dirty).toBe(false);
  expect(guard.canDelete()).toBe(false);
  expect(inventory).not.toHaveBeenCalled();
});

it("rejects a latched reopen even after the captured write completes", async () => {
  const { guard, flusher, reopen } = fixture();
  const flush = deferred<void>();
  flusher(() => flush.promise);
  const preparation = guard.prepare();
  reopen();
  flush.resolve();
  expect(await preparation).toBe(false);
  expect(inventory).not.toHaveBeenCalled();
});

it("ignores late recovery replies after retirement and never clears newly typed content", async () => {
  const { guard, draft, state } = fixture();
  const read = deferred<Awaited<ReturnType<NativeDraftStore["inventory"]>>>();
  inventory.mockReturnValueOnce(read.promise);
  const preparation = guard.prepare();
  await waitFor(() => expect(inventory).toHaveBeenCalledOnce());
  state("closed");
  draft.edit({ markdown: "Reopened private draft", title: "", tasks: [] });
  expect(await preparation).toBe(false);
  read.resolve([]);
  await Promise.resolve();
  expect(guard.canDelete()).toBe(false);
  expect(draft.getSnapshot().draft.markdown).toBe("Reopened private draft");
});

it("checks scoped pending enhanced documents without mixing identical IDs across vaults", async () => {
  const { guard, client, sessionId } = fixture();
  const other = enhancedNoteDraftKey("other-vault", sessionId, "doc");
  captureNoteDraft(other, () => ({ type: "doc", content: [] }));
  pendingKeys.push(other);
  expect(await guard.prepare()).toBe(true);
  const own = enhancedNoteDraftKey(client.scope, sessionId, "doc");
  captureNoteDraft(own, () => ({ type: "doc", content: [] }));
  pendingKeys.push(own);
  expect(guard.canDelete()).toBe(false);
  discardNoteDraft(own);
  expect(guard.canDelete()).toBe(false);
});

it("skips rejected flushes and device reads rather than discarding recovery ownership", async () => {
  const first = fixture();
  first.flusher(async () => {
    throw new Error("failed save");
  });
  expect(await first.guard.prepare()).toBe(false);
  const second = fixture();
  inventory.mockRejectedValueOnce(new Error("failed recovery read"));
  expect(await second.guard.prepare()).toBe(false);
});

it("invalidates the candidate if a different draft replaces the captured registry entry", async () => {
  const { guard, draft, client, sessionId } = fixture();
  const read = deferred<Awaited<ReturnType<NativeDraftStore["inventory"]>>>();
  inventory.mockReturnValueOnce(read.promise);
  const preparation = guard.prepare();
  await waitFor(() => expect(inventory).toHaveBeenCalledOnce());
  noteEditorDrafts.restore(client, sessionId, draft.recovery());
  read.resolve([]);
  expect(await preparation).toBe(false);
  expect(
    noteEditorDrafts.get(client.scope, noteEditorDraftId(sessionId)),
  ).not.toBe(draft);
});

it.each(["summary", "template_output"] as const)(
  "blocks cleanup on compound %s edits for the physical session owner",
  async (kind) => {
    const context = fixture();
    const target =
      kind === "summary" ? { kind } : { kind, document_id: "output" };
    const draft = documentEditorDrafts.open(
      { ...context.client, saveDocumentEditor: vi.fn() },
      context.sessionId,
      target,
      {
        ...baseline("Enhanced body"),
        metadata: {
          meta: {
            id: context.sessionId,
            title: "",
            created_at: "",
            started_at: null,
            ended_at: null,
            tags: [],
          },
          revision: "m0",
        },
      },
    );
    draft.edit({
      ...editorText(draft.getSnapshot().baseline),
      markdown: "Unsent Enhanced edits",
    });
    const captured = captureSessionCleanupGuard(
      context.scope,
      context.sessionId,
      { isOpen: () => false },
    );
    guards.push(captured);
    await expect(captured.prepare()).resolves.toBe(false);
    expect(inventory).not.toHaveBeenCalled();
  },
);
it("recognizes only valid compound recovery owner coordinates", () => {
  expect(draftSessionId('["document_editor","owner","summary"]')).toBe("owner");
  expect(
    draftSessionId('["document_editor","owner","template_output","output"]'),
  ).toBe("owner");
  const wrong = '["document_editor","owner","unknown"]';
  expect(draftSessionId(wrong)).toBe(wrong);
});
