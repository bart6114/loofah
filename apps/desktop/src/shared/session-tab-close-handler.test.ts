import { onlineManager } from "@tanstack/react-query";
import { waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  cache: new Map<string, import("~/types/tauri.gen").NoteDraftRecovery>(),
  inventory: vi.fn(),
  put: vi.fn(),
  remove: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({
  commands: {
    draftCacheInventory: native.inventory,
    draftCachePut: native.put,
    draftCacheRemove: native.remove,
  },
}));

import { createSessionTabCloseHandler } from "./desktop-tab-lifecycle";
import {
  drainDatabaseWrites,
  enqueueDatabaseWrite,
  registerWriteFlusher,
} from "./write-queue";

import { NativeDraftStore } from "~/shared/application/draft-store";
import {
  noteEditorDrafts,
  noteEditorDraftId,
} from "~/shared/application/note-editor-draft";
import type {
  DeleteEmptySessionOutcome,
  NoteEditorSnapshot,
} from "~/shared/application/protocol.gen";
import {
  draftWriteQueueKey,
  sessionEditorFlushKey,
} from "~/shared/application/session-cleanup-guard";
import type { VaultClient } from "~/shared/application/vault-client";
import type { VaultScope } from "~/shared/application/vault-scope";
import { createTabsStore } from "~/store/zustand/tabs/store";
import {
  createSessionTab,
  createSettingsTab,
} from "~/store/zustand/tabs/test-utils";

const owners: (() => void)[] = [];
const releases: (() => void)[] = [];
let sequence = 0;
function deferred<T>(fallback: T) {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  releases.push(() => resolve(fallback));
  return { promise, resolve };
}
function snapshot(
  markdown = "",
  title = "",
  revision = "n0",
): NoteEditorSnapshot {
  return {
    note: { markdown, title, revision },
    tasks: { tasks: [], revision: "t0" },
  };
}
function fixture(
  options: {
    mode?: string;
    networkMode?: "always" | "online";
    supported?: boolean;
    canEdit?: boolean;
  } = {},
) {
  const client = {
    scope: `close-handler:${++sequence}`,
    canEdit: options.canEdit ?? true,
    networkMode: options.networkMode ?? "always",
    features: options.supported === false ? [] : ["session_cleanup"],
    deleteEmptySession: vi
      .fn<VaultClient["deleteEmptySession"]>()
      .mockResolvedValue({ kind: "deleted" }),
    saveNoteEditor: vi
      .fn<VaultClient["saveNoteEditor"]>()
      .mockImplementation(async (_id, input) => ({
        kind: "saved",
        snapshot: snapshot(input.markdown, input.title ?? "", "n1"),
      })),
  };
  let connection = { state: "connected", error: null };
  let mode = options.mode ?? "idle";
  const scopeWatches = new Set<() => void>(),
    captureWatches = new Set<() => void>();
  const resync = vi.fn();
  const run = vi.fn(async <T>(operation: () => Promise<T>) => operation());
  const scope = {
    client,
    resync,
    run,
    getSnapshot: () => connection,
    subscribe: (changed: () => void) => {
      scopeWatches.add(changed);
      return () => scopeWatches.delete(changed);
    },
  } as unknown as VaultScope;
  const store = createTabsStore(
    client.scope,
    () => connection.state === "connected",
  );
  const tab = createSessionTab({ id: "same-note" });
  const handler = createSessionTabCloseHandler({
    scope,
    store,
    captureState: () => mode,
    subscribeCapture: (_id, changed) => {
      captureWatches.add(changed);
      return () => captureWatches.delete(changed);
    },
  });
  store.getState().registerOnClose(handler);
  store.getState().openCurrent(tab);
  owners.push(() => {
    handler.dispose();
    store.clear();
  });
  return {
    client,
    scope,
    run,
    resync,
    store,
    tab,
    handler,
    scopeWatches,
    captureWatches,
    close: () => store.getState().close(tab),
    state: (state: string) => {
      connection = { state, error: null };
      for (const changed of scopeWatches) changed();
    },
    capture: (next: string) => {
      mode = next;
      for (const changed of captureWatches) changed();
    },
    flusher: (flush: () => Promise<void>) => {
      const release = registerWriteFlusher(
        sessionEditorFlushKey(client.scope, tab.id),
        flush,
      );
      releases.push(release);
    },
  };
}

