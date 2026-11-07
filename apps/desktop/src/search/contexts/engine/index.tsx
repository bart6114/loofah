import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useSyncExternalStore,
} from "react";

import { buildTantivyFilters } from "./filters";
import type {
  SearchEntityType,
  SearchFilters,
  SearchHit,
  SearchOptions,
} from "./types";
import { normalizeQuery } from "./utils";

import { useVaultHost } from "~/shared/application/vault-provider";
import type { VaultScope } from "~/shared/application/vault-scope";

export type {
  SearchDocument,
  SearchEntityType,
  SearchFilters,
  SearchHit,
  SearchOptions,
  SearchSnippet,
} from "./types";

const SearchEngineContext = createContext<{
  scope: VaultScope | null;
  ready: boolean;
  search: (
    query: string,
    filters?: SearchFilters | null,
    options?: SearchOptions,
    signal?: AbortSignal,
  ) => Promise<SearchHit[]>;
} | null>(null);

const pendingConnection = { state: "connecting" as const, error: null };
const getPendingConnection = () => pendingConnection;
const subscribePending = () => () => {};

export function SearchEngineProvider({
  children,
}: {
  children: React.ReactNode;
}) {
  const { scope } = useVaultHost();
  const connection = useSyncExternalStore(
    scope?.subscribe ?? subscribePending,
    scope?.getSnapshot ?? getPendingConnection,
  );
  const current = useRef(scope);
  current.current = scope;
  const owner = useMemo(
    () => ({ scope, requests: new Set<AbortController>() }),
    [scope],
  );
  useEffect(() => {
    const { scope, requests } = owner;
    current.current = scope;
    const abort = () => {
      for (const request of requests) request.abort();
      requests.clear();
    };
    const changed = () => {
      if (scope?.getSnapshot().state !== "connected") abort();
    };
    const stop = scope?.subscribe(changed);
    changed();
    return () => {
      stop?.();
      abort();
      if (current.current === scope) current.current = null;
    };
  }, [owner]);
  const ready =
    connection.state === "connected" &&
    !!scope?.client.features.includes("search");
  const search = useCallback(
    async (
      query: string,
      filters: SearchFilters | null = null,
      options: SearchOptions = {},
      signal?: AbortSignal,
    ): Promise<SearchHit[]> => {
      const { scope, requests } = owner;
      if (
        !scope ||
        current.current !== scope ||
        scope.getSnapshot().state !== "connected" ||
        !scope.client.features.includes("search") ||
        signal?.aborted
      )
        return [];
      const normalizedQuery = normalizeQuery(query);
      const tantivyFilters = buildTantivyFilters(filters);
      const controller = new AbortController();
      const cancel = () => controller.abort();
      signal?.addEventListener("abort", cancel, { once: true });
      requests.add(controller);
      try {
        const result = await scope.run(() =>
          scope.client.search(
            {
              query: normalizedQuery,
              filters: tantivyFilters,
              ...(options.limit !== undefined && { limit: options.limit }),
              ...(options.snippets && {
                options: {
                  fuzzy: null,
                  distance: null,
                  phrase_slop: null,
                  snippets: true,
                  snippet_max_chars: options.snippetMaxChars ?? null,
                },
              }),
            },
            { signal: controller.signal },
          ),
        );
        if (
          controller.signal.aborted ||
          current.current !== scope ||
          scope.getSnapshot().state !== "connected"
        )
          return [];

        return result.hits.map((hit) => ({
          score: hit.score,
          document: {
            id: hit.document.id,
            type: hit.document.doc_type as SearchEntityType,
            title: hit.document.title,
            content: hit.document.content,
            created_at: hit.document.created_at,
          },
          titleSnippet: hit.title_snippet,
          contentSnippet: hit.content_snippet,
        }));
      } catch (error) {
        if (!controller.signal.aborted) console.error("Search failed:", error);
        return [];
      } finally {
        signal?.removeEventListener("abort", cancel);
        requests.delete(controller);
      }
    },
    [owner],
  );

  const value = {
    scope,
    ready,
    search,
  };

  return (
    <SearchEngineContext.Provider value={value}>
      {children}
    </SearchEngineContext.Provider>
  );
}

export function useSearchEngine() {
  const context = useContext(SearchEngineContext);
  if (!context) {
    throw new Error("useSearchEngine must be used within SearchEngineProvider");
  }
  return context;
}
