import { useQuery } from "@tanstack/react-query";
import { useSyncExternalStore } from "react";

import type { SessionSummaryQuery } from "~/shared/application/protocol.gen";
import type { VaultScope } from "~/shared/application/vault-scope";

export function useSessionSummaries(
  scope: VaultScope,
  input: SessionSummaryQuery,
  enabled = true,
) {
  // TanStack keeps the initial query client, so the host must remount per scope.
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  return useQuery(
    {
      ...scope.sessionSummariesOptions(input),
      enabled: enabled && connection.state === "connected",
    },
    scope.queryClient,
  );
}
