import { useEffect, useMemo, useRef } from "react";

import type { MentionConfig } from "@hypr/editor/widgets";

import { useSearchEngine } from "~/search/contexts/engine";

export function useMentionConfig(): MentionConfig {
  const { search, scope } = useSearchEngine();
  const current = useRef(scope);
  current.current = scope;
  useEffect(() => {
    current.current = scope;
    return () => {
      if (current.current === scope) current.current = null;
    };
  }, [scope]);

  return useMemo(
    () => ({
      trigger: "@",
      handleSearch: async (query: string) => {
        if (
          !scope ||
          current.current !== scope ||
          scope.getSnapshot().state !== "connected"
        )
          return [];
        const results: {
          id: string;
          type: string;
          label: string;
          content?: string;
        }[] = [];

        if (query.trim()) {
          const searchResults = await search(query);
          for (const hit of searchResults) {
            results.push({
              id: hit.document.id,
              type: hit.document.type,
              label: hit.document.title,
            });
          }
        } else {
          try {
            const sessions = await scope.queryClient.fetchQuery(
              scope.sessionSummariesOptions({
                kind: "list",
                limit: 5,
                order: "oldest_first",
                titled_only: true,
              }),
            );
            for (const session of sessions)
              results.push({
                id: session.id,
                type: "session",
                label: session.title,
              });
          } catch {
            return [];
          }
        }
        if (
          current.current !== scope ||
          scope.getSnapshot().state !== "connected"
        )
          return [];
        return results.slice(0, 5);
      },
    }),
    [scope, search],
  );
}
