import { onlineManager } from "@tanstack/react-query";
import {
  act,
  cleanup,
  render,
  renderHook,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

import { useSessionSummaries } from "./summary-queries";

import type {
  SessionSummary,
  SessionSummaryQuery,
} from "~/shared/application/protocol.gen";
import type {
  VaultClient,
  VaultClientEvent,
} from "~/shared/application/vault-client";
import { VaultScope } from "~/shared/application/vault-scope";

const input: SessionSummaryQuery = {
  kind: "list",
  limit: 55,
  order: "newest_first",
  titled_only: false,
};
const scopes: VaultScope[] = [];
async function fixture(id: string) {
  let receive!: (event: VaultClientEvent) => void;
  const summaries = vi
    .fn<
      (
        input: SessionSummaryQuery,
        options?: { signal?: AbortSignal },
      ) => Promise<SessionSummary[]>
    >()
    .mockResolvedValue([
      { id: "same-note", title: id, created_at: "2026-01-01T00:00:00Z" },
    ]);
  const client = {
    scope: id,
    networkMode: "always",
    sessionSummaries: summaries,
    subscribe: async (callback: (event: VaultClientEvent) => void) => {
      receive = callback;
      callback({ kind: "connection", state: "connected", error: null });
      return () => {};
    },
  } as unknown as VaultClient;
  const scope = new VaultScope(client);
  scopes.push(scope);
  scope.queryClient.setDefaultOptions({
    queries: { retry: false, networkMode: "always", staleTime: Infinity },
  });
  await scope.start();
  return {
    scope,
    summaries,
    emit: (event: VaultClientEvent) => receive(event),
  };
}
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
  onlineManager.setOnline(true);
});

it("uses one bounded scoped query and revalidates when session headers change", async () => {
  const { scope, summaries, emit } = await fixture("local:fixture");
  const { result } = renderHook(() => useSessionSummaries(scope, input));
  await waitFor(() =>
    expect(result.current.data?.[0].title).toBe("local:fixture"),
  );
  expect(summaries).toHaveBeenCalledWith(input, {
    signal: expect.any(AbortSignal),
  });
  summaries.mockResolvedValueOnce([
    {
      id: "same-note",
      title: "Updated title",
      created_at: "2026-01-01T00:00:00Z",
    },
  ]);
  act(() =>
    emit({
      kind: "change",
      change: {
        kind: "index_changed",
        entity: "session_headers",
        ids: ["same-note"],
      },
    }),
  );
  await waitFor(() =>
    expect(result.current.data?.[0].title).toBe("Updated title"),
  );
});

it("does not expose a delayed summary from a retired scope with an identical session ID", async () => {
  const first = await fixture("remote:first"),
    second = await fixture("remote:second");
  let finish!: (rows: SessionSummary[]) => void;
  first.summaries.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  function Summary({ scope }: { scope: VaultScope }) {
    const result = useSessionSummaries(scope, input);
    return <output>{result.data?.[0].title}</output>;
  }
  const { rerender } = render(
    <Summary key={first.scope.client.scope} scope={first.scope} />,
  );
  await waitFor(() => expect(first.summaries).toHaveBeenCalledOnce());
  rerender(<Summary key={second.scope.client.scope} scope={second.scope} />);
  act(() => first.scope.dispose());
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("remote:second"),
  );
  await act(async () =>
    finish([
      {
        id: "same-note",
        title: "PRIVATE FIRST TITLE",
        created_at: "2026-01-01T00:00:00Z",
      },
    ]),
  );
  expect(screen.getByRole("status").textContent).toBe("remote:second");
  expect(
    second.scope.queryClient.getQueryData(
      second.scope.sessionSummariesOptions(input).queryKey,
    ),
  ).toEqual([
    {
      id: "same-note",
      title: "remote:second",
      created_at: "2026-01-01T00:00:00Z",
    },
  ]);
  expect(first.scope.queryClient.getQueryCache().getAll()).toHaveLength(0);
  expect(first.summaries).toHaveBeenCalledOnce();
});

it("clears late private data and stops query admission after access loss", async () => {
  const { scope, summaries, emit } = await fixture("remote:private");
  let finish!: (rows: SessionSummary[]) => void;
  summaries.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const { result } = renderHook(() => useSessionSummaries(scope, input));
  await waitFor(() => expect(summaries).toHaveBeenCalledOnce());
  act(() => emit({ kind: "connection", state: "access_denied", error: null }));
  await act(async () =>
    finish([
      {
        id: "same-note",
        title: "PRIVATE TITLE",
        created_at: "2026-01-01T00:00:00Z",
      },
    ]),
  );
  expect(result.current.data).toBeUndefined();
  expect(summaries).toHaveBeenCalledOnce();
});

it("keeps local metadata reads available when browser networking is offline", async () => {
  const { scope, summaries } = await fixture("local:offline");
  onlineManager.setOnline(false);
  const { result } = renderHook(() => useSessionSummaries(scope, input));
  await waitFor(() =>
    expect(result.current.data?.[0].title).toBe("local:offline"),
  );
  expect(summaries).toHaveBeenCalledOnce();
});
