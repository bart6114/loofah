import { expect, it, vi } from "vitest";

import { createAsyncListenerScope } from "./async-listener-scope";

it("disposes late registrations exactly once and disables handlers synchronously", async () => {
  const scope = createAsyncListenerScope();
  const cleanup = vi.fn();
  const handler = vi.fn();
  const guarded = scope.guard(handler);
  let resolve!: (cleanup: () => void) => void;
  const pending = scope.add(
    () =>
      new Promise((r) => {
        resolve = r;
      }),
  );
  guarded(null);
  scope.dispose();
  guarded(null);
  resolve(cleanup);
  await pending;
  scope.dispose();
  expect(cleanup).toHaveBeenCalledOnce();
  expect(handler).toHaveBeenCalledOnce();
});

it("handles synchronous registration errors and attempts every cleanup even if one throws", async () => {
  const scope = createAsyncListenerScope();
  const cleanup = vi.fn();
  await scope.add(async () => () => {
    throw new Error("cleanup failure");
  });
  await scope.add(async () => cleanup);
  await expect(
    scope.add(() => {
      throw new Error("setup failure");
    }),
  ).rejects.toThrow("setup failure");
  expect(cleanup).toHaveBeenCalledOnce();
  expect(scope.active).toBe(false);
  scope.dispose();
  expect(cleanup).toHaveBeenCalledOnce();
});

it("owns each registration even when a provider returns the same cleanup function", async () => {
  const scope = createAsyncListenerScope();
  const cleanup = vi.fn();
  await scope.add(async () => cleanup);
  await scope.add(async () => cleanup);
  scope.dispose();
  scope.dispose();
  expect(cleanup).toHaveBeenCalledTimes(2);
});