beforeEach(() => {
  native.cache.clear();
  native.inventory.mockReset().mockImplementation(async (scope: string) => ({
    status: "ok",
    data: [...native.cache.values()]
      .filter((item) => item.scope === scope)
      .map((item) => ({
        id: item.sessionId,
        sessionId: item.sessionId,
        title: item.draft.title,
        uncertain: item.uncertain,
      })),
  }));
  native.put
    .mockReset()
    .mockImplementation(
      async (value: import("~/types/tauri.gen").NoteDraftRecovery) => {
        native.cache.set(
          JSON.stringify([value.scope, value.sessionId]),
          structuredClone(value),
        );
        return { status: "ok", data: undefined };
      },
    );
  native.remove
    .mockReset()
    .mockImplementation(async (scope: string, id: string) => {
      native.cache.delete(JSON.stringify([scope, id]));
      return { status: "ok", data: undefined };
    });
  onlineManager.setOnline(true);
});
afterEach(async () => {
  for (const stop of owners.splice(0)) stop();
  for (const release of releases.splice(0)) release();
  await drainDatabaseWrites().catch(() => {});
  vi.useRealTimers();
  onlineManager.setOnline(true);
});

it("closes immediately and flushes captured writes before one atomic scoped request", async () => {
  const context = fixture();
  const flush = deferred<void>(undefined);
  context.flusher(() => flush.promise);
  expect(context.close()).toBeUndefined();
  expect(context.store.getState().tabs).toEqual([]);
  expect(context.client.deleteEmptySession).not.toHaveBeenCalled();
  flush.resolve();
  await waitFor(() =>
    expect(context.client.deleteEmptySession).toHaveBeenCalledOnce(),
  );
  expect(context.client.deleteEmptySession).toHaveBeenCalledWith(
    { session_id: "same-note" },
    {
      requestId: expect.stringMatching(/^[0-9a-f-]{36}$/),
      signal: expect.any(AbortSignal),
    },
  );
  expect(context.run).toHaveBeenCalledOnce();
  await waitFor(() => expect(context.resync).toHaveBeenCalledOnce());
  expect(context.scopeWatches.size).toBe(0);
  expect(context.captureWatches.size).toBe(0);
});

it("bounds burst closes to eight candidates without queueing excess cleanup", async () => {
  const context = fixture();
  const response = deferred<DeleteEmptySessionOutcome>({ kind: "not_empty" });
  context.client.deleteEmptySession.mockReturnValue(response.promise);
  context.store.getState().registerOnClose(null);
  context.close();
  const tabs = Array.from({ length: 16 }, (_, index) =>
    createSessionTab({ id: `burst-${index}` }),
  );
  for (const tab of tabs) context.store.getState().openNew(tab);
  context.store.getState().registerOnClose(context.handler);
  for (const tab of tabs) context.store.getState().close(tab);
  expect(context.store.getState().tabs).toEqual([]);
  expect(context.captureWatches.size).toBe(8);
  expect(context.scopeWatches.size).toBe(16);
  await waitFor(() =>
    expect(context.client.deleteEmptySession).toHaveBeenCalledTimes(8),
  );
  expect(native.inventory).toHaveBeenCalledTimes(8);
  response.resolve({ kind: "not_empty" });
  await waitFor(() => expect(context.scopeWatches.size).toBe(0));
  expect(context.captureWatches.size).toBe(0);
  expect(context.client.deleteEmptySession).toHaveBeenCalledTimes(8);
  expect(native.inventory).toHaveBeenCalledTimes(8);
  context.handler(tabs[8]);
  await waitFor(() =>
    expect(context.client.deleteEmptySession).toHaveBeenCalledTimes(9),
  );
});

it("deduplicates a session while its captured draft flush is pending", async () => {
  const context = fixture();
  const flush = deferred<void>(undefined);
  const flusher = vi.fn(() => flush.promise);
  context.flusher(flusher);
  context.close();
  for (let index = 0; index < 16; index++) context.handler(context.tab);
  expect(context.captureWatches.size).toBe(1);
  expect(context.scopeWatches.size).toBe(2);
  await waitFor(() => expect(flusher).toHaveBeenCalledOnce());
  expect(native.inventory).not.toHaveBeenCalled();
  expect(context.client.deleteEmptySession).not.toHaveBeenCalled();
  flush.resolve();
  await waitFor(() => expect(context.resync).toHaveBeenCalledOnce());
  expect(context.client.deleteEmptySession).toHaveBeenCalledOnce();
  expect(native.inventory).toHaveBeenCalledOnce();
});

