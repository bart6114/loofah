import { QueryClientProvider, onlineManager } from "@tanstack/react-query";
import {
  act,
  cleanup,
  render,
  renderHook,
  screen,
  waitFor,
} from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { TRANSCRIPT_RENDER_CACHE_TIME_MS } from "../cache";
import { renderedTranscriptPrefix } from "../render-cache";
import { createRenderCacheHost } from "../render-cache.test-support";
import { useRenderedTranscriptData } from "./data-hooks";

import { VaultHostContext } from "~/shared/application/vault-provider";

const native = vi.hoisted(() => ({ render: vi.fn() }));
vi.mock("@hypr/plugin-transcription", () => ({
  commands: { renderTranscriptSegments: native.render },
}));

const transcript = {
  id: "same-transcript",
  sessionId: "same-session",
  ownerUserId: "self",
  startedAt: 0,
  words: [{ id: "w", text: "hello", start_ms: 0, end_ms: 100, channel: 0 }],
  speakerHints: [],
};
const segments = (text: string) => [{ id: text, text, words: [] }];
let hosts: ReturnType<typeof createRenderCacheHost>[];
function host(scopeId?: string, networkMode?: "always" | "online") {
  const value = createRenderCacheHost(scopeId, networkMode);
  hosts.push(value);
  return value;
}
beforeEach(() => {
  hosts = [];
  native.render
    .mockReset()
    .mockResolvedValue({ status: "ok", data: segments("Original") });
});
afterEach(() => {
  cleanup();
  for (const value of hosts) value.dispose();
  onlineManager.setOnline(true);
});
function Probe() {
  const result = useRenderedTranscriptData(transcript.id, transcript, []);
  return <output>{result.segments.map((row) => row.text).join("|")}</output>;
}

it("owns the complete key and cache in the captured Scope, deduplicates, and reuses it on remount", async () => {
  const backend = host();
  const view = render(
    <backend.Host>
      <Probe />
      <Probe />
    </backend.Host>,
  );
  await waitFor(() => expect(screen.getAllByText("Original")).toHaveLength(2));
  expect(native.render).toHaveBeenCalledOnce();
  expect(backend.rootClient.getQueryCache().getAll()).toHaveLength(0);
  const query = backend.scope.queryClient.getQueryCache().getAll()[0];
  expect(query.queryKey.slice(0, 5)).toEqual(
    renderedTranscriptPrefix(
      backend.scope,
      transcript.sessionId,
      transcript.id,
    ),
  );
  expect(query.queryKey[5]).toMatch(/^1:1:/);
  expect(query.gcTime).toBe(TRANSCRIPT_RENDER_CACHE_TIME_MS);
  view.unmount();
  render(
    <backend.Host>
      <Probe />
    </backend.Host>,
  );
  expect(screen.getByText("Original")).toBeTruthy();
  expect(native.render).toHaveBeenCalledOnce();
});

it("keeps only same-owner segments visible during a speaker change and failed refresh", async () => {
  const backend = host();
  const view = renderHook(
    ({ sessionId, people }) =>
      useRenderedTranscriptData(
        transcript.id,
        { ...transcript, sessionId },
        people,
      ),
    {
      wrapper: backend.Host,
      initialProps: {
        sessionId: transcript.sessionId,
        people: [{ id: "self", name: "Original" }],
      },
    },
  );
  await waitFor(() =>
    expect(view.result.current.segments).toEqual(segments("Original")),
  );
  let finish!: (value: unknown) => void;
  native.render.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  view.rerender({
    sessionId: transcript.sessionId,
    people: [{ id: "self", name: "Renamed" }],
  });
  expect(view.result.current.segments).toEqual(segments("Original"));
  await act(async () => finish({ status: "error", error: "format failed" }));
  await waitFor(() => expect(view.result.current.isError).toBe(true));
  expect(view.result.current.segments).toEqual(segments("Original"));
  native.render.mockImplementationOnce(() => new Promise(() => {}));
  view.rerender({ sessionId: "other-session", people: [] });
  expect(view.result.current.segments).toEqual([]);
});

it.each(["always", "online"] as const)(
  "formats explicit content offline with %s client policy, including reconnecting",
  async (policy) => {
    const backend = host("scope", policy);
    onlineManager.setOnline(false);
    act(() => backend.emit("reconnecting"));
    render(
      <backend.Host>
        <Probe />
      </backend.Host>,
    );
    await screen.findByText("Original");
    expect(native.render).toHaveBeenCalledOnce();
  },
);

