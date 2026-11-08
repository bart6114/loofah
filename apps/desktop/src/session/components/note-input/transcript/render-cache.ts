import { CancelledError } from "@tanstack/react-query";
import { useCallback, useRef, useSyncExternalStore } from "react";

import { useVaultHost } from "~/shared/application/vault-provider";
import type { VaultScope } from "~/shared/application/vault-scope";

const terminal = new Set([
  "closed",
  "access_denied",
  "login_required",
  "incompatible",
]);

export function renderedTranscriptPrefix(
  scope: VaultScope,
  sessionId: string,
  transcriptId: string,
) {
  return [
    "vault",
    scope.client.scope,
    "rendered_transcript_segments",
    sessionId,
    transcriptId,
  ] as const;
}

export function transcriptExportKey(
  scope: VaultScope,
  sessionId: string,
  transcriptIds: readonly string[],
  requestHash: string,
) {
  return [
    "vault",
    scope.client.scope,
    "transcript_export_segments",
    sessionId,
    transcriptIds,
    requestHash,
  ] as const;
}

export function useTranscriptCacheOwner() {
  const { scope } = useVaultHost();
  if (!scope)
    throw new Error("Transcript caches require an immutable vault host");
  const owner = useRef({ scope, retired: false }).current;
  if (owner.scope !== scope || terminal.has(owner.scope.getSnapshot().state))
    owner.retired = true;
  const subscribe = useCallback(
    (changed: () => void) =>
      owner.scope.subscribe(() => {
        if (terminal.has(owner.scope.getSnapshot().state)) owner.retired = true;
        changed();
      }),
    [owner],
  );
  useSyncExternalStore(subscribe, owner.scope.getSnapshot);
  const isCurrent = useCallback(() => {
    const state = owner.scope.getSnapshot().state;
    if (terminal.has(state)) owner.retired = true;
    return (
      !owner.retired && (state === "connected" || state === "reconnecting")
    );
  }, [owner]);
  const assertCurrent = useCallback(
    (signal: AbortSignal) => {
      if (signal.aborted || !isCurrent()) throw new CancelledError();
    },
    [isCurrent],
  );
  return {
    scope: owner.scope,
    available: isCurrent(),
    isCurrent,
    assertCurrent,
  };
}
