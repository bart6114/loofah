import { useCallback, useRef, useSyncExternalStore } from "react";

import { useVaultHost } from "./vault-provider";
import type { VaultScope } from "./vault-scope";

const terminal = new Set([
  "closed",
  "access_denied",
  "login_required",
  "incompatible",
]);

export function useVaultReadScope() {
  const { scope } = useVaultHost();
  if (!scope) throw new Error("This feature requires an immutable vault host");
  const owner = useRef({
    scope,
    retired: terminal.has(scope.getSnapshot().state),
  }).current;
  if (scope !== owner.scope) owner.retired = true;
  const subscribe = useCallback(
    (changed: () => void) =>
      owner.scope.subscribe(() => {
        if (terminal.has(owner.scope.getSnapshot().state)) owner.retired = true;
        changed();
      }),
    [owner],
  );
  const connection = useSyncExternalStore(subscribe, owner.scope.getSnapshot);
  if (terminal.has(connection.state)) owner.retired = true;
  const current = useCallback(
    () => !owner.retired && !terminal.has(owner.scope.getSnapshot().state),
    [owner],
  );
  const readable =
    current() &&
    (connection.state === "connected" || connection.state === "reconnecting");
  return { scope: owner.scope, connection, readable, current };
}

export function legacyTranscriptControlsAvailable(scope: VaultScope) {
  return (
    scope.getSnapshot().state === "connected" &&
    scope.client.canEdit &&
    scope.client.features.includes("legacy_desktop_transcript_controls")
  );
}
