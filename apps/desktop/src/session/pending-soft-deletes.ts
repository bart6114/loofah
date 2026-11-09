import { deletionKey } from "~/store/zustand/undo-delete";

// In-flight soft-delete writes by captured vault scope and session id. The undo toast appears before
// the tombstone commits, so a restore must wait for the write to settle —
// otherwise "session still alive" can mean "delete not committed yet" and the
// in-flight delete lands after the undo, vanishing the note with no toast.
const pending = new Map<string, Promise<unknown>>();

export function trackPendingSoftDelete(
  scope: string,
  sessionId: string,
  write: Promise<unknown>,
) {
  const key = deletionKey(scope, sessionId);
  const entry = write
    .catch(() => undefined)
    .finally(() => {
      if (pending.get(key) === entry) {
        pending.delete(key);
      }
    });
  pending.set(key, entry);
}

export function waitForPendingSoftDelete(
  scope: string,
  sessionId: string,
): Promise<unknown> {
  return pending.get(deletionKey(scope, sessionId)) ?? Promise.resolve();
}
