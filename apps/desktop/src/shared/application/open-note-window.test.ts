import { expect, it, vi } from "vitest";

import { openScopedNoteWindow } from "./open-note-window";
import type { VaultScope } from "./vault-scope";
it("retires only an in-flight publication when its original workspace closes", async () => {
  let state = "connected";
  let changed!: () => void;
  let finish!: () => void;
  const release = vi.fn();
  const open = vi.fn(
    () =>
      new Promise<void>((resolve) => {
        finish = resolve;
      }),
  );
  const scope = {
    client: { openNoteWindow: open },
    getSnapshot: () => ({ state }),
    subscribe: (listener: () => void) => {
      changed = listener;
      return release;
    },
    run: (work: () => unknown) => work(),
  } as unknown as VaultScope;
  const work = openScopedNoteWindow(scope, "original");
  expect(open.mock.calls[0]).toEqual([
    "original",
    expect.objectContaining({
      signal: expect.any(AbortSignal),
      requestId: expect.any(String),
    }),
  ]);
  const signal = (open.mock.calls[0] as any)[1].signal;
  state = "closed";
  changed();
  expect(signal.aborted).toBe(true);
  finish();
  await work;
  expect(release).toHaveBeenCalledOnce();
  await expect(openScopedNoteWindow(scope, "other")).rejects.toThrow(
    "Reconnect",
  );
  expect(open).toHaveBeenCalledOnce();
});
