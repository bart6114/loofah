import { useQuery } from "@tanstack/react-query";
import { useCallback, useMemo, useRef, useSyncExternalStore } from "react";

import {
  ApplicationRequestError,
  connectionClosed,
} from "~/shared/application/errors";
import { useVaultHost } from "~/shared/application/vault-provider";

const terminal = new Set([
  "closed",
  "access_denied",
  "login_required",
  "incompatible",
]);

export function useTranscriptOverview(sessionId: string) {
  const { scope } = useVaultHost();
  if (!scope)
    throw new Error("Transcript overview requires an immutable vault host");
  const owner = useRef({
    scope,
    retired: terminal.has(scope.getSnapshot().state),
  }).current;
  if (owner.scope !== scope) owner.retired = true;
  const currentSession = useRef(sessionId);
  currentSession.current = sessionId;
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
  const supported = owner.scope.client.features.includes("transcript_overview");
  const readable =
    !owner.retired &&
    (connection.state === "connected" || connection.state === "reconnecting");
  const query = useQuery(
    {
      ...owner.scope.transcriptOverviewOptions(sessionId),
      enabled: Boolean(sessionId) && supported && readable,
    },
    owner.scope.queryClient,
  );
  const unavailable = useMemo(
    () =>
      !supported
        ? new ApplicationRequestError({
            code: "incompatible_version",
            message:
              "This server does not support transcript overview. Upgrade the server, then reconnect.",
            retryable: false,
            request_id: null,
            current_revision: null,
            mutation_outcome: null,
          })
        : owner.retired
          ? connectionClosed()
          : null,
    [supported, owner.retired],
  );
  const error = unavailable ?? query.error;
  const { refetch: refetchQuery } = query;
  const refetch = useCallback(() => {
    const state = owner.scope.getSnapshot().state;
    if (
      supported &&
      !owner.retired &&
      Boolean(sessionId) &&
      currentSession.current === sessionId &&
      (state === "connected" || state === "reconnecting")
    )
      return refetchQuery();
  }, [owner, supported, sessionId, refetchQuery]);
  return {
    scope: owner.scope,
    readable: readable && supported,
    data:
      sessionId && readable && supported && !query.isError
        ? query.data
        : undefined,
    error,
    isError: Boolean(error),
    isPending: !error && Boolean(sessionId) && query.isPending,
    refetch,
  };
}
