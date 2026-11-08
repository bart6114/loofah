import { onlineManager } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { useTranscriptExportSegments } from "./export-data";
import { createRenderCacheHost } from "./render-cache.test-support";

const source = vi.hoisted(() => ({ read: vi.fn(), render: vi.fn() }));
vi.mock("~/types/tauri.gen", () => ({
  commands: { sessionTranscripts: source.read },
  events: { indexChanged: { listen: async () => () => {} } },
}));
vi.mock("~/people/queries", () => ({ usePeople: () => [] }));
vi.mock("@hypr/plugin-transcription", () => ({
  commands: { renderTranscriptSegments: source.render },
}));
const row = (id = "t1", sessionId = "s1") => ({
  id,
  session_id: sessionId,
  user_id: "self",
  started_at: 0,
  words: [{ id: "w", text: "hello", start_ms: 0, end_ms: 100, channel: 0 }],
  speaker_hints: [],
});
const segments = (text: string) => [
  { id: text, text, start_ms: 0, end_ms: 100, speaker_label: "You", words: [] },
];
let hosts: ReturnType<typeof createRenderCacheHost>[];
function host(scopeId?: string, networkMode?: "always" | "online") {
  const value = createRenderCacheHost(scopeId, networkMode);
  hosts.push(value);
  return value;
}
beforeEach(() => {
  hosts = [];
  source.read.mockReset().mockResolvedValue({ status: "ok", data: [row()] });
  source.render
    .mockReset()
    .mockResolvedValue({ status: "ok", data: segments("Original") });
});
afterEach(() => {
  cleanup();
  for (const backend of hosts) backend.dispose();
  onlineManager.setOnline(true);
});
function Probe({ id = "s1" }: { id?: string }) {
  const value = useTranscriptExportSegments(id);
  return (
    <>
      <output>
        {value.data
          .map((segment) => `${segment.speaker}:${segment.text}`)
          .join("|")}
      </output>
      <button onClick={value.refetch}>Retry</button>
      {value.error && <div role="alert">Error</div>}
    </>
  );
}

it("keeps source reads on the unchanged source key and owns export cache by session, ordered transcript IDs and hash", async () => {
  const backend = host();
  const view = render(
    <backend.Host>
      <Probe />
      <Probe />
    </backend.Host>,
  );
  await waitFor(() =>
    expect(screen.getAllByText("You:Original")).toHaveLength(2),
  );
  expect(source.read).toHaveBeenCalledOnce();
  expect(source.read).toHaveBeenCalledWith("s1");
  expect(source.render).toHaveBeenCalledOnce();
  expect(
    backend.rootClient
      .getQueryCache()
      .getAll()
      .map((query) => query.queryKey),
  ).toEqual([["session-transcripts", "s1"]]);
  const key = backend.scope.queryClient
    .getQueryCache()
    .getAll()
    .find((query) => query.state.data)?.queryKey;
  expect(key?.slice(0, 5)).toEqual([
    "vault",
    backend.scope.client.scope,
    "transcript_export_segments",
    "s1",
    ["t1"],
  ]);
  view.unmount();
  render(
    <backend.Host>
      <Probe />
    </backend.Host>,
  );
  expect(screen.getByText("You:Original")).toBeTruthy();
  expect(source.render).toHaveBeenCalledOnce();
});

it("does not reuse another transcript's result when IDs change but explicit formatting data/hash is identical", async () => {
  const backend = host();
  render(
    <backend.Host>
      <Probe />
    </backend.Host>,
  );
  await screen.findByText("You:Original");
  source.read.mockResolvedValue({ status: "ok", data: [row("t2")] });
  source.render.mockResolvedValue({
    status: "ok",
    data: segments("Replacement transcript"),
  });
  await act(async () => {
    await backend.rootClient.refetchQueries({
      queryKey: ["session-transcripts", "s1"],
    });
  });
  await screen.findByText("You:Replacement transcript");
  expect(source.render).toHaveBeenCalledTimes(2);
  expect(
    backend.scope.queryClient
      .getQueryCache()
      .getAll()
      .filter((query) => query.state.data)
      .map((query) => query.queryKey[4]),
  ).toEqual([["t1"], ["t2"]]);
});

it.each([50, 150, 500])(
  "ignores late export replies after %sms across same-string Scope replacement",
  async (delay) => {
    const first = host("same-scope", "online");
    const second = host("same-scope", "online");
    source.render.mockImplementationOnce(
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
    await waitFor(() => expect(source.render).toHaveBeenCalledOnce());
    view.rerender(
      <second.Host>
        <Probe />
      </second.Host>,
    );
    act(() => first.dispose());
    await screen.findByText("You:Original");
    await act(
      async () => new Promise((resolve) => setTimeout(resolve, delay + 10)),
    );
    expect(screen.queryByText("You:PRIVATE OLD")).toBeNull();
    expect(first.scope.queryClient.getQueryCache().getAll()).toHaveLength(0);
  },
);

it("hides terminal exports, discards pending results and never restarts through an old retry closure", async () => {
  const backend = host();
  let finish!: (value: unknown) => void;
  source.render.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const view = render(
    <backend.Host>
      <Probe />
    </backend.Host>,
  );
  await waitFor(() => expect(source.render).toHaveBeenCalledOnce());
  act(() => {
    backend.emit("login_required");
    backend.emit("connected");
  });
  await act(async () =>
    finish({ status: "ok", data: segments("PRIVATE OLD") }),
  );
  expect(screen.queryByText("You:PRIVATE OLD")).toBeNull();
  (screen.getByText("Retry") as HTMLButtonElement).click();
  expect(source.render).toHaveBeenCalledOnce();
  view.unmount();
  expect(
    backend.scope.queryClient
      .getQueryCache()
      .getAll()
      .some((query) => query.state.data),
  ).toBe(false);
});

it.each(["always", "online"] as const)(
  "formats loaded export input offline with %s client policy",
  async (policy) => {
    const backend = host("scope", policy);
    backend.rootClient.setQueryData(
      ["session-transcripts", "s1"],
      [
        {
          id: "t1",
          sessionId: "s1",
          ownerUserId: "self",
          startedAt: 0,
          words: row().words,
          speakerHints: [],
        },
      ],
    );
    onlineManager.setOnline(false);
    act(() => backend.emit("reconnecting"));
    render(
      <backend.Host>
        <Probe />
      </backend.Host>,
    );
    await screen.findByText("You:Original");
    expect(source.render).toHaveBeenCalledOnce();
  },
);

it("refuses export formatting if source records belong to a different session", async () => {
  const backend = host();
  source.read.mockResolvedValue({
    status: "ok",
    data: [row("t1", "other-session")],
  });
  render(
    <backend.Host>
      <Probe />
    </backend.Host>,
  );
  await waitFor(() => expect(source.read).toHaveBeenCalled());
  await act(async () => {});
  expect(source.render).not.toHaveBeenCalled();
});