it("retains aborted request admission until its promise settles and releases only that slot", async () => {
  const context = fixture();
  const responses = Array.from({ length: 9 }, () =>
    deferred<DeleteEmptySessionOutcome>({ kind: "not_empty" }),
  );
  for (const response of responses)
    context.client.deleteEmptySession.mockReturnValueOnce(response.promise);
  context.store.getState().registerOnClose(null);
  context.close();
  const tabs = Array.from({ length: 10 }, (_, index) =>
    createSessionTab({ id: `held-${index}` }),
  );
  for (const tab of tabs.slice(0, 8)) context.handler(tab);
  await waitFor(() =>
    expect(context.client.deleteEmptySession).toHaveBeenCalledTimes(8),
  );
  context.capture("unknown");
  context.capture("idle");
  expect(context.captureWatches.size).toBe(0);
  expect(context.scopeWatches.size).toBe(0);
  for (const [, options] of context.client.deleteEmptySession.mock.calls)
    expect(options!.signal!.aborted).toBe(true);
  context.handler(tabs[8]);
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(native.inventory).toHaveBeenCalledTimes(8);
  expect(context.client.deleteEmptySession).toHaveBeenCalledTimes(8);
  responses[0].resolve({ kind: "not_empty" });
  await new Promise((resolve) => setTimeout(resolve, 0));
  context.handler(tabs[8]);
  context.handler(tabs[9]);
  await waitFor(() =>
    expect(context.client.deleteEmptySession).toHaveBeenCalledTimes(9),
  );
  expect(native.inventory).toHaveBeenCalledTimes(9);
  for (const response of responses) response.resolve({ kind: "not_empty" });
  await waitFor(() => expect(context.scopeWatches.size).toBe(0));
  expect(context.client.deleteEmptySession).toHaveBeenCalledTimes(9);
  expect(context.resync).toHaveBeenCalledOnce();
});

it("deduplicates a reopened and reclosed session until its aborted request settles", async () => {
  const context = fixture();
  const response = deferred<DeleteEmptySessionOutcome>({ kind: "deleted" });
  context.client.deleteEmptySession.mockReturnValueOnce(response.promise);
  context.close();
  await waitFor(() =>
    expect(context.client.deleteEmptySession).toHaveBeenCalledOnce(),
  );
  context.store.getState().openNew(context.tab);
  context.close();
  context.handler(context.tab);
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(context.client.deleteEmptySession).toHaveBeenCalledOnce();
  expect(native.inventory).toHaveBeenCalledOnce();
  expect(
    context.client.deleteEmptySession.mock.calls[0][1]!.signal!.aborted,
  ).toBe(true);
  response.resolve({ kind: "deleted" });
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(context.resync).not.toHaveBeenCalled();
  context.store.getState().openNew(context.tab);
  context.close();
  await waitFor(() => expect(context.resync).toHaveBeenCalledOnce());
  expect(context.client.deleteEmptySession).toHaveBeenCalledTimes(2);
  expect(native.inventory).toHaveBeenCalledTimes(2);
});

it.each<DeleteEmptySessionOutcome>([
  { kind: "deleted" },
  { kind: "not_empty" },
  { kind: "not_found" },
])(
  "only invalidates current captured queries for advisory outcome $kind",
  async (outcome) => {
    const context = fixture();
    context.client.deleteEmptySession.mockResolvedValueOnce(outcome);
    const removeResource = vi.spyOn(
      context.store.getState(),
      "invalidateResource",
    );
    context.close();
    const history = context.store.getState().history;
    const closed = context.store.getState().closedTabs;
    await waitFor(() => expect(context.resync).toHaveBeenCalledOnce());
    expect(removeResource).not.toHaveBeenCalled();
    expect(context.store.getState().closedTabs).toHaveLength(1);
    expect(context.store.getState().history).toBe(history);
    expect(context.store.getState().closedTabs).toEqual(closed);
  },
);

it("does not treat a replayed Deleted intent as proof that a restored session is absent", async () => {
  const context = fixture();
  const response = deferred<DeleteEmptySessionOutcome>({ kind: "deleted" });
  context.client.deleteEmptySession.mockReturnValueOnce(response.promise);
  const removeResource = vi.spyOn(
    context.store.getState(),
    "invalidateResource",
  );
  context.close();
  await waitFor(() =>
    expect(context.client.deleteEmptySession).toHaveBeenCalledOnce(),
  );
  response.resolve({ kind: "deleted" });
  await waitFor(() => expect(context.resync).toHaveBeenCalledOnce());
  expect(removeResource).not.toHaveBeenCalled();
  expect(context.store.getState().closedTabs[0]).toMatchObject({
    id: "same-note",
  });
});

