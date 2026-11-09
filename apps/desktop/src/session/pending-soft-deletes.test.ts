import { expect, it } from "vitest";

import {
  trackPendingSoftDelete,
  waitForPendingSoftDelete,
} from "./pending-soft-deletes";

it("never waits for another vault's same-ID delete", async () => {
  let release!: () => void;
  const write = new Promise<void>((resolve) => {
    release = resolve;
  });
  trackPendingSoftDelete("first", "same-id", write);
  let first = false;
  void waitForPendingSoftDelete("first", "same-id").then(() => {
    first = true;
  });
  await waitForPendingSoftDelete("second", "same-id");
  expect(first).toBe(false);
  release();
  await waitForPendingSoftDelete("first", "same-id");
  expect(first).toBe(true);
});
it("keeps a replacement delete owned when an older write settles", async () => {
  let oldRelease!: () => void, nextRelease!: () => void;
  const old = new Promise<void>((resolve) => {
      oldRelease = resolve;
    }),
    next = new Promise<void>((resolve) => {
      nextRelease = resolve;
    });
  trackPendingSoftDelete("first", "same-id", old);
  const oldWait = waitForPendingSoftDelete("first", "same-id");
  trackPendingSoftDelete("first", "same-id", next);
  const nextWait = waitForPendingSoftDelete("first", "same-id");
  oldRelease();
  await oldWait;
  expect(waitForPendingSoftDelete("first", "same-id")).toBe(nextWait);
  nextRelease();
  await nextWait;
});
