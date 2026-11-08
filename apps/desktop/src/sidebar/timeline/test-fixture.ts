import { vi } from "vitest";

import type {
  TimelineFacets,
  TimelineHeader,
  TimelinePage,
  TimelineQuery,
} from "~/shared/application/protocol.gen";
import type {
  VaultClient,
  VaultClientEvent,
} from "~/shared/application/vault-client";
import { VaultScope } from "~/shared/application/vault-scope";

export function header(
  id: string,
  timestamp_ms: number | null = 1,
): TimelineHeader {
  return {
    header: {
      id,
      title: id,
      created_at:
        timestamp_ms === null
          ? "handwritten"
          : new Date(timestamp_ms).toISOString(),
      folder: null,
      tags: [],
      author: null,
      has_transcript_words: false,
    },
    timestamp_ms,
  };
}
export function page(
  items: TimelineHeader[] = [],
  patch: Partial<TimelinePage> = {},
): TimelinePage {
  return {
    items,
    revision: "r1",
    total_count: items.length,
    future_count: 0,
    undated_count: 0,
    next_cursor: null,
    ...patch,
  };
}
export function timelineFixture(
  networkMode: "always" | "online" = "always",
  id = "same",
) {
  let receive!: (event: VaultClientEvent) => void;
  const client = {
    scope: id,
    networkMode,
    canEdit: true,
    features: ["timeline_read"],
    timelineHeaders: vi.fn(async (_input: TimelineQuery) => page()),
    timelineFacets: vi.fn(
      async (): Promise<TimelineFacets> => ({
        revision: "r1",
        nodes: [],
        untagged_count: 0,
      }),
    ),
    subscribe: vi.fn(async (callback: (event: VaultClientEvent) => void) => {
      receive = callback;
      callback({ kind: "connection", state: "connected", error: null });
      return vi.fn();
    }),
  };
  const scope = new VaultScope(client as unknown as VaultClient);
  scope.queryClient.setDefaultOptions({
    queries: { networkMode, retry: false, staleTime: Infinity },
  });
  return {
    client,
    scope,
    start: () => scope.start(),
    emit: (event: VaultClientEvent) => receive(event),
  };
}
export function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}
