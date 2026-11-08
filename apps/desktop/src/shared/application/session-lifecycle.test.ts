import { onlineManager } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";

import { ApplicationRequestError } from "./errors";
import type {
  DeleteSessionOutcome,
  SessionLifecycleSnapshot,
} from "./protocol.gen";
import {
  captureSessionDeletion,
  createSessionLifecycleQueue,
  sessionDeletionAvailable,
} from "./session-lifecycle";
import type { VaultClient, VaultClientEvent } from "./vault-client";
import { VaultScope } from "./vault-scope";

import { createTabsStore } from "~/store/zustand/tabs/store";

const scopes: VaultScope[] = [];
afterEach(() => {
  for (const scope of scopes.splice(0)) scope.dispose();
  onlineManager.setOnline(true);
});
const revision = "a".repeat(64);
const token = "b".repeat(64);
const deleted: DeleteSessionOutcome = {
  kind: "trashed",
  trash_token: token,
  revision,
};
function error(
  outcome: "unknown" | "not_committed" = "unknown",
  code: "unavailable" | "not_found" = "unavailable",
) {
  return new ApplicationRequestError({
    code,
    message: "Request failed",
    request_id: null,
    retryable: false,
    current_revision: null,
    mutation_outcome: outcome,
  });
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (value: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
async function fixture(
  networkMode: "always" | "online" = "always",
  scopeId = "same-scope",
) {
  let receive!: (event: VaultClientEvent) => void;
  const read = vi
    .fn<VaultClient["readSessionLifecycle"]>()
    .mockImplementation(async (session_id) => ({ session_id, revision }));
  const remove = vi
    .fn<VaultClient["deleteSession"]>()
    .mockResolvedValue(deleted);
  const restore = vi
    .fn<VaultClient["restoreSession"]>()
    .mockResolvedValue({ kind: "restored", session_id: "same-id", revision });
  const access = { canEdit: true, features: ["session_lifecycle"] };
  const client = {
    scope: scopeId,
    networkMode,
    get canEdit() {
      return access.canEdit;
    },
    get features() {
      return access.features;
    },
    readSessionLifecycle: read,
    deleteSession: remove,
    restoreSession: restore,
    subscribe: async (listener: (event: VaultClientEvent) => void) => {
      receive = listener;
      listener({ kind: "connection", state: "connected", error: null });
      return () => {};
    },
  } as unknown as VaultClient;
  const scope = new VaultScope(client);
  scopes.push(scope);
  await scope.start();
  const store = createTabsStore(
    scopeId,
    () => scope.getSnapshot().state !== "closed",
  );
  store.getState().openCurrent({ type: "sessions", id: "same-id" });
  return {
    scope,
    store,
    client,
    access,
    read,
    remove,
    restore,
    emit: (state: "reconnecting" | "access_denied" | "login_required") =>
      receive({ kind: "connection", state, error: null }),
  };
}
it.each(["always", "online"] as const)(
  "pins the original revision and UUID through unknown replies in %s mode",
  async (mode) => {
    const f = await fixture(mode);
    const intent = captureSessionDeletion(f.scope, f.store, "same-id");
    f.remove
      .mockRejectedValueOnce(error())
      .mockRejectedValueOnce(error("not_committed"));
    await expect(intent.submit()).rejects.toThrow();
    expect(intent.uncertain()).toBe(true);
    f.read.mockResolvedValue({
      session_id: "same-id",
      revision: "c".repeat(64),
    });
    await expect(intent.submit()).rejects.toThrow();
    expect(intent.uncertain()).toBe(true);
    await expect(intent.submit()).resolves.toEqual(deleted);
    expect(intent.uncertain()).toBe(false);
    expect(f.read).toHaveBeenCalledTimes(1);
    for (const call of f.remove.mock.calls) {
      expect(call[0]).toEqual({
        session_id: "same-id",
        expected_revision: revision,
      });
      expect(call[1]?.requestId).toBe(intent.requestId);
    }
    intent.release();
  },
);
it("keeps default Local offline admission and refuses Remote offline instead of queuing edits", async () => {
  const local = await fixture("always");
  const remote = await fixture("online");
  onlineManager.setOnline(false);
  expect(sessionDeletionAvailable(local.scope, local.store)).toBe(true);
  expect(sessionDeletionAvailable(remote.scope, remote.store)).toBe(false);
  await expect(
    captureSessionDeletion(local.scope, local.store, "same-id").submit(),
  ).resolves.toEqual(deleted);
  await expect(
    captureSessionDeletion(remote.scope, remote.store, "same-id").submit(),
  ).rejects.toThrow();
  expect(remote.read).not.toHaveBeenCalled();
  expect(remote.remove).not.toHaveBeenCalled();
});
it("refuses reader, unsupported and disconnected owners before reads or writes", async () => {
  for (const reason of ["reader", "unsupported", "disconnected"]) {
    const f = await fixture();
    if (reason === "reader") f.access.canEdit = false;
    if (reason === "unsupported") f.access.features = [];
    if (reason === "disconnected") f.emit("reconnecting");
    await expect(
      captureSessionDeletion(f.scope, f.store, "same-id").submit(),
    ).rejects.toThrow();
    expect(f.read).not.toHaveBeenCalled();
    expect(f.remove).not.toHaveBeenCalled();
  }
});
it.each([50, 150, 500])(
  "a late reply at %sms cannot enter a same-string replacement Scope or tabs owner",
  async (delay) => {
    const old = await fixture();
    const next = await fixture();
    const pending = deferred<DeleteSessionOutcome>();
    old.remove.mockReturnValue(pending.promise);
    const intent = captureSessionDeletion(old.scope, old.store, "same-id");
    intent.hide();
    const result = intent.submit();
    await vi.waitFor(() => expect(old.remove).toHaveBeenCalledTimes(1));
    old.scope.dispose();
    await new Promise((resolve) => setTimeout(resolve, delay));
    pending.resolve(deleted);
    await expect(result).rejects.toThrow();
    expect(intent.uncertain()).toBe(true);
    intent.rollback();
    expect(next.store.getState().currentTab).toMatchObject({
      type: "sessions",
      id: "same-id",
    });
    expect(next.remove).not.toHaveBeenCalled();
    expect(next.scope.queryClient.getQueryCache().getAll()).toHaveLength(0);
  },
);
it("reopening while a baseline is pending cancels before dispatch and preserves the new owner", async () => {
  const f = await fixture();
  const baseline = deferred<SessionLifecycleSnapshot>();
  f.read.mockReturnValue(baseline.promise);
  const intent = captureSessionDeletion(f.scope, f.store, "same-id");
  intent.hide();
  const result = intent.submit();
  await vi.waitFor(() => expect(f.read).toHaveBeenCalled());
  f.store.getState().openCurrent({ type: "sessions", id: "same-id" });
  baseline.resolve({ session_id: "same-id", revision });
  await expect(result).rejects.toThrow();
  expect(f.remove).not.toHaveBeenCalled();
  expect(intent.canCloseWindows()).toBe(false);
  intent.rollback();
  expect(f.store.getState().currentTab).toMatchObject({ id: "same-id" });
});
it("late failure cannot replace an unrelated tab selected after optimistic close", async () => {
  const f = await fixture();
  const intent = captureSessionDeletion(f.scope, f.store, "same-id");
  intent.hide();
  f.store.getState().openCurrent({ type: "sessions", id: "other" });
  intent.rollback();
  expect(f.store.getState().currentTab).toMatchObject({ id: "other" });
});
it("undo before dispatch cancels preparation without creating either mutation", async () => {
  const f = await fixture();
  const baseline = deferred<SessionLifecycleSnapshot>();
  f.read.mockReturnValue(baseline.promise);
  const intent = captureSessionDeletion(f.scope, f.store, "same-id");
  const pending = intent.submit();
  await vi.waitFor(() => expect(f.read).toHaveBeenCalled());
  const restore = intent.restore();
  baseline.resolve({ session_id: "same-id", revision });
  await expect(pending).rejects.toThrow();
  await restore;
  expect(f.remove).not.toHaveBeenCalled();
  expect(f.restore).not.toHaveBeenCalled();
});
it("undo awaits an admitted delete and preserves the original restore UUID after a lost reply", async () => {
  const f = await fixture();
  const pending = deferred<DeleteSessionOutcome>();
  f.remove.mockReturnValue(pending.promise);
  const intent = captureSessionDeletion(f.scope, f.store, "same-id");
  const deletion = intent.submit();
  await vi.waitFor(() => expect(f.remove).toHaveBeenCalled());
  f.restore.mockRejectedValueOnce(error());
  const undo = intent.restore();
  expect(f.restore).not.toHaveBeenCalled();
  pending.resolve(deleted);
  await deletion;
  await expect(undo).rejects.toThrow();
  await intent.restore();
  expect(f.restore).toHaveBeenCalledTimes(2);
  expect(f.restore.mock.calls[0]).toEqual(f.restore.mock.calls[1]);
  expect(f.restore.mock.calls[0][0]).toEqual({
    trash_token: token,
    expected_revision: revision,
  });
});
it("historical success requires a fresh absence proof and never seeds canonical data", async () => {
  const f = await fixture();
  const intent = captureSessionDeletion(f.scope, f.store, "same-id");
  await intent.submit();
  expect(await intent.confirmDeleted()).toBe(false);
  f.read.mockRejectedValueOnce(error("not_committed", "not_found"));
  expect(await intent.confirmDeleted()).toBe(true);
  expect(f.scope.queryClient.getQueryCache().getAll()).toHaveLength(0);
});
it("bounds concurrent operations per captured owner without dropping explicit batch requests", async () => {
  const f = await fixture();
  const queue = createSessionLifecycleQueue();
  const release = deferred<void>();
  let active = 0,
    max = 0;
  f.read.mockImplementation(async (session_id) => {
    active++;
    max = Math.max(max, active);
    await release.promise;
    active--;
    return { session_id, revision };
  });
  const intents = Array.from({ length: 20 }, (_, index) =>
    captureSessionDeletion(f.scope, f.store, `note-${index}`, queue),
  );
  const work = intents.map((intent) => intent.submit());
  await vi.waitFor(() => expect(f.read).toHaveBeenCalledTimes(2));
  release.resolve();
  await Promise.all(work);
  expect(max).toBe(2);
  expect(f.remove).toHaveBeenCalledTimes(20);
  for (const intent of intents) intent.release();
});