it.each(["active", "finalizing", "running_batch", "unknown"])(
  "skips capture state %s",
  async (mode) => {
    const context = fixture({ mode });
    context.close();
    await Promise.resolve();
    expect(context.client.deleteEmptySession).not.toHaveBeenCalled();
    expect(native.inventory).not.toHaveBeenCalled();
    expect(context.scopeWatches.size).toBe(0);
  },
);

it("skips missing or incomplete capture observation instead of assuming idle", async () => {
  const context = fixture();
  context.store.getState().registerOnClose(null);
  context.close();
  const missing = createSessionTabCloseHandler({
    scope: context.scope,
    store: context.store,
  });
  const incomplete = createSessionTabCloseHandler({
    scope: context.scope,
    store: context.store,
    captureState: () => "idle",
  });
  missing(context.tab);
  incomplete(context.tab);
  missing.dispose();
  incomplete.dispose();
  await Promise.resolve();
  expect(context.client.deleteEmptySession).not.toHaveBeenCalled();
  expect(native.inventory).not.toHaveBeenCalled();
});

it("releases candidate watches if capture observation cannot be established", async () => {
  const context = fixture();
  context.store.getState().registerOnClose(null);
  context.close();
  const unavailable = createSessionTabCloseHandler({
    scope: context.scope,
    store: context.store,
    captureState: () => "idle",
    subscribeCapture: () => {
      throw new Error("capture state unavailable");
    },
  });
  expect(() => unavailable(context.tab)).not.toThrow();
  unavailable.dispose();
  await Promise.resolve();
  expect(context.client.deleteEmptySession).not.toHaveBeenCalled();
  expect(context.scopeWatches.size).toBe(0);
  expect(native.inventory).not.toHaveBeenCalled();
});

it.each([{ supported: false }, { canEdit: false }])(
  "skips unsupported/read-only clients without device reads",
  async (options) => {
    const context = fixture(options);
    context.close();
    await Promise.resolve();
    expect(context.client.deleteEmptySession).not.toHaveBeenCalled();
    expect(native.inventory).not.toHaveBeenCalled();
  },
);

it("skips unavailable scopes and non-session tabs", async () => {
  const context = fixture();
  context.state("reconnecting");
  context.handler(context.tab);
  context.handler(createSettingsTab());
  await Promise.resolve();
  expect(context.client.deleteEmptySession).not.toHaveBeenCalled();
});

it("does not send a captured scope's operation through another scope's tab store", async () => {
  const first = fixture(),
    second = fixture();
  const mismatch = createSessionTabCloseHandler({
    scope: first.scope,
    store: second.store,
    captureState: () => "idle",
    subscribeCapture: () => () => {},
  });
  second.store.getState().registerOnClose(null);
  second.close();
  mismatch(second.tab);
  mismatch.dispose();
  await Promise.resolve();
  expect(first.client.deleteEmptySession).not.toHaveBeenCalled();
  expect(second.client.deleteEmptySession).not.toHaveBeenCalled();
});

it("keeps local offline/no-login cleanup and skips online-only admission offline", async () => {
  const offline = fixture({ networkMode: "always" }),
    online = fixture({ networkMode: "online" });
  onlineManager.setOnline(false);
  offline.close();
  online.close();
  await waitFor(() =>
    expect(offline.client.deleteEmptySession).toHaveBeenCalledOnce(),
  );
  expect(online.client.deleteEmptySession).not.toHaveBeenCalled();
});

it("latches reopen then reclose while a flush is pending", async () => {
  const context = fixture();
  const flush = deferred<void>(undefined);
  context.flusher(() => flush.promise);
  context.close();
  context.store.getState().registerOnClose(null);
  context.store.getState().openNew(context.tab);
  context.store.getState().close(context.tab);
  flush.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(context.client.deleteEmptySession).not.toHaveBeenCalled();
  expect(context.scopeWatches.size).toBe(0);
  expect(context.captureWatches.size).toBe(0);
});

it("latches idle to active to idle during preparation", async () => {
  const context = fixture();
  const flush = deferred<void>(undefined);
  context.flusher(() => flush.promise);
  context.close();
  context.capture("active");
  context.capture("idle");
  flush.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(context.client.deleteEmptySession).not.toHaveBeenCalled();
});

