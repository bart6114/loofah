import {
  type QueryClient,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useEffect } from "react";

import { measureLocalQuery } from "./performance";

import { events, type IndexEntity } from "~/types/tauri.gen";

type IndexSubscription = {
  entities: readonly IndexEntity[];
  ids?: readonly string[];
  /** Receives the event's session ids (empty = anything may have changed). */
  onChange: (ids: readonly string[]) => void;
  // Dedupe handle: subscriptions sharing a key (many mounted copies of the same
  // query) get one onChange per event instead of one per copy. A large transcript
  // mounts hundreds of identical `useTranscript`/`usePeople` subscriptions; firing
  // them all made every event cancel-and-restart the same refetch hundreds of
  // times (invalidateQueries defaults to cancelRefetch), which could leave the
  // query stuck with stale data until remount.
  dedupeKey?: string;
};

const subscriptions = new Set<IndexSubscription>();
let listenerPromise: Promise<unknown> | undefined;
const queryCaches = new Map<QueryClient, () => void>();

// One tauri event listener per webview (started lazily, never unlistened -- module
// lifetime), fanning each coalesced `index-changed` event out to every mounted
// subscriber. Each window is its own JS context, so every webview gets its own
// singleton and the backend's emit-to-all reaches them all.
function invalidateCachedQueries(
  client: QueryClient,
  entity?: IndexEntity,
  ids: readonly string[] = [],
) {
  void client.invalidateQueries({
    predicate: (query) => {
      const scope = query.meta?.vaultIndex as
        | { entities: readonly IndexEntity[]; ids?: readonly string[] }
        | undefined;
      return (
        !!scope &&
        (!entity || scope.entities.includes(entity)) &&
        (!scope.ids ||
          ids.length === 0 ||
          ids.some((id) => scope.ids!.includes(id)))
      );
    },
  });
}

function revalidateCachedQueries() {
  for (const client of queryCaches.keys()) invalidateCachedQueries(client);
}

function ensureIndexChangedListener(): Promise<unknown> {
  if (listenerPromise) return listenerPromise;
  listenerPromise = events.indexChanged
    .listen(({ payload }) => {
      for (const client of queryCaches.keys())
        invalidateCachedQueries(client, payload.entity, payload.ids);
      const seenDedupeKeys = new Set<string>();
      for (const subscription of subscriptions) {
        if (!subscription.entities.includes(payload.entity)) continue;
        if (
          subscription.ids &&
          payload.ids.length > 0 &&
          !payload.ids.some((id) => subscription.ids!.includes(id))
        )
          continue;
        if (subscription.dedupeKey) {
          if (seenDedupeKeys.has(subscription.dedupeKey)) continue;
          seenDedupeKeys.add(subscription.dedupeKey);
        }
        subscription.onChange(payload.ids);
      }
    })
    .then(() => {
      revalidateCachedQueries();
    })
    .catch((error) => {
      listenerPromise = undefined;
      console.error("[index-query] failed to listen for index changes", error);
      if (subscriptions.size || queryCaches.size) {
        setTimeout(() => {
          void ensureIndexChangedListener().catch(() => {});
        }, 1000);
      }
      throw error;
    });
  return listenerPromise;
}

function trackQueryCache(client: QueryClient) {
  if (queryCaches.has(client)) return;
  if (queryCaches.size === 0) {
    window.addEventListener("focus", revalidateCachedQueries);
    document.addEventListener("visibilitychange", revalidateVisibleQueries);
  }
  const unsubscribe = client.getQueryCache().subscribe((event) => {
    if (
      event.type !== "removed" ||
      client
        .getQueryCache()
        .getAll()
        .some((query) => query.meta?.vaultIndex)
    )
      return;
    unsubscribe();
    queryCaches.delete(client);
    if (queryCaches.size === 0) {
      window.removeEventListener("focus", revalidateCachedQueries);
      document.removeEventListener(
        "visibilitychange",
        revalidateVisibleQueries,
      );
    }
  });
  queryCaches.set(client, unsubscribe);
}

function revalidateVisibleQueries() {
  if (document.visibilityState === "visible") revalidateCachedQueries();
}

/**
 * Non-hook variant for imperative subscribers (meeting-float host, event listeners):
 * calls `onChange` whenever a matching `index-changed` event arrives. Returns an
 * unsubscribe function.
 */
export function subscribeIndexChanged(
  entity: IndexEntity | readonly IndexEntity[],
  onChange: (ids: readonly string[]) => void,
  ids?: readonly string[],
  dedupeKey?: string,
): () => void {
  void ensureIndexChangedListener().catch(() => {});
  const subscription: IndexSubscription = {
    entities: Array.isArray(entity) ? entity : [entity as IndexEntity],
    ids,
    onChange,
    dedupeKey,
  };
  subscriptions.add(subscription);
  return () => {
    subscriptions.delete(subscription);
  };
}

/**
 * `useQuery` that re-fetches when the vault index reports a change to the given
 * entity (optionally scoped to specific event ids -- note that `docs` and
 * `transcripts` events carry *session* ids). Replaces the SQL live-query hooks:
 * consumers keep the same `{ data, isLoading, error }` contract.
 */
export function useIndexQuery<TData>({
  entity,
  ids,
  queryKey,
  queryFn,
  enabled = true,
  refetchOnMount,
}: {
  entity: IndexEntity | readonly IndexEntity[];
  ids?: readonly string[];
  queryKey: readonly unknown[];
  queryFn: () => Promise<TData>;
  enabled?: boolean;
  refetchOnMount?: boolean | "always";
}) {
  const queryClient = useQueryClient();
  const query = useQuery({
    queryKey: queryKey as unknown[],
    queryFn: async () => {
      await ensureIndexChangedListener();
      return measureLocalQuery(queryFn);
    },
    enabled,
    staleTime: Infinity,
    refetchOnMount,
    refetchOnWindowFocus: false,
    meta: {
      vaultIndex: { entities: Array.isArray(entity) ? entity : [entity], ids },
    },
    // These queries read local vault state over Tauri IPC, not the network.
    // The default "online" mode pauses (re)fetches whenever the webview
    // reports itself offline, leaving the mounted view stuck with stale data
    // until a remount -- there is no network to wait for here.
    networkMode: "always",
  });

  useEffect(() => {
    trackQueryCache(queryClient);
    void ensureIndexChangedListener().catch(() => {});
  }, [queryClient]);

  return query;
}
