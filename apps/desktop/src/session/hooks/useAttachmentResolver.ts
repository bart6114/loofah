import { useInfiniteQuery } from "@tanstack/react-query";
import { useCallback, useMemo, useRef } from "react";

import { useVaultReadScope } from "~/shared/application/use-vault-read-scope";

export function useSessionAttachments(sessionId: string) {
  const { scope, current: ownsScope, readable } = useVaultReadScope();
  const session = useRef(sessionId);
  session.current = sessionId;
  const supported = scope.client.features.includes("attachment_list");
  const query = useInfiniteQuery(
    {
      ...scope.attachmentsOptions(sessionId),
      enabled: !!sessionId && supported && readable,
    },
    scope.queryClient,
  );
  const { refetch: refresh, fetchNextPage: next } = query;
  const current = useCallback(
    () =>
      ownsScope() && session.current === sessionId && supported && !!sessionId,
    [ownsScope, sessionId, supported],
  );
  const refetch = useCallback(
    () => (current() ? refresh() : Promise.resolve()),
    [current, refresh],
  );
  const fetchNextPage = useCallback(
    () => (current() ? next() : Promise.resolve()),
    [current, next],
  );
  const data = useMemo(
    () =>
      readable && supported
        ? query.data?.pages.flatMap((page) => page.files)
        : undefined,
    [readable, supported, query.data],
  );
  return {
    ...query,
    data,
    refetch,
    fetchNextPage,
    supported,
    isError: query.isError || !supported,
  };
}
