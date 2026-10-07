import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { useRenderedTranscriptData } from "./renderer/data-hooks";

import { useSessionTranscriptsQuery } from "~/stt/queries";

const mocks = vi.hoisted(() => ({ read: vi.fn(), render: vi.fn() }));
vi.mock("~/types/tauri.gen", () => ({
  commands: { sessionTranscripts: mocks.read },
  events: { indexChanged: { listen: async () => () => {} } },
}));
vi.mock("@hypr/plugin-transcription", () => ({
  commands: { renderTranscriptSegments: mocks.render },
}));
const transcript = {
  id: "t1",
  session_id: "s1",
  user_id: "self",
  started_at: 0,
  words: [{ id: "w1", text: "hello", start_ms: 0, end_ms: 100, channel: 0 }],
  speaker_hints: [],
};
const people: [] = [];
let client: QueryClient;
beforeEach(() => {
  vi.resetAllMocks();
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
});
afterEach(() => {
  cleanup();
  client.clear();
});
function wrapper({ children }: { children: ReactNode }) {
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

it("distinguishes fetching from rendering, retries rendering, and clears removed words", async () => {
  let resolveRead!: (value: unknown) => void;
  let resolveRender!: (value: unknown) => void;
  mocks.read.mockReturnValueOnce(
    new Promise((resolve) => {
      resolveRead = resolve;
    }),
  );
  mocks.render.mockReturnValueOnce(
    new Promise((resolve) => {
      resolveRender = resolve;
    }),
  );
  const view = renderHook(
    () => {
      const raw = useSessionTranscriptsQuery("s1");
      const rendered = useRenderedTranscriptData(
        "t1",
        raw.data?.[0] ?? null,
        people,
      );
      return { raw, rendered };
    },
    { wrapper },
  );
  expect(view.result.current.raw.isPending).toBe(true);
  await waitFor(() => expect(mocks.read).toHaveBeenCalled());
  await act(async () => resolveRead({ status: "ok", data: [transcript] }));
  await waitFor(() => expect(mocks.render).toHaveBeenCalledOnce());
  expect(view.result.current.raw.isSuccess).toBe(true);
  expect(view.result.current.rendered.isLoading).toBe(true);
  await act(async () =>
    resolveRender({ status: "error", error: "render failed" }),
  );
  await waitFor(() => expect(view.result.current.rendered.isError).toBe(true));
  const segments = [{ id: "segment", text: "hello", words: [] }];
  mocks.render.mockResolvedValue({ status: "ok", data: segments });
  await act(async () => {
    await view.result.current.rendered.refetch();
  });
  await waitFor(() =>
    expect(view.result.current.rendered.segments).toEqual(segments),
  );
  mocks.render.mockResolvedValue({ status: "error", error: "refresh failed" });
  await act(async () => {
    await view.result.current.rendered.refetch();
  });
  await waitFor(() => expect(view.result.current.rendered.isError).toBe(true));
  expect(view.result.current.rendered.segments).toEqual(segments);
  mocks.read.mockResolvedValue({
    status: "ok",
    data: [{ ...transcript, words: [] }],
  });
  await act(async () => {
    await view.result.current.raw.refetch();
  });
  await waitFor(() =>
    expect(view.result.current.rendered.segments).toEqual([]),
  );
});

it("never replaces the selected session with a late response from another session", async () => {
  let resolveFirst!: (value: unknown) => void;
  mocks.read.mockImplementation((id) =>
    id === "first"
      ? new Promise((resolve) => {
          resolveFirst = resolve;
        })
      : Promise.resolve({
          status: "ok",
          data: [{ ...transcript, id: "second-transcript", session_id: id }],
        }),
  );
  const view = renderHook(({ id }) => useSessionTranscriptsQuery(id), {
    wrapper,
    initialProps: { id: "first" },
  });
  await waitFor(() => expect(mocks.read).toHaveBeenCalledWith("first"));
  view.rerender({ id: "second" });
  await waitFor(() =>
    expect(view.result.current.data?.[0].id).toBe("second-transcript"),
  );
  await act(async () => resolveFirst({ status: "ok", data: [transcript] }));
  expect(view.result.current.data?.[0].id).toBe("second-transcript");
  expect(
    client
      .getQueryCache()
      .find({ queryKey: ["session-transcripts", "first"] })
      ?.getObserversCount(),
  ).toBe(0);
});
