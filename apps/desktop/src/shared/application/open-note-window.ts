import type { VaultScope } from "./vault-scope";

export async function openScopedNoteWindow(
  scope: VaultScope,
  sessionId: string,
) {
  const open = scope.client.openNoteWindow;
  if (!open || scope.getSnapshot().state !== "connected")
    throw new Error("Reconnect to this workspace before opening its note.");
  const abort = new AbortController();
  const release = scope.subscribe(() => {
    if (scope.getSnapshot().state !== "connected") abort.abort();
  });
  try {
    await scope.run(() =>
      open(sessionId, { signal: abort.signal, requestId: crypto.randomUUID() }),
    );
  } finally {
    release();
  }
}
