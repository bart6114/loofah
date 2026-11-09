import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import {
  confirmAllPendingDeletions,
  deletionKey,
  pendingDeletionsForScope,
  UNDO_TIMEOUT_MS,
  useScopedPendingDeletions,
  useUndoDelete,
} from "./undo-delete";

const data = (title = "Note") => ({
  session: { id: "same-id", title },
  tombstone: "t",
  deletedAt: 1,
});
const restore = vi.fn(async (_assert: () => void) => {});
beforeEach(() => {
  vi.useFakeTimers();
  useUndoDelete.setState({ pendingDeletions: {} });
  restore.mockClear();
});
afterEach(() => {
  cleanup();
  for (const entry of Object.values(useUndoDelete.getState().pendingDeletions))
    if (entry.timeoutId) clearTimeout(entry.timeoutId);
  useUndoDelete.setState({ pendingDeletions: {} });
  vi.useRealTimers();
});

it("uses collision-free scope/session tuples and preserves another vault's same-ID entry", () => {
  expect(deletionKey("a:b", "c")).not.toBe(deletionKey("a", "b:c"));
  expect(deletionKey('a"', "b")).not.toBe(deletionKey("a", '"b'));
  const store = useUndoDelete.getState();
  store.addDeletion("first", data("Private"), { restore });
  store.addDeletion("second", data("Shared"), { restore });
  expect(
    pendingDeletionsForScope(
      useUndoDelete.getState().pendingDeletions,
      "first",
    )["same-id"].data.session.title,
  ).toBe("Private");
  store.clearDeletion("first", "same-id");
  expect(
    Object.values(useUndoDelete.getState().pendingDeletions).map(
      (entry) => entry.scope,
    ),
  ).toEqual(["second"]);
});
it("keeps equal batch IDs isolated across scopes", () => {
  const first = vi.fn(),
    second = vi.fn();
  const store = useUndoDelete.getState();
  store.addDeletion("first", data(), {
    restore,
    batchId: "same-batch",
    onConfirm: first,
  });
  store.addDeletion("second", data(), {
    restore,
    batchId: "same-batch",
    onConfirm: second,
  });
  store.confirmBatch("first", "same-batch");
  expect(first).toHaveBeenCalledOnce();
  expect(second).not.toHaveBeenCalled();
  expect(
    Object.values(useUndoDelete.getState().pendingDeletions).map(
      (entry) => entry.scope,
    ),
  ).toEqual(["second"]);
  store.clearBatch("second", "same-batch");
  expect(useUndoDelete.getState().pendingDeletions).toEqual({});
});
it("preserves the five-second confirmation timing without performing restores", async () => {
  const confirm = vi.fn();
  useUndoDelete
    .getState()
    .addDeletion("first", data(), { restore, onConfirm: confirm });
  await vi.advanceTimersByTimeAsync(UNDO_TIMEOUT_MS - 1);
  expect(confirm).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  expect(confirm).toHaveBeenCalledOnce();
  expect(restore).not.toHaveBeenCalled();
  expect(useUndoDelete.getState().pendingDeletions).toEqual({});
});
it("does not let an old confirmation clear a replacement entry", () => {
  const next = data("New");
  const store = useUndoDelete.getState();
  store.addDeletion("first", data("Old"), {
    restore,
    onConfirm: () => store.addDeletion("first", next, { restore }),
  });
  void store.confirmDeletion("first", "same-id");
  expect(
    useUndoDelete.getState().pendingDeletions[deletionKey("first", "same-id")]
      .data.session.title,
  ).toBe("New");
});
it("confirms all original namespaces on exit, awaits accepted work and tolerates another callback failing", async () => {
  let release!: () => void;
  const original = vi.fn(
    () =>
      new Promise<void>((resolve) => {
        release = resolve;
      }),
  );
  const failure = vi.fn(() => {
    throw new Error("failed");
  });
  const store = useUndoDelete.getState();
  store.addDeletion("first", data(), { restore, onConfirm: original });
  store.addDeletion("second", data(), { restore, onConfirm: failure });
  let completed = false;
  const exiting = confirmAllPendingDeletions().then(() => {
    completed = true;
  });
  await Promise.resolve();
  expect(original).toHaveBeenCalledOnce();
  expect(failure).toHaveBeenCalledOnce();
  expect(completed).toBe(false);
  release();
  await exiting;
  expect(useUndoDelete.getState().pendingDeletions).toEqual({});
});
it("does not rerender a scoped selector for another vault's tombstone", () => {
  let renders = 0;
  const { result } = renderHook(() => {
    renders++;
    return useScopedPendingDeletions("first");
  });
  const before = renders;
  act(() =>
    useUndoDelete.getState().addDeletion("second", data(), { restore }),
  );
  expect(renders).toBe(before);
  expect(result.current).toEqual({});
  act(() => useUndoDelete.getState().addDeletion("first", data(), { restore }));
  expect(Object.keys(result.current)).toEqual(["same-id"]);
});

it("isolates undo visibility by exact owner when the replacement uses the same scope string", () => {
  const original = {},
    replacement = {};
  useUndoDelete
    .getState()
    .addDeletion("same-scope", data(), { owner: original, restore });
  const entries = useUndoDelete.getState().pendingDeletions;
  expect(
    Object.keys(pendingDeletionsForScope(entries, "same-scope", original)),
  ).toEqual(["same-id"]);
  expect(pendingDeletionsForScope(entries, "same-scope", replacement)).toEqual(
    {},
  );
});