it.each([50, 150, 500])(
  "discards a late %sms reply across identical-ID, same-string Scope replacement",
  async (delay) => {
    const first = host("same-scope", "online");
    const second = host("same-scope", "online");
    native.render.mockImplementationOnce(
      () =>
        new Promise((resolve) =>
          setTimeout(
            () => resolve({ status: "ok", data: segments("PRIVATE OLD") }),
            delay,
          ),
        ),
    );
    const view = render(
      <first.Host>
        <Probe />
      </first.Host>,
    );
    await waitFor(() => expect(native.render).toHaveBeenCalledOnce());
    view.rerender(
      <second.Host>
        <Probe />
      </second.Host>,
    );
    act(() => first.dispose());
    await screen.findByText("Original");
    await act(
      async () => new Promise((resolve) => setTimeout(resolve, delay + 10)),
    );
    expect(screen.queryByText("PRIVATE OLD")).toBeNull();
    expect(first.scope.queryClient.getQueryCache().getAll()).toHaveLength(0);
    expect(
      second.scope.queryClient.getQueryCache().getAll()[0].state.data,
    ).toEqual(segments("Original"));
  },
);

it.each(["access_denied", "login_required", "closed", "incompatible"] as const)(
  "latches %s and refuses stale refetch handlers even after connected returns",
  async (state) => {
    const backend = host();
    const view = renderHook(
      () => useRenderedTranscriptData(transcript.id, transcript, []),
      { wrapper: backend.Host },
    );
    await waitFor(() =>
      expect(view.result.current.segments).toEqual(segments("Original")),
    );
    const retry = view.result.current.refetch;
    act(() => {
      backend.emit(state);
      backend.emit("connected");
    });
    expect(view.result.current.segments).toEqual([]);
    await act(async () => {
      await retry();
    });
    expect(native.render).toHaveBeenCalledOnce();
  },
);

it("never admits pending native work after unmount or terminal loss", async () => {
  const backend = host();
  let finish!: (value: unknown) => void;
  native.render.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const view = renderHook(
    () => useRenderedTranscriptData(transcript.id, transcript, []),
    { wrapper: backend.Host },
  );
  await waitFor(() => expect(native.render).toHaveBeenCalledOnce());
  act(() => backend.emit("access_denied"));
  view.unmount();
  await act(async () =>
    finish({ status: "ok", data: segments("PRIVATE OLD") }),
  );
  expect(
    backend.scope.queryClient
      .getQueryCache()
      .getAll()
      .some((query) => query.state.data !== undefined),
  ).toBe(false);
});

it("clears continuity for missing words/null live placeholders and refuses mismatched transcript identity", async () => {
  const backend = host();
  const view = renderHook(
    ({ value, id }) => useRenderedTranscriptData(id, value, []),
    {
      wrapper: backend.Host,
      initialProps: {
        value: transcript as typeof transcript | null,
        id: transcript.id,
      },
    },
  );
  await waitFor(() =>
    expect(view.result.current.segments).toEqual(segments("Original")),
  );
  view.rerender({ value: { ...transcript, words: [] }, id: transcript.id });
  expect(view.result.current.segments).toEqual([]);
  view.rerender({ value: null, id: "__live-transcript__" });
  expect(view.result.current.segments).toEqual([]);
  view.rerender({ value: transcript, id: "different" });
  expect(view.result.current.segments).toEqual([]);
  expect(native.render).toHaveBeenCalledOnce();
});

it("fails closed when an identical-string replacement Scope is supplied without the required subtree remount", async () => {
  const first = host("same-scope");
  const second = host("same-scope");
  const content = (backend: typeof first) => (
    <StrictMode>
      <QueryClientProvider client={first.rootClient}>
        <VaultHostContext.Provider
          value={{ scope: backend.scope, error: null, retry: () => {} }}
        >
          <Probe />
        </VaultHostContext.Provider>
      </QueryClientProvider>
    </StrictMode>
  );
  const view = render(content(first));
  await screen.findByText("Original");
  const before = native.render.mock.calls.length;
  view.rerender(content(second));
  expect(screen.queryByText("Original")).toBeNull();
  expect(second.scope.queryClient.getQueryCache().getAll()).toHaveLength(0);
  expect(native.render).toHaveBeenCalledTimes(before);
  view.rerender(content(first));
  expect(screen.queryByText("Original")).toBeNull();
});

it("hides an already-rendered transcript immediately on Scope disposal", async () => {
  const backend = host();
  render(
    <StrictMode>
      <backend.Host>
        <Probe />
      </backend.Host>
    </StrictMode>,
  );
  await screen.findByText("Original");
  act(() => backend.scope.dispose());
  expect(screen.queryByText("Original")).toBeNull();
  expect(
    backend.scope.queryClient
      .getQueryCache()
      .getAll()
      .some((query) => query.state.data !== undefined),
  ).toBe(false);
});
