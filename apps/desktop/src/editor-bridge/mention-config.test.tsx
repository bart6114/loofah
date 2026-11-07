import { QueryClient, onlineManager } from "@tanstack/react-query";
import { cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import type {
  SessionSummary,
  SessionSummaryQuery,
} from "~/shared/application/protocol.gen";
import type { VaultScope } from "~/shared/application/vault-scope";

const mocks = vi.hoisted(() => ({
  scope: null as VaultScope | null,
  search: vi.fn(),
}));
vi.mock("~/search/contexts/engine", () => ({
  useSearchEngine: () => ({ scope: mocks.scope, search: mocks.search }),
}));

import { useMentionConfig } from "./mention-config";

const scopes: VaultScope[] = [];
function fixture(id = "local:fixture") {
  let state = "connected";
  const summaries = vi
    .fn<(input: SessionSummaryQuery) => Promise<SessionSummary[]>>()
    .mockResolvedValue([
      {
        id: "old-note",
        title: "Oldest titled note",
        created_at: "2020-01-01T00:00:00Z",
      },
    ]);
  const scope = {
    client: { scope: id },
    queryClient: new QueryClient({
      defaultOptions: {
        queries: { retry: false, networkMode: "always", staleTime: Infinity },
      },
    }),
    getSnapshot: () => ({ state, error: null }),
    sessionSummariesOptions: (input: SessionSummaryQuery) => ({
      queryKey: ["vault", id, "session_summaries", input],
      queryFn: () => summaries(input),
    }),
  } as unknown as VaultScope;
  scopes.push(scope);
  return {
    scope,
    summaries,
    setState: (value: string) => {
      state = value;
    },
  };
}
beforeEach(() => {
  vi.resetAllMocks();
});
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.queryClient.clear();
  onlineManager.setOnline(true);
});

it("loads five oldest titled summaries through the scoped query instead of the timeline", async () => {
  const { scope, summaries } = fixture();
  mocks.scope = scope;
  const { result } = renderHook(() => useMentionConfig());
  await expect(result.current.handleSearch("")).resolves.toEqual([
    { id: "old-note", type: "session", label: "Oldest titled note" },
  ]);
  expect(summaries).toHaveBeenCalledWith({
    kind: "list",
    limit: 5,
    order: "oldest_first",
    titled_only: true,
  });
  expect(mocks.search).not.toHaveBeenCalled();
});

it("keeps local empty-query mentions available while browser networking is offline", async () => {
  const { scope } = fixture();
  mocks.scope = scope;
  onlineManager.setOnline(false);
  const { result } = renderHook(() => useMentionConfig());
  await expect(result.current.handleSearch("   ")).resolves.toHaveLength(1);
});

it("does not redirect old callbacks or reveal their late summary replies after switching scopes", async () => {
  const first = fixture("remote:first"),
    second = fixture("remote:second");
  mocks.scope = first.scope;
  let finish!: (rows: SessionSummary[]) => void;
  first.summaries.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const { result, rerender } = renderHook(() => useMentionConfig());
  const old = result.current.handleSearch;
  const pending = old("");
  mocks.scope = second.scope;
  rerender();
  finish([
    {
      id: "same-note",
      title: "FIRST PRIVATE TITLE",
      created_at: "2020-01-01T00:00:00Z",
    },
  ]);
  await expect(pending).resolves.toEqual([]);
  await expect(old("")).resolves.toEqual([]);
  expect(second.summaries).not.toHaveBeenCalled();
  await result.current.handleSearch("");
  expect(second.summaries).toHaveBeenCalledOnce();
});

it("does not expose cached or late metadata after access loss or unmount", async () => {
  const { scope, summaries, setState } = fixture();
  mocks.scope = scope;
  let finish!: (rows: SessionSummary[]) => void;
  summaries.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const { result, unmount } = renderHook(() => useMentionConfig());
  const callback = result.current.handleSearch;
  const pending = callback("");
  setState("access_denied");
  unmount();
  finish([
    {
      id: "private",
      title: "PRIVATE TITLE",
      created_at: "2020-01-01T00:00:00Z",
    },
  ]);
  await expect(pending).resolves.toEqual([]);
  await expect(callback("")).resolves.toEqual([]);
  expect(summaries).toHaveBeenCalledOnce();
});

it("refreshes the bounded summary cache after a header invalidation", async () => {
  const { scope, summaries } = fixture();
  mocks.scope = scope;
  const { result } = renderHook(() => useMentionConfig());
  await result.current.handleSearch("");
  summaries.mockResolvedValueOnce([
    {
      id: "new",
      title: "Updated canonical title",
      created_at: "2020-01-01T00:00:00Z",
    },
  ]);
  await scope.queryClient.invalidateQueries({
    queryKey: ["vault", scope.client.scope, "session_summaries"],
  });
  await expect(result.current.handleSearch("")).resolves.toEqual([
    { id: "new", type: "session", label: "Updated canonical title" },
  ]);
  expect(summaries).toHaveBeenCalledTimes(2);
});
