import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { ExportModal } from "./export-modal";

const mocks = vi.hoisted(() => ({
  read: vi.fn(),
  render: vi.fn(),
  export: vi.fn(),
  download: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({
  commands: {
    sessionTranscripts: mocks.read,
    sessionTranscriptMetadata: async () => ({
      status: "ok",
      data: { speaker_labels: [], started_at: 0, ended_at: 60000 },
    }),
  },
  events: { indexChanged: { listen: async () => () => {} } },
}));
vi.mock("~/session/queries", () => ({
  useSession: () => ({
    title: "Meeting",
    created_at: "2026-01-01",
    raw_md: "",
  }),
  useEnhancedNote: () => null,
}));
vi.mock("~/people/queries", () => ({ usePeople: () => [] }));
vi.mock("@hypr/plugin-transcription", () => ({
  commands: { renderTranscriptSegments: mocks.render },
}));
vi.mock("@hypr/plugin-export", () => ({
  commands: { export: mocks.export, exportText: mocks.export },
}));
vi.mock("@hypr/plugin-fs-sync", () => ({
  commands: { attachmentList: async () => ({ status: "ok", data: [] }) },
}));
vi.mock("@hypr/plugin-opener2", () => ({
  commands: { revealItemInDir: vi.fn() },
}));
vi.mock("@tauri-apps/api/path", () => ({
  downloadDir: mocks.download,
  join: async (...parts: string[]) => parts.join("/"),
}));

const transcript = {
  id: "t1",
  session_id: "s1",
  user_id: "self",
  started_at: 0,
  words: [{ id: "w1", text: "hello", start_ms: 0, end_ms: 100, channel: 0 }],
  speaker_hints: [],
};
let client: QueryClient;
beforeEach(() => {
  vi.clearAllMocks();
  client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  mocks.read.mockResolvedValue({ status: "ok", data: [transcript] });
  mocks.render.mockResolvedValue({
    status: "ok",
    data: [
      {
        text: "hello",
        speaker_label: "You",
        start_ms: 0,
        end_ms: 100,
        words: [],
      },
    ],
  });
  mocks.download.mockResolvedValue("/downloads");
  mocks.export.mockResolvedValue({ status: "ok", data: null });
});
afterEach(() => {
  cleanup();
  client.clear();
  vi.useRealTimers();
});

function setup() {
  const onOpenChange = vi.fn();
  const content = (open: boolean) => (
    <QueryClientProvider client={client}>
      <ExportModal
        sessionId="s1"
        currentView={{ type: "raw" }}
        open={open}
        onOpenChange={onOpenChange}
      />
    </QueryClientProvider>
  );
  const view = render(content(true));
  return { ...view, setOpen: (open: boolean) => view.rerender(content(open)) };
}

it("exports duration without loading words, and only subscribes to words while selected and open", async () => {
  const view = setup();
  await waitFor(() =>
    expect(
      (screen.getByRole("button", { name: "Export" }) as HTMLButtonElement)
        .disabled,
    ).toBe(false),
  );
  expect(mocks.read).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Export" }));
  await waitFor(() => expect(mocks.export).toHaveBeenCalled());
  expect(mocks.export.mock.calls[0][1].metadata.duration).toBe("1m");
  fireEvent.click(screen.getByLabelText("Transcript"));
  await waitFor(() => expect(mocks.render).toHaveBeenCalledTimes(1));
  await waitFor(() =>
    expect(screen.queryByText("Preparing transcript...")).toBeNull(),
  );
  const raw = client
    .getQueryCache()
    .find({ queryKey: ["session-transcripts", "s1"] })!;
  const rendered = client
    .getQueryCache()
    .findAll({ queryKey: ["transcript-export-segments", "s1"] })
    .find((query) => query.state.data !== undefined)!;
  expect(raw.getObserversCount()).toBe(1);
  fireEvent.click(screen.getByLabelText("Transcript"));
  expect(raw.getObserversCount()).toBe(0);
  expect(rendered.getObserversCount()).toBe(0);
  fireEvent.click(screen.getByLabelText("Transcript"));
  view.setOpen(false);
  expect(raw.getObserversCount()).toBe(0);
  expect(rendered.getObserversCount()).toBe(0);
  view.setOpen(true);
  expect(
    (screen.getByLabelText("Transcript") as HTMLInputElement).checked,
  ).toBe(true);
  expect(mocks.read).toHaveBeenCalledTimes(1);
  expect(mocks.render).toHaveBeenCalledTimes(1);
  view.setOpen(false);
  vi.useFakeTimers();
  // Remount under fake timers so the expiry timer is deterministic.
  view.setOpen(true);
  view.setOpen(false);
  await act(() => vi.advanceTimersByTimeAsync(300001));
  expect(
    client.getQueryCache().find({ queryKey: ["session-transcripts", "s1"] }),
  ).toBeUndefined();
  expect(
    client
      .getQueryCache()
      .findAll({ queryKey: ["transcript-export-segments", "s1"] })
      .filter((query) => query.state.data !== undefined),
  ).toHaveLength(0);
});

it("keeps preparation pending across both fetch stages and retries a failed read", async () => {
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
  setup();
  fireEvent.click(screen.getByLabelText("Transcript"));
  await waitFor(() => expect(mocks.read).toHaveBeenCalled());
  expect(
    screen
      .getByRole("button", { name: "Preparing transcript..." })
      .hasAttribute("disabled"),
  ).toBe(true);
  await act(async () => resolveRead({ status: "error", error: "unavailable" }));
  await waitFor(() =>
    expect(screen.getByRole("alert").textContent).toContain(
      "Couldn't prepare export",
    ),
  );
  expect(
    screen.getByRole("button", { name: "Export" }).hasAttribute("disabled"),
  ).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await waitFor(() => expect(mocks.render).toHaveBeenCalled());
  expect(
    screen
      .getByRole("button", { name: "Preparing transcript..." })
      .hasAttribute("disabled"),
  ).toBe(true);
  await act(async () => resolveRender({ status: "ok", data: [] }));
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "Export" }).hasAttribute("disabled"),
    ).toBe(false),
  );
});

it("finishes an immutable export snapshot after closing the dialog", async () => {
  let resolveDownload!: (value: string) => void;
  mocks.download.mockReturnValueOnce(
    new Promise((resolve) => {
      resolveDownload = resolve;
    }),
  );
  const view = setup();
  fireEvent.click(screen.getByLabelText("Transcript"));
  await waitFor(() => expect(mocks.render).toHaveBeenCalled());
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "Export" }).hasAttribute("disabled"),
    ).toBe(false),
  );
  fireEvent.click(screen.getByRole("button", { name: "Export" }));
  await waitFor(() => expect(mocks.download).toHaveBeenCalled());
  fireEvent.click(screen.getByLabelText("Transcript"));
  view.setOpen(false);
  await act(async () => resolveDownload("/downloads"));
  await waitFor(() => expect(mocks.export).toHaveBeenCalled());
  expect(mocks.export.mock.calls[0][1].transcript.items[0].text).toBe("hello");
  await waitFor(() =>
    expect(client.getMutationCache().getAll()).toHaveLength(0),
  );
});
