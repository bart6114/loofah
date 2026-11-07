import { QueryObserver } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";

import type { AttachmentResource } from "./attachments";
import { ApplicationRequestError } from "./errors";
import type { PlaybackRecording } from "./playback";
import type { NoteEditorSnapshot, NoteSnapshot } from "./protocol.gen";
import type { VaultClient, VaultClientEvent } from "./vault-client";
import { VaultScope } from "./vault-scope";

function fixture(scope: string) {
  let event!: (value: VaultClientEvent) => void;
  const unsubscribe = vi.fn();
  const backend = {
    scope,
    networkMode: "always",
    canEdit: true,
    readNote: vi
      .fn()
      .mockResolvedValue({ markdown: scope, title: "Title", revision: scope }),
    readNoteEditor: vi.fn(),
    openRecording: vi.fn(),
    openAttachment: vi.fn(),
    session: vi.fn(),
    sessions: vi.fn(),
    search: vi.fn(),
    subscribe: vi.fn(async (listener) => {
      event = listener;
      listener({ kind: "connection", state: "connected", error: null });
      return unsubscribe;
    }),
  } as unknown as VaultClient & {
    readNote: ReturnType<typeof vi.fn>;
    readNoteEditor: ReturnType<typeof vi.fn>;
    openRecording: ReturnType<typeof vi.fn>;
    openAttachment: ReturnType<typeof vi.fn>;
  };
  const active = new VaultScope(backend);
  return {
    backend,
    active,
    unsubscribe,
    emit: (value: VaultClientEvent) => event(value),
  };
}

function watchCacheCleanup(scope: VaultScope) {
  const cache = scope.queryClient.getQueryCache();
  const subscribe = cache.subscribe.bind(cache);
  const stopped = vi.fn();
  vi.spyOn(cache, "subscribe").mockImplementation((listener) => {
    const stop = subscribe(listener);
    return () => {
      stopped();
      stop();
    };
  });
  return stopped;
}

