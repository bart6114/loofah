import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { md2json } from "@hypr/editor/markdown";
import {
  TaskStorageProvider,
  createInMemoryTaskStorage,
} from "@hypr/editor/task-storage";
import { extractTasksFromContent } from "@hypr/editor/tasks";

import type { IndexChanged } from "~/types/tauri.gen";

const mocks = vi.hoisted(() => ({
  sessionGet: vi.fn(),
  sessionReadNote: vi.fn(),
  indexListeners: [] as Array<(event: { payload: IndexChanged }) => void>,
}));

vi.mock("~/types/tauri.gen", () => ({
  commands: {
    sessionGet: mocks.sessionGet,
    // Still exported by the bindings; nothing in the note-load path may call it.
    sessionReadNote: mocks.sessionReadNote,
  },
  events: {
    indexChanged: {
      listen: vi.fn((handler: (event: { payload: IndexChanged }) => void) => {
        mocks.indexListeners.push(handler);
        return Promise.resolve(() => {});
      }),
    },
  },
}));

import { useSessionRawMd } from "./queries";

function sessionRecord(noteMarkdown: string | null) {
  return {
    meta: {
      id: "session-1",
      title: "Standup",
      started_at: null,
      ended_at: null,
      created_at: "2026-07-10T10:00:00.000Z",
      tags: [],
      event: null,
      folder: null,
    },
    note_markdown: noteMarkdown,
  };
}

function emitIndexChanged(payload: IndexChanged) {
  for (const listener of mocks.indexListeners) {
    listener({ payload });
  }
}

function createWrapper(queryClient: QueryClient) {
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    );
  };
}

const ORIGINAL = "Notes I already typed";
const EDITED = "Notes I already typed, plus a new line";

describe("useSessionRawMd", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    mocks.sessionGet.mockResolvedValue({
      status: "ok",
      data: sessionRecord(ORIGINAL),
    });
    mocks.sessionReadNote.mockResolvedValue({ status: "ok", data: ORIGINAL });
  });

  it("returns null until the session has loaded", async () => {
    mocks.sessionGet.mockResolvedValue({ status: "ok", data: null });
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });

    const { result } = renderHook(() => useSessionRawMd("session-1"), {
      wrapper: createWrapper(queryClient),
    });

    expect(result.current).toBeNull();
    await waitFor(() => expect(mocks.sessionGet).toHaveBeenCalled());
    expect(result.current).toBeNull();
  });

  // Regression: the note body used to come from a second `session_read_note` query cached
  // under `["session-note-file", id]` with `staleTime: Infinity`. Nothing ever invalidated
  // that key, and it was *preferred* over the index value the bus does refresh -- so an
  // edit made in Obsidian (or in a second window) never reached the editor, and the next
  // keystroke's `persistChange` wrote the frozen content back over `notes.md`.
  it("picks up an external edit when the index bus fires", async () => {
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });

    const { result } = renderHook(() => useSessionRawMd("session-1"), {
      wrapper: createWrapper(queryClient),
    });

    await waitFor(() =>
      expect(result.current).toBe(JSON.stringify(md2json(ORIGINAL))),
    );

    // Obsidian writes `notes.md`; the vault watcher refreshes the index and emits.
    mocks.sessionGet.mockResolvedValue({
      status: "ok",
      data: sessionRecord(EDITED),
    });
    act(() => {
      emitIndexChanged({ entity: "sessions", ids: ["session-1"] });
    });

    await waitFor(() =>
      expect(result.current).toBe(JSON.stringify(md2json(EDITED))),
    );
  });

  // Regression: non-active tabs unmount (see main/body.tsx), so switching away and back
  // remounts the editor. With the immortal file-read cache, the remount re-seeded the
  // editor with pre-edit content and the next keystroke reverted the user's typing on disk.
  it("re-reads the note on remount instead of serving a cached body", async () => {
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const wrapper = createWrapper(queryClient);

    const first = renderHook(() => useSessionRawMd("session-1"), { wrapper });
    await waitFor(() =>
      expect(first.result.current).toBe(JSON.stringify(md2json(ORIGINAL))),
    );

    // Tab switched away; the note changes while this surface is unmounted.
    first.unmount();
    mocks.sessionGet.mockResolvedValue({
      status: "ok",
      data: sessionRecord(EDITED),
    });

    const second = renderHook(() => useSessionRawMd("session-1"), { wrapper });
    await waitFor(() =>
      expect(second.result.current).toBe(JSON.stringify(md2json(EDITED))),
    );
  });

  it("never reads the note file behind the bus's back", async () => {
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });

    const { result } = renderHook(() => useSessionRawMd("session-1"), {
      wrapper: createWrapper(queryClient),
    });
    await waitFor(() => expect(result.current).not.toBeNull());

    expect(mocks.sessionReadNote).not.toHaveBeenCalled();
    expect(
      queryClient.getQueryCache().findAll({ queryKey: ["session-note-file"] }),
    ).toEqual([]);
  });
});

it("refreshes canonical task status when task persistence completes after the note event", async () => {
  const source = { type: "session_raw_note", id: "session-1" };
  const storage = createInMemoryTaskStorage();
  let status: "todo" | "done" = "todo";
  storage.loadSource = async () => [
    {
      taskId: "saved-task",
      sourceType: source.type,
      sourceId: source.id,
      sourceOrder: 0,
      status,
      textPreview: "Send proposal",
      dueDate: "2026-10-01",
      body: [
        {
          type: "paragraph",
          content: [{ type: "text", text: "Send proposal" }],
        },
      ],
    },
  ];
  mocks.sessionGet.mockResolvedValue({
    status: "ok",
    data: sessionRecord("- [x] Send proposal"),
  });
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>
      <TaskStorageProvider storage={storage}>{children}</TaskStorageProvider>
    </QueryClientProvider>
  );
  const taskStatus = (content: string | null) =>
    content && extractTasksFromContent(JSON.parse(content), source)[0]?.status;
  const first = renderHook(() => useSessionRawMd(source.id), { wrapper });
  await waitFor(() => expect(taskStatus(first.result.current)).toBe("todo"));
  status = "done";
  act(() => emitIndexChanged({ entity: "tasks", ids: [source.id] }));
  await waitFor(() => expect(taskStatus(first.result.current)).toBe("done"));
  first.unmount();
  const reopened = renderHook(() => useSessionRawMd(source.id), { wrapper });
  expect(taskStatus(reopened.result.current)).toBe("done");
  reopened.unmount();
  client.clear();
});