it("disposes a retired handler without canceling its already owned durable save", async () => {
  const context = fixture();
  const saved = deferred<Awaited<ReturnType<VaultClient["saveNoteEditor"]>>>({
    kind: "saved",
    snapshot: snapshot("Owned save", "", "n1"),
  });
  context.client.saveNoteEditor.mockReturnValueOnce(saved.promise);
  const draft = noteEditorDrafts.open(
    context.client,
    context.tab.id,
    snapshot(),
  );
  draft.edit({ markdown: "Owned save", title: "", tasks: [] });
  context.flusher(() =>
    enqueueDatabaseWrite(
      draftWriteQueueKey(context.client, context.tab.id),
      async () => {
        await draft.save();
      },
    ),
  );
  context.close();
  await waitFor(() =>
    expect(context.client.saveNoteEditor).toHaveBeenCalledOnce(),
  );
  context.handler.dispose();
  saved.resolve({ kind: "saved", snapshot: snapshot("Owned save", "", "n1") });
  await drainDatabaseWrites();
  expect(draft.getSnapshot().baseline.note.markdown).toBe("Owned save");
  expect(context.client.deleteEmptySession).not.toHaveBeenCalled();
  expect(context.scopeWatches.size).toBe(0);
});

it("preserves a newly typed durable recovery when reopen aborts a sent cleanup with unknown outcome", async () => {
  const context = fixture();
  const response = deferred<DeleteEmptySessionOutcome>({ kind: "deleted" });
  context.client.deleteEmptySession.mockReturnValueOnce(response.promise);
  context.close();
  await waitFor(() =>
    expect(context.client.deleteEmptySession).toHaveBeenCalledOnce(),
  );
  const signal = context.client.deleteEmptySession.mock.calls[0][1]!.signal!;
  context.store.getState().openNew(context.tab);
  const draft = noteEditorDrafts.open(
    context.client,
    context.tab.id,
    snapshot(),
  );
  draft.edit({
    markdown: "New private typing after dispatch",
    title: "",
    tasks: [],
  });
  await new NativeDraftStore(context.client.scope).save(draft.recovery());
  expect(signal.aborted).toBe(true);
  response.resolve({ kind: "deleted" });
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(context.resync).not.toHaveBeenCalled();
  expect(context.store.getState().currentTab).toMatchObject({
    id: "same-note",
  });
  expect(draft.getSnapshot().draft.markdown).toBe(
    "New private typing after dispatch",
  );
  expect(
    native.cache.get(
      JSON.stringify([context.client.scope, noteEditorDraftId(context.tab.id)]),
    )?.draft.markdown,
  ).toBe("New private typing after dispatch");
  expect(native.remove).not.toHaveBeenCalled();
});

it("does not revive a sent candidate after a capture transition or access loss", async () => {
  const context = fixture();
  const response = deferred<DeleteEmptySessionOutcome>({ kind: "not_empty" });
  context.client.deleteEmptySession.mockReturnValueOnce(response.promise);
  context.close();
  await waitFor(() =>
    expect(context.client.deleteEmptySession).toHaveBeenCalledOnce(),
  );
  const signal = context.client.deleteEmptySession.mock.calls[0][1]!.signal!;
  context.capture("unknown");
  context.capture("idle");
  context.state("access_denied");
  context.state("connected");
  response.resolve({ kind: "not_empty" });
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(signal.aborted).toBe(true);
  expect(context.resync).not.toHaveBeenCalled();
  expect(context.captureWatches.size).toBe(0);
});

it("does not retry an uncertain network result or remove local navigation state", async () => {
  const context = fixture();
  const error = vi.spyOn(console, "error").mockImplementation(() => {});
  context.client.deleteEmptySession.mockRejectedValueOnce(
    new Error("response lost after dispatch"),
  );
  context.close();
  await waitFor(() => expect(error).toHaveBeenCalledOnce());
  context.state("reconnecting");
  context.state("connected");
  expect(context.client.deleteEmptySession).toHaveBeenCalledOnce();
  expect(context.resync).not.toHaveBeenCalled();
  expect(context.store.getState().closedTabs).toHaveLength(1);
  expect(context.scopeWatches.size).toBe(0);
  error.mockRestore();
});

it.each([50, 150, 500])(
  "does not delay close feedback for a %dms request",
  async (latency) => {
    vi.useFakeTimers();
    const context = fixture();
    context.client.deleteEmptySession.mockImplementationOnce(
      () =>
        new Promise((resolve) =>
          setTimeout(() => resolve({ kind: "not_empty" }), latency),
        ),
    );
    context.close();
    expect(context.store.getState().tabs).toEqual([]);
    await vi.advanceTimersByTimeAsync(0);
    expect(context.client.deleteEmptySession).toHaveBeenCalledOnce();
    expect(context.resync).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(latency);
    expect(context.resync).toHaveBeenCalledOnce();
  },
);
