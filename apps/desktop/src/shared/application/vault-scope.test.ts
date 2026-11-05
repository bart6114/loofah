import { describe, expect, it, vi } from "vitest";

import { ApplicationRequestError } from "./errors";
import type { NoteSnapshot } from "./protocol.gen";
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
    session: vi.fn(),
    sessions: vi.fn(),
    search: vi.fn(),
    subscribe: vi.fn(async (listener) => {
      event = listener;
      listener({ kind: "connection", state: "connected", error: null });
      return unsubscribe;
    }),
  } as unknown as VaultClient & { readNote: ReturnType<typeof vi.fn> };
  const active = new VaultScope(backend);
  return {
    backend,
    active,
    unsubscribe,
    emit: (value: VaultClientEvent) => event(value),
  };
}

describe("vault-scoped query lifecycle", () => {
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
