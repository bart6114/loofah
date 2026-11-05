import { beforeEach, describe, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  draftCacheGet: vi.fn(),
  draftCacheInventory: vi.fn(),
  draftCacheRecovery: vi.fn(),
  draftCachePut: vi.fn(),
  draftCacheRemove: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({ commands: native }));

import { NativeDraftStore } from "./draft-store";

const recovery = (scope: string) => ({
  scope,
  sessionId: "same",
  baseline: { markdown: "Original", title: "Title", revision: "a" },
  draft: { markdown: "Unsaved", title: "Title" },
  uncertain: true,
});

beforeEach(() => {
  vi.resetAllMocks();
  native.draftCacheInventory.mockResolvedValue({ status: "ok", data: [] });
  native.draftCacheRecovery.mockResolvedValue({ status: "ok", data: null });
  native.draftCacheGet.mockResolvedValue({ status: "ok", data: null });
  native.draftCachePut.mockResolvedValue({ status: "ok", data: undefined });
  native.draftCacheRemove.mockResolvedValue({ status: "ok", data: undefined });
});

describe("native device draft persistence", () => {
  it("flushes queued device writes before inventory and pins recovery lookups to the original scope", async () => {
    const store = new NativeDraftStore("alice");
    let finish!: (value: unknown) => void;
    native.draftCachePut.mockReturnValueOnce(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    const writing = store.save(recovery("alice"));
    await vi.waitFor(() => expect(native.draftCachePut).toHaveBeenCalledOnce());
    const listing = store.inventory();
    expect(native.draftCacheInventory).not.toHaveBeenCalled();
    finish({ status: "ok", data: undefined });
    await writing;
    await listing;
    expect(native.draftCacheInventory).toHaveBeenCalledWith("alice");
    await store.recovery("opaque-id");
    expect(native.draftCacheRecovery).toHaveBeenCalledWith(
      "alice",
      "opaque-id",
    );
    native.draftCacheRecovery.mockResolvedValueOnce({
      status: "ok",
      data: recovery("bob"),
    });
    await expect(store.recovery("opaque-id")).rejects.toThrow(
      "different account or vault",
    );
  });

  it("serializes writes and cleanup while pinning immutable inputs at enqueue time", async () => {
    const store = new NativeDraftStore("server:alice:vault");
    let finish!: (value: unknown) => void;
    native.draftCachePut.mockReturnValueOnce(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    const draft = recovery(store.scope);
    const first = store.save(draft);
    await vi.waitFor(() => expect(native.draftCachePut).toHaveBeenCalledOnce());
    draft.draft.markdown = "Queued";
    const next = store.save(draft);
    draft.draft.markdown = "Later mutation";
    const cleanup = store.remove("same");
    expect(native.draftCacheRemove).not.toHaveBeenCalled();
    finish({ status: "ok", data: undefined });
    await first;
    await next;
    await cleanup;
    expect(native.draftCachePut.mock.calls[0][0].draft.markdown).toBe(
      "Unsaved",
    );
    expect(native.draftCachePut.mock.calls[1][0].draft.markdown).toBe("Queued");
    expect(native.draftCacheRemove).toHaveBeenCalledWith(store.scope, "same");
  });

  it("keeps account scopes separate while waiting for pending recovery writes on read", async () => {
    const alice = new NativeDraftStore("server:alice:vault");
    const bob = new NativeDraftStore("server:bob:vault");
    let finish!: (value: unknown) => void;
    native.draftCachePut.mockReturnValueOnce(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    const writing = alice.save(recovery(alice.scope));
    await vi.waitFor(() => expect(native.draftCachePut).toHaveBeenCalledOnce());
    const reading = alice.load("same");
    await bob.save(recovery(bob.scope));
    expect(native.draftCacheGet).not.toHaveBeenCalled();
    finish({ status: "ok", data: undefined });
    await writing;
    await reading;
    expect(native.draftCacheGet).toHaveBeenCalledWith(alice.scope, "same");
    await expect(bob.save(recovery(alice.scope))).rejects.toThrow(
      "different account",
    );
  });

  it("reports native storage failures without falling back to another scope", async () => {
    const store = new NativeDraftStore("scope");
    native.draftCachePut.mockResolvedValueOnce({
      status: "error",
      error: {
        code: "resource_limit",
        message: "Recovery storage full",
        request_id: null,
        retryable: false,
        current_revision: null,
        mutation_outcome: "not_committed",
      },
    });
    await expect(store.save(recovery("scope"))).rejects.toThrow(
      "Recovery storage full",
    );
    expect(native.draftCachePut).toHaveBeenCalledOnce();
    expect(native.draftCacheRemove).not.toHaveBeenCalled();
  });
});
