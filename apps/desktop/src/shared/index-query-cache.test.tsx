import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";

import { useIndexQuery } from "./index-query";

const mocks = vi.hoisted(() => ({
  listener: undefined as
    | undefined
    | ((event: { payload: { entity: string; ids: string[] } }) => void),
}));
vi.mock("~/types/tauri.gen", () => ({
  events: {
    indexChanged: {
      listen: vi.fn(async (listener) => {
        mocks.listener = listener;
        return () => {};
      }),
    },
  },
}));

function setup(client: QueryClient, read: () => Promise<number>, id = "one") {
  return renderHook(
    () =>
      useIndexQuery({
        entity: "sessions",
        ids: [id],
        queryKey: ["session", id],
        queryFn: read,
      }),
    {
      wrapper: ({ children }: { children: ReactNode }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  );
}

describe("vault query cache lifetime", () => {
  it("reuses fresh data across mounts and invalidates inactive entries by session", async () => {
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const read = vi.fn(async () => 1);
    const first = setup(client, read);
    await waitFor(() => expect(first.result.current.data).toBe(1));
    first.unmount();
    const second = setup(client, read);
    await waitFor(() => expect(second.result.current.data).toBe(1));
    expect(read).toHaveBeenCalledTimes(1);
    second.unmount();
    act(() =>
      mocks.listener!({ payload: { entity: "sessions", ids: ["other"] } }),
    );
    expect(client.getQueryState(["session", "one"])?.isInvalidated).toBe(false);
    act(() =>
      mocks.listener!({ payload: { entity: "sessions", ids: ["one"] } }),
    );
    expect(client.getQueryState(["session", "one"])?.isInvalidated).toBe(true);
    expect(read).toHaveBeenCalledTimes(1);
    read.mockResolvedValue(2);
    const third = setup(client, read);
    await waitFor(() => expect(third.result.current.data).toBe(2));
    expect(read).toHaveBeenCalledTimes(2);
    third.unmount();
    client.clear();
  });

  it("deduplicates mounted consumers, reconciles on resume, and detaches an evicted cache", async () => {
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const read = vi.fn(async () => 1);
    const first = setup(client, read);
    const second = setup(client, read);
    await waitFor(() => expect(second.result.current.data).toBe(1));
    expect(read).toHaveBeenCalledTimes(1);
    act(() => mocks.listener!({ payload: { entity: "sessions", ids: [] } }));
    await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(second.result.current.isFetching).toBe(false));
    act(() => window.dispatchEvent(new Event("focus")));
    await waitFor(() => expect(read).toHaveBeenCalledTimes(3));
    first.unmount();
    second.unmount();
    client.clear();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    act(() => mocks.listener!({ payload: { entity: "sessions", ids: [] } }));
    expect(invalidate).not.toHaveBeenCalled();
  });
});