describe("vault-scoped query lifecycle", () => {
  it("invalidates only the changed private job and job lists while preserving other vaults", async () => {
    const { active, emit } = fixture("scope");
    await active.start();
    const listInput = {
      limit: 50,
      after: null,
      session_id: "note",
      active_only: false,
    };
    const changed = active.processingJobOptions("changed").queryKey;
    const result = active.processingJobResultOptions("changed").queryKey;
    const other = active.processingJobOptions("other").queryKey;
    const list = active.processingJobsOptions(listInput).queryKey;
    const noteKey = active.noteOptions("note").queryKey;
    const foreign = ["vault", "another-scope", "processing_job", "changed"];
    for (const key of [changed, result, other, list, noteKey, foreign])
      active.queryClient.setQueryData(key, { retained: true });
    emit({
      kind: "change",
      change: { kind: "job_changed", job_id: "changed" },
    });
    for (const key of [changed, result, list])
      expect(active.queryClient.getQueryState(key)?.isInvalidated).toBe(true);
    for (const key of [other, noteKey, foreign])
      expect(active.queryClient.getQueryState(key)?.isInvalidated).toBe(false);
    emit({ kind: "connection", state: "reconnecting", error: null });
    emit({ kind: "connection", state: "connected", error: null });
    expect(active.queryClient.getQueryState(other)?.isInvalidated).toBe(true);
    expect(active.queryClient.getQueryState(foreign)?.isInvalidated).toBe(
      false,
    );
    emit({ kind: "connection", state: "access_denied", error: null });
    expect(active.queryClient.getQueryData(result)).toBeUndefined();
    active.dispose();
  });
  it("shares attachment handles across duplicate references and closes them on replacement and last-observer removal", async () => {
    const { active, backend, emit } = fixture("scope");
    await active.start();
    const resource = (id: string): AttachmentResource => ({
      id,
      info: { filename: "a.png", contentType: "image/png", sizeBytes: 42 },
      src: `media:${id}`,
      labels: { open: { id: "open" }, reveal: { id: "reveal" } },
      open: vi.fn(),
      reveal: vi.fn(),
      close: vi.fn().mockResolvedValue(undefined),
    });
    const first = resource("one"),
      second = resource("two"),
      other = resource("other");
    backend.openAttachment
      .mockResolvedValueOnce(first)
      .mockResolvedValueOnce(second)
      .mockResolvedValueOnce(other);
    const options = active.attachmentOptions("note", "a.png");
    const observer = new QueryObserver(active.queryClient, options);
    const duplicate = new QueryObserver(active.queryClient, options);
    const stop = observer.subscribe(() => {});
    await vi.waitFor(() =>
      expect(observer.getCurrentResult().data).toBe(first),
    );
    const stopDuplicate = duplicate.subscribe(() => {});
    expect(duplicate.getCurrentResult().data).toBe(first);
    expect(backend.openAttachment).toHaveBeenCalledTimes(1);
    stop();
    expect(first.close).not.toHaveBeenCalled();
    emit({
      kind: "change",
      change: {
        kind: "index_changed",
        entity: "artifacts",
        ids: ["note"],
        artifacts: [{ session_id: "note", target: { kind: "recording" } }],
      },
    });
    expect(backend.openAttachment).toHaveBeenCalledTimes(1);
    emit({
      kind: "change",
      change: {
        kind: "index_changed",
        entity: "artifacts",
        ids: ["note"],
        artifacts: [
          {
            session_id: "note",
            target: { kind: "attachment", attachment_id: "a.png" },
          },
        ],
      },
    });
    await vi.waitFor(() =>
      expect(duplicate.getCurrentResult().data).toBe(second),
    );
    expect(first.close).toHaveBeenCalledOnce();
    expect(second.close).not.toHaveBeenCalled();
    stopDuplicate();
    await vi.waitFor(() => expect(second.close).toHaveBeenCalledOnce());
    await active.queryClient.fetchQuery(
      active.attachmentOptions("other", "a.png"),
    );
    emit({ kind: "connection", state: "access_denied", error: null });
    expect(other.close).toHaveBeenCalledOnce();
    active.dispose();
    expect(other.close).toHaveBeenCalledOnce();
  });

  it("closes attachment handles that arrive after cancellation or scope disposal", async () => {
    for (const dispose of [false, true]) {
      const { active, backend } = fixture(`scope:${dispose}`);
      let finish!: (resource: AttachmentResource) => void;
      backend.openAttachment.mockReturnValue(
        new Promise((resolve) => {
          finish = resolve;
        }),
      );
      const options = active.attachmentOptions("note", "a.png");
      const opening = active.queryClient.fetchQuery(options);
      const rejected = expect(opening).rejects.toThrow();
      if (dispose) active.dispose();
      else
        await active.queryClient.cancelQueries({ queryKey: options.queryKey });
      const close = vi.fn().mockResolvedValue(undefined);
      finish({
        id: "late",
        info: { filename: "a.png", contentType: "image/png", sizeBytes: 42 },
        labels: { open: { id: "open" }, reveal: { id: "reveal" } },
        open: vi.fn(),
        reveal: vi.fn(),
        close,
      });
      await rejected;
      await vi.waitFor(() => expect(close).toHaveBeenCalledOnce());
      expect(active.queryClient.getQueryData(options.queryKey)).toBeUndefined();
      active.dispose();
    }
  });

  it("invalidates only the named media resource and keeps legacy broad events safe", async () => {
    const { active, emit } = fixture("scope");
    await active.start();
    const keys = {
      audio: ["vault", "scope", "audio_resource", "first"],
      otherAudio: ["vault", "scope", "audio_resource", "second"],
      image: ["vault", "scope", "attachment_resource", "first", "a.png"],
      otherImage: ["vault", "scope", "attachment_resource", "first", "b.png"],
      info: ["vault", "scope", "attachment_info", "first", "a.png"],
      list: ["vault", "scope", "attachment_list", "first"],
      otherNoteImage: [
        "vault",
        "scope",
        "attachment_resource",
        "second",
        "a.png",
      ],
      foreign: ["vault", "other", "attachment_resource", "first", "a.png"],
      note: ["vault", "scope", "note", "first"],
    };
    const reset = () => {
      for (const key of Object.values(keys))
        active.queryClient.setQueryData(key, null);
    };
    const invalidated = () =>
      Object.entries(keys)
        .filter(
          ([, key]) => active.queryClient.getQueryState(key)?.isInvalidated,
        )
        .map(([name]) => name);
    reset();
    for (let i = 0; i < 100; i++)
      emit({
        kind: "change",
        change: {
          kind: "index_changed",
          entity: "artifacts",
          ids: ["first"],
          artifacts: [{ session_id: "first", target: { kind: "recording" } }],
        },
      });
    expect(invalidated()).toEqual(["audio"]);
    reset();
    emit({
      kind: "change",
      change: {
        kind: "index_changed",
        entity: "artifacts",
        ids: ["first"],
        artifacts: [
          {
            session_id: "first",
            target: { kind: "attachment", attachment_id: "a.png" },
          },
        ],
      },
    });
    expect(invalidated()).toEqual(["image", "info", "list"]);
    reset();
    emit({
      kind: "change",
      change: {
        kind: "index_changed",
        entity: "artifacts",
        ids: ["first", "second"],
        artifacts: [
          { session_id: "first", target: { kind: "attachments" } },
          { session_id: "second", target: { kind: "recording" } },
        ],
      },
    });
    expect(invalidated()).toEqual([
      "otherAudio",
      "image",
      "otherImage",
      "info",
      "list",
    ]);
    reset();
    emit({
      kind: "change",
      change: {
        kind: "index_changed",
        entity: "artifacts",
        ids: ["first"],
        artifacts: [{ session_id: "first", target: { kind: "all" } }],
      },
    });
    expect(invalidated()).toEqual([
      "audio",
      "image",
      "otherImage",
      "info",
      "list",
    ]);
    reset();
    emit({
      kind: "change",
      change: { kind: "index_changed", entity: "artifacts", ids: ["first"] },
    });
    expect(invalidated()).toEqual([
      "audio",
      "image",
      "otherImage",
      "info",
      "list",
    ]);
    reset();
    emit({
      kind: "change",
      change: { kind: "index_changed", entity: "artifacts", ids: [] },
    });
    expect(invalidated()).toEqual([
      "audio",
      "otherAudio",
      "image",
      "otherImage",
      "info",
      "list",
      "otherNoteImage",
    ]);
    active.dispose();
  });

  it("does not reopen recordings for note, title or task edits, and scopes artifact invalidation", async () => {
    const { active, emit } = fixture("scope");
    await active.start();
    const keys = ["audio_resource", "audio_peaks"].map((kind) => ({
      first: ["vault", "scope", kind, "first"],
      second: ["vault", "scope", kind, "second"],
      other: ["vault", "other", kind, "first"],
    }));
    for (const group of keys)
      for (const key of Object.values(group))
        active.queryClient.setQueryData(key, null);
    for (const entity of ["sessions", "session_headers", "tasks"] as const) {
      emit({
        kind: "change",
        change: { kind: "index_changed", entity, ids: ["first"] },
      });
      for (const group of keys)
        expect(
          active.queryClient.getQueryState(group.first)?.isInvalidated,
        ).toBe(false);
    }
    emit({
      kind: "change",
      change: { kind: "index_changed", entity: "artifacts", ids: ["first"] },
    });
    for (const group of keys) {
      expect(active.queryClient.getQueryState(group.first)?.isInvalidated).toBe(
        true,
      );
      expect(
        active.queryClient.getQueryState(group.second)?.isInvalidated,
      ).toBe(false);
      expect(active.queryClient.getQueryState(group.other)?.isInvalidated).toBe(
        false,
      );
    }
    active.dispose();
  });
  it("releases recordings on replacement, last-observer removal and scope shutdown", async () => {
    const { active, backend } = fixture("scope");
    await active.start();
    const resource = (id: string): PlaybackRecording => ({
      id,
      url: `media:${id}`,
      resumeKey: id,
      deletionRevision: vi.fn().mockResolvedValue("sha"),
      peaks: vi.fn().mockResolvedValue(null),
      close: vi.fn().mockResolvedValue(undefined),
    });
    const first = resource("one"),
      second = resource("two"),
      third = resource("three");
    backend.openRecording
      .mockResolvedValueOnce(first)
      .mockResolvedValueOnce(second)
      .mockResolvedValueOnce(third);
    const options = active.recordingOptions("session");
    const observer = new QueryObserver(active.queryClient, options);
    const stop = observer.subscribe(() => {});
    await vi.waitFor(() =>
      expect(observer.getCurrentResult().data).toBe(first),
    );
    await observer.refetch();
    expect(first.close).toHaveBeenCalledOnce();
    expect(second.close).not.toHaveBeenCalled();
    stop();
    await vi.waitFor(() => expect(second.close).toHaveBeenCalledOnce());
    await active.queryClient.fetchQuery(active.recordingOptions("other"));
    active.dispose();
    expect(third.close).toHaveBeenCalledOnce();
  });

  it("closes a recording returned after a cancelled query instead of caching it", async () => {
    const { active, backend } = fixture("scope");
    const close = vi.fn().mockResolvedValue(undefined);
    let finish!: (value: PlaybackRecording) => void;
    backend.openRecording.mockReturnValue(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    const options = active.recordingOptions("session");
    const opening = active.queryClient.fetchQuery(options);
    const rejected = expect(opening).rejects.toThrow();
    await active.queryClient.cancelQueries({ queryKey: options.queryKey });
    finish({
      id: "late",
      url: "media:late",
      resumeKey: "late",
      deletionRevision: vi.fn().mockResolvedValue("sha"),
      peaks: vi.fn(),
      close,
    });
    await rejected;
    await vi.waitFor(() => expect(close).toHaveBeenCalledOnce());
    expect(active.queryClient.getQueryData(options.queryKey)).toBeUndefined();
    active.dispose();
  });

  it("releases media on access loss without waiting for the view to unmount", async () => {
    const { active, backend, emit } = fixture("scope");
    await active.start();
    const close = vi.fn().mockResolvedValue(undefined);
    backend.openRecording.mockResolvedValue({
      id: "id",
      url: "media:id",
      peaks: vi.fn(),
      close,
    });
    await active.queryClient.fetchQuery(active.recordingOptions("session"));
    emit({ kind: "connection", state: "access_denied", error: null });
    expect(close).toHaveBeenCalledOnce();
    active.dispose();
  });
  it("invalidates recording metadata only for matching artifact changes or resync", async () => {
    const { active, emit } = fixture("scope");
    await active.start();
    const first = active.audioMetadataOptions("first").queryKey;
    const second = active.audioMetadataOptions("second").queryKey;
    const other = ["vault", "another", "audio_metadata", "first"];
    for (const key of [first, second, other])
      active.queryClient.setQueryData(key, null);
    for (const entity of ["sessions", "session_headers", "tasks"] as const) {
      emit({
        kind: "change",
        change: { kind: "index_changed", entity, ids: ["first"] },
      });
      expect(active.queryClient.getQueryState(first)?.isInvalidated).toBe(
        false,
      );
    }
    emit({
      kind: "change",
      change: { kind: "index_changed", entity: "artifacts", ids: ["first"] },
    });
    expect(active.queryClient.getQueryState(first)?.isInvalidated).toBe(true);
    expect(active.queryClient.getQueryState(second)?.isInvalidated).toBe(false);
    expect(active.queryClient.getQueryState(other)?.isInvalidated).toBe(false);
    active.resync();
    expect(active.queryClient.getQueryState(second)?.isInvalidated).toBe(true);
    expect(active.queryClient.getQueryState(other)?.isInvalidated).toBe(false);
    active.dispose();
  });

  it("refreshes newer task-only canonical state after an older compound save reply", async () => {
    const { active, backend, emit } = fixture("scope");
    await active.start();
    const options = active.noteEditorOptions("note");
    const value = (revision: string): NoteEditorSnapshot => ({
      note: {
        markdown: "Same Markdown",
        title: "Title",
        revision: "same-note",
      },
      tasks: { tasks: [], revision },
    });
    const initial = value("t0"),
      saved = value("t1"),
      newer = value("t2");
    active.queryClient.setQueryData(options.queryKey, initial);
    const otherKey = active.noteEditorOptions("other").queryKey;
    active.queryClient.setQueryData(otherKey, initial);
    const tasksKey = active.tasksSnapshotOptions("note").queryKey;
    active.queryClient.setQueryData(tasksKey, initial.tasks);
    backend.readNoteEditor.mockResolvedValue(newer);
    const observer = new QueryObserver(active.queryClient, options);
    const stop = observer.subscribe(() => {});
    let acknowledge!: (value: NoteEditorSnapshot) => void;
    const saving = active.saveAndConfirm(
      options.queryKey,
      () =>
        new Promise<NoteEditorSnapshot>((done) => {
          acknowledge = done;
        }),
      (result) => active.confirmNoteEditor("note", result),
    );
    emit({
      kind: "change",
      change: { kind: "index_changed", entity: "tasks", ids: ["note"] },
    });
    await vi.waitFor(() =>
      expect(observer.getCurrentResult().data).toEqual(newer),
    );
    acknowledge(saved);
    await saving;
    await vi.waitFor(() =>
      expect(observer.getCurrentResult().data).toEqual(newer),
    );
    expect(backend.readNoteEditor).toHaveBeenCalledTimes(2);
    expect(active.queryClient.getQueryState(tasksKey)?.isInvalidated).toBe(
      true,
    );
    expect(active.queryClient.getQueryData(otherKey)).toEqual(initial);
    expect(active.queryClient.getQueryState(otherKey)?.isInvalidated).toBe(
      false,
    );
    stop();
    active.dispose();
    await active.confirmNoteEditor("note", saved);
    expect(active.queryClient.getQueryData(options.queryKey)).toBeUndefined();
  });

  it("invalidates coherent editor snapshots for matching note or task events", async () => {
    const { active, emit } = fixture("scope");
    await active.start();
    const first = active.noteEditorOptions("first").queryKey;
    const second = active.noteEditorOptions("second").queryKey;
    const outside = ["vault", "other", "note_editor", "first"];
    const snapshot = {
      note: { markdown: null, title: "", revision: "note" },
      tasks: { tasks: [], revision: "tasks" },
    };
    for (const key of [first, second, outside])
      active.queryClient.setQueryData(key, snapshot);
    emit({
      kind: "change",
      change: {
        kind: "index_changed",
        entity: "session_headers",
        ids: ["first"],
      },
    });
    expect(active.queryClient.getQueryState(first)?.isInvalidated).toBe(false);
    emit({
      kind: "change",
      change: { kind: "index_changed", entity: "tasks", ids: ["first"] },
    });
    expect(active.queryClient.getQueryState(first)?.isInvalidated).toBe(true);
    expect(active.queryClient.getQueryState(second)?.isInvalidated).toBe(false);
    expect(active.queryClient.getQueryState(outside)?.isInvalidated).toBe(
      false,
    );
    active.queryClient.setQueryData(first, snapshot);
    emit({
      kind: "change",
      change: { kind: "index_changed", entity: "sessions", ids: ["first"] },
    });
    expect(active.queryClient.getQueryState(first)?.isInvalidated).toBe(true);
    active.dispose();
  });

  it("refreshes a newer canonical note again when its change and read precede an older save acknowledgement", async () => {
    const { active, backend, emit } = fixture("scope");
    await active.start();
    const options = active.noteOptions("note");
    const older = {
      markdown: "My committed text",
      title: "Title",
      revision: "b",
    };
    const newer = {
      markdown: "Another client's newer text",
      title: "Title",
      revision: "c",
    };
    active.queryClient.setQueryData(options.queryKey, {
      markdown: "Original",
      title: "Title",
      revision: "a",
    });
    const otherKey = active.noteOptions("other").queryKey;
    active.queryClient.setQueryData(otherKey, older);
    backend.readNote.mockResolvedValue(newer);
    const observer = new QueryObserver(active.queryClient, options);
    const stop = observer.subscribe(() => {});
    let acknowledge!: (value: NoteSnapshot) => void;
    const saving = active.saveAndConfirm(
      options.queryKey,
      () =>
        new Promise<NoteSnapshot>((resolve) => {
          acknowledge = resolve;
        }),
      (value) => active.confirmNote("note", value),
    );
    emit({
      kind: "change",
      change: { kind: "index_changed", entity: "sessions", ids: ["note"] },
    });
    await vi.waitFor(() =>
      expect(observer.getCurrentResult().data).toEqual(newer),
    );
    acknowledge(older);
    await saving;
    await vi.waitFor(() =>
      expect(observer.getCurrentResult().data).toEqual(newer),
    );
    expect(backend.readNote).toHaveBeenCalledTimes(2);
    expect(active.queryClient.getQueryData(otherKey)).toEqual(older);
    expect(active.queryClient.getQueryState(otherKey)?.isInvalidated).toBe(
      false,
    );
    stop();
    active.dispose();
  });

  it("retains a refresh started during a save even before that read or a change event completes", async () => {
    const { active, backend } = fixture("scope");
    const stopped = watchCacheCleanup(active);
    await active.start();
    const options = active.noteOptions("note");
    const saved = { markdown: "Saved", title: "Title", revision: "b" };
    active.queryClient.setQueryData(options.queryKey, {
      markdown: "Original",
      title: "Title",
      revision: "a",
    });
    let acknowledge!: (value: NoteSnapshot) => void;
    const saving = active.saveAndConfirm(
      options.queryKey,
      () =>
        new Promise<NoteSnapshot>((resolve) => {
          acknowledge = resolve;
        }),
      (value) => active.confirmNote("note", value),
    );
    let finishRead!: (value: NoteSnapshot) => void;
    backend.readNote.mockReturnValueOnce(
      new Promise((resolve) => {
        finishRead = resolve;
      }),
    );
    const reading = active.queryClient
      .fetchQuery({ ...options, staleTime: 0 })
      .catch(() => {});
    acknowledge(saved);
    await saving;
    finishRead({
      markdown: "Another client's newer text",
      title: "Title",
      revision: "c",
    });
    await reading;
    expect(active.queryClient.getQueryData(options.queryKey)).toEqual(saved);
    expect(
      active.queryClient.getQueryState(options.queryKey)?.isInvalidated,
    ).toBe(true);
    expect(stopped).toHaveBeenCalledOnce();
    active.dispose();
    expect(active.queryClient.getQueryCache().hasListeners()).toBe(false);
  });

  it("does not add a read for an ordinary acknowledgement without an intervening change", async () => {
    const { active, backend } = fixture("scope");
    const stopped = watchCacheCleanup(active);
    await active.start();
    const options = active.noteOptions("note");
    const saved = { markdown: "Saved", title: "Title", revision: "b" };
    active.queryClient.setQueryData(options.queryKey, {
      markdown: "Original",
      title: "Title",
      revision: "a",
    });
    await active.saveAndConfirm(
      options.queryKey,
      async () => saved,
      (value) => active.confirmNote("note", value),
    );
    expect(active.queryClient.getQueryData(options.queryKey)).toEqual(saved);
    expect(
      active.queryClient.getQueryState(options.queryKey)?.isInvalidated,
    ).toBe(false);
    expect(backend.readNote).not.toHaveBeenCalled();
    expect(stopped).toHaveBeenCalledOnce();
    active.dispose();
    expect(active.queryClient.getQueryCache().hasListeners()).toBe(false);
  });

  it("preserves output invalidation through a delayed reply and unsubscribes after failure or authorization loss", async () => {
    const { active, emit } = fixture("scope");
    const stopped = watchCacheCleanup(active);
    await active.start();
    const target = { kind: "summary" as const };
    const options = active.documentTextOptions("note", target);
    const saved = { markdown: "Summary", title: "", revision: "b" };
    active.queryClient.setQueryData(options.queryKey, saved);
    let acknowledge!: (value: NoteSnapshot) => void;
    const saving = active.saveAndConfirm(
      options.queryKey,
      () =>
        new Promise<NoteSnapshot>((resolve) => {
          acknowledge = resolve;
        }),
      (value) => active.confirmDocumentText("note", target, value),
    );
    emit({
      kind: "change",
      change: { kind: "index_changed", entity: "docs", ids: ["note"] },
    });
    acknowledge(saved);
    await saving;
    expect(
      active.queryClient.getQueryState(options.queryKey)?.isInvalidated,
    ).toBe(true);
    expect(stopped).toHaveBeenCalledTimes(1);
    const failure = new Error("The connection stopped");
    await expect(
      active.saveAndConfirm(
        options.queryKey,
        async () => {
          throw failure;
        },
        vi.fn(),
      ),
    ).rejects.toBe(failure);
    expect(stopped).toHaveBeenCalledTimes(2);
    let finish!: (value: NoteSnapshot) => void;
    const lost = active.saveAndConfirm(
      options.queryKey,
      () =>
        new Promise<NoteSnapshot>((resolve) => {
          finish = resolve;
        }),
      (value) => active.confirmDocumentText("note", target, value),
    );
    emit({ kind: "connection", state: "access_denied", error: null });
    finish(saved);
    await lost;
    expect(active.queryClient.getQueryCache().getAll()).toHaveLength(0);
    expect(stopped).toHaveBeenCalledTimes(3);
    active.dispose();
    expect(active.queryClient.getQueryCache().hasListeners()).toBe(false);
  });

  it("invalidates checked task snapshots for the affected session without touching another session or vault", async () => {
    const { active, emit } = fixture("scope");
    await active.start();
    const first = active.tasksSnapshotOptions("first").queryKey;
    const second = active.tasksSnapshotOptions("second").queryKey;
    const outside = ["vault", "outside", "tasks_snapshot", "first"];
    for (const key of [first, second, outside])
      active.queryClient.setQueryData(key, { tasks: [], revision: "r" });
    emit({
      kind: "change",
      change: { kind: "index_changed", entity: "tasks", ids: ["first"] },
    });
    expect(active.queryClient.getQueryState(first)?.isInvalidated).toBe(true);
    expect(active.queryClient.getQueryState(second)?.isInvalidated).toBe(false);
    expect(active.queryClient.getQueryState(outside)?.isInvalidated).toBe(
      false,
    );
    active.dispose();
  });

  it("invalidates metadata for the changed session without invalidating another session or vault", async () => {
    const { active, emit } = fixture("scope");
    await active.start();
    const first = active.metadataOptions("first").queryKey;
    const second = active.metadataOptions("second").queryKey;
    const outside = ["vault", "other-scope", "metadata", "first"];
    for (const key of [first, second, outside])
      active.queryClient.setQueryData(key, {
        meta: { title: "Title" },
        revision: "r",
      });
    emit({
      kind: "change",
      change: { kind: "index_changed", entity: "sessions", ids: ["first"] },
    });
    expect(active.queryClient.getQueryState(first)?.isInvalidated).toBe(true);
    expect(active.queryClient.getQueryState(second)?.isInvalidated).toBe(false);
    expect(active.queryClient.getQueryState(outside)?.isInvalidated).toBe(
      false,
    );
    active.dispose();
  });

  it("keeps output snapshots isolated and invalidates only changed sessions", async () => {
    const { active, emit } = fixture("scope");
    await active.start();
    const summary = active.documentOptions("first", {
      kind: "summary",
    }).queryKey;
    const output = active.documentOptions("first", {
      kind: "template_output",
      document_id: "out",
    }).queryKey;
    const text = active.documentTextOptions("first", {
      kind: "summary",
    }).queryKey;
    const other = active.documentOptions("second", {
      kind: "summary",
    }).queryKey;
    const value = { document: null, revision: "r" };
    for (const key of [summary, output, other])
      active.queryClient.setQueryData(key, value);
    active.queryClient.setQueryData(text, {
      markdown: "Summary",
      title: "",
      revision: "r",
    });
    emit({
      kind: "change",
      change: { kind: "index_changed", entity: "docs", ids: ["first"] },
    });
    expect(active.queryClient.getQueryState(summary)?.isInvalidated).toBe(true);
    expect(active.queryClient.getQueryState(output)?.isInvalidated).toBe(true);
    expect(active.queryClient.getQueryState(text)?.isInvalidated).toBe(true);
    expect(active.queryClient.getQueryState(other)?.isInvalidated).toBe(false);
    active.dispose();
  });
  it("does not let an older output read replace a confirmed save and invalidates its task projection", async () => {
    const { active, backend } = fixture("scope");
    await active.start();
    let finish!: (value: unknown) => void;
    backend.readDocument = vi.fn().mockReturnValue(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    const target = { kind: "summary" as const };
    const options = active.documentTextOptions("first", target);
    const pending = active.queryClient.fetchQuery(options).catch(() => {});
    const tasks = active.tasksOptions("first").queryKey;
    active.queryClient.setQueryData(tasks, []);
    const saved = { markdown: "Saved summary", title: "", revision: "b" };
    await active.confirmDocumentText("first", target, saved);
    finish({
      document: {
        id: "first",
        session_id: "first",
        kind: "summary",
        title: "Summary",
        template_id: "",
        sort_order: 0,
        markdown: "Old summary",
      },
      revision: "a",
    });
    await pending;
    expect(active.queryClient.getQueryData(options.queryKey)).toEqual(saved);
    expect(active.queryClient.getQueryState(tasks)?.isInvalidated).toBe(true);
    active.dispose();
  });
  it("invalidates complete registry lists for individual people and tag changes", async () => {
    const { active, emit } = fixture("scope");
    await active.start();
    const people = active.peopleOptions().queryKey;
    const tags = active.tagsOptions().queryKey;
    active.queryClient.setQueryData(people, [{ id: "alice", name: "Alice" }]);
    active.queryClient.setQueryData(tags, [{ id: "project", name: "Project" }]);
    emit({
      kind: "change",
      change: { kind: "index_changed", entity: "people", ids: ["alice"] },
    });
    expect(active.queryClient.getQueryState(people)?.isInvalidated).toBe(true);
    expect(active.queryClient.getQueryState(tags)?.isInvalidated).toBe(false);
    emit({
      kind: "change",
      change: { kind: "index_changed", entity: "tags", ids: ["project"] },
    });
    expect(active.queryClient.getQueryState(tags)?.isInvalidated).toBe(true);
    active.dispose();
  });
  it("never shares notes for identical session IDs across account or vault scopes", async () => {
    const alice = fixture("server:alice:private");
    const bob = fixture("server:bob:private");
    await alice.active.start();
    await bob.active.start();
    const first = await alice.active.queryClient.fetchQuery(
      alice.active.noteOptions("same"),
    );
    const second = await bob.active.queryClient.fetchQuery(
      bob.active.noteOptions("same"),
    );
    expect(first.markdown).toBe("server:alice:private");
    expect(second.markdown).toBe("server:bob:private");
    alice.active.dispose();
    expect(alice.active.queryClient.getQueryCache().getAll()).toHaveLength(0);
    expect(
      bob.active.queryClient.getQueryData(
        bob.active.noteOptions("same").queryKey,
      ),
    ).toEqual(second);
    bob.active.dispose();
  });

  it("invalidates only affected notes and waits for search readiness to invalidate search", async () => {
    const { active, emit } = fixture("scope");
    await active.start();
    const first = active.noteOptions("first");
    const second = active.noteOptions("second");
    active.queryClient.setQueryData(first.queryKey, {
      markdown: "first",
      title: "Title",
      revision: "a",
    });
    active.queryClient.setQueryData(second.queryKey, {
      markdown: "second",
      title: "Title",
      revision: "b",
    });
    const search = ["vault", "scope", "search", "query"];
    active.queryClient.setQueryData(search, { hits: [], count: 0 });
    emit({
      kind: "change",
      change: { kind: "index_changed", entity: "sessions", ids: ["first"] },
    });
    expect(
      active.queryClient.getQueryState(first.queryKey)?.isInvalidated,
    ).toBe(true);
    expect(
      active.queryClient.getQueryState(second.queryKey)?.isInvalidated,
    ).toBe(false);
    expect(active.queryClient.getQueryState(search)?.isInvalidated).toBe(false);
    emit({ kind: "change", change: { kind: "search_ready" } });
    expect(active.queryClient.getQueryState(search)?.isInvalidated).toBe(true);
    active.dispose();
  });

  it("retains cached display data during a disconnect and revalidates on reconnect", async () => {
    const { active, emit } = fixture("scope");
    await active.start();
    const key = active.noteOptions("note").queryKey;
    active.queryClient.setQueryData(key, {
      markdown: "cached",
      title: "Title",
      revision: "a",
    });
    emit({ kind: "connection", state: "reconnecting", error: null });
    expect(active.getSnapshot().state).toBe("reconnecting");
    expect(active.queryClient.getQueryData(key)).toBeDefined();
    emit({ kind: "connection", state: "connected", error: null });
    expect(active.queryClient.getQueryState(key)?.isInvalidated).toBe(true);
    active.dispose();
  });

  it("removes private cached content after authorization is lost", async () => {
    const { active, emit } = fixture("scope");
    await active.start();
    active.queryClient.setQueryData(active.noteOptions("note").queryKey, {
      markdown: "private",
      title: "Title",
      revision: "a",
    });
    emit({ kind: "connection", state: "access_denied", error: null });
    expect(active.queryClient.getQueryCache().getAll()).toHaveLength(0);
    expect(active.getSnapshot().state).toBe("access_denied");
    active.dispose();
  });

  it("stops a subscription whose admission finishes after the scope was closed", async () => {
    const { active, backend, unsubscribe } = fixture("scope");
    let admit!: (value: () => void) => void;
    vi.mocked(backend.subscribe).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          admit = resolve;
        }),
    );
    const starting = active.start();
    active.dispose();
    admit(unsubscribe);
    await starting;
    expect(unsubscribe).toHaveBeenCalledOnce();
    expect(active.getSnapshot().state).toBe("closed");
  });

  it("aborts an old scope's reads and ignores late events without touching another scope", async () => {
    const first = fixture("first");
    const second = fixture("second");
    await first.active.start();
    await second.active.start();
    let finish!: (value: NoteSnapshot) => void;
    let signal!: AbortSignal;
    first.backend.readNote.mockImplementationOnce((_id, options) => {
      signal = options.signal;
      return new Promise((resolve) => {
        finish = resolve;
      });
    });
    const loading = first.active.queryClient.fetchQuery(
      first.active.noteOptions("same"),
    );
    const settling = Promise.allSettled([loading]);
    first.active.dispose();
    expect(signal.aborted).toBe(true);
    finish({ markdown: "old response", title: "Title", revision: "a" });
    await settling;
    first.emit({ kind: "change", change: { kind: "resync_required" } });
    expect(first.active.queryClient.getQueryCache().getAll()).toHaveLength(0);
    expect(second.active.getSnapshot().state).toBe("connected");
    second.active.dispose();
  });
  it("revalidates on window focus and detaches lifecycle listeners on disposal", async () => {
    const { active } = fixture("scope");
    await active.start();
    const key = active.noteOptions("note").queryKey;
    active.queryClient.setQueryData(key, {
      markdown: "note",
      title: "Title",
      revision: "a",
    });
    window.dispatchEvent(new Event("focus"));
    expect(active.queryClient.getQueryState(key)?.isInvalidated).toBe(true);
    active.dispose();
    const invalidations = vi.spyOn(active.queryClient, "invalidateQueries");
    window.dispatchEvent(new Event("focus"));
    expect(invalidations).not.toHaveBeenCalled();
  });
  it("does not recreate private cached content from a late save confirmation", async () => {
    const { active, emit } = fixture("scope");
    await active.start();
    emit({ kind: "connection", state: "access_denied", error: null });
    await active.confirmNote("note", {
      markdown: "private",
      title: "Title",
      revision: "b",
    });
    expect(active.queryClient.getQueryCache().getAll()).toHaveLength(0);
    active.dispose();
    await active.confirmNote("note", {
      markdown: "late",
      title: "Title",
      revision: "c",
    });
    expect(active.queryClient.getQueryCache().getAll()).toHaveLength(0);
  });

  it("clears cached private data when a request discovers lost authorization", async () => {
    const { active } = fixture("scope");
    await active.start();
    active.queryClient.setQueryData(active.noteOptions("note").queryKey, {
      markdown: "private",
      title: "Title",
      revision: "a",
    });
    await expect(
      active.run(async () => {
        throw new ApplicationRequestError({
          code: "forbidden",
          message: "Access revoked",
          retryable: false,
          request_id: null,
          current_revision: null,
          mutation_outcome: "not_committed",
        });
      }),
    ).rejects.toThrow("Access revoked");
    expect(active.getSnapshot().state).toBe("access_denied");
    expect(active.queryClient.getQueryCache().getAll()).toHaveLength(0);
    active.dispose();
  });
});
