import { renderHook } from "@testing-library/react";
import { expect, it, vi } from "vitest";

import { createInMemoryTaskStorage } from "@hypr/editor/task-storage";
import { extractTasksFromContent } from "@hypr/editor/tasks";

const mocks = vi.hoisted(() => ({
  useIndexQuery: vi.fn((_options: unknown) => ({ data: null })),
  useTaskStorageOptional: vi.fn(),
  enhancedDocGet: vi.fn(),
  sessionSummaryGet: vi.fn(),
  sessionGet: vi.fn(),
}));
vi.mock("~/shared/index-query", () => ({ useIndexQuery: mocks.useIndexQuery }));
vi.mock("~/types/tauri.gen", () => ({
  commands: {
    enhancedDocGet: mocks.enhancedDocGet,
    sessionSummaryGet: mocks.sessionSummaryGet,
    sessionGet: mocks.sessionGet,
  },
}));
vi.mock("@hypr/editor/task-storage", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@hypr/editor/task-storage")>()),
  useTaskStorageOptional: mocks.useTaskStorageOptional,
}));

import { useEnhancedNote, useSession } from "./queries";

it.each([
  { type: "session_summary", sessionId: "session" },
  { type: "session_summary", sessionId: undefined },
  { type: "enhanced_note", sessionId: "session" },
])(
  "waits for saved $type tasks with session context $sessionId before exposing Markdown",
  async ({ type, sessionId }) => {
    mocks.useIndexQuery.mockClear();
    const source = {
      type,
      id: type === "session_summary" ? "session" : "summary",
    };
    const storage = createInMemoryTaskStorage();
    let finishLoad!: () => void;
    const loaded = new Promise<void>((resolve) => {
      finishLoad = resolve;
    });
    storage.loadSource = vi.fn(async () => {
      await loaded;
      storage.upsertTasksForSource(source, [
        {
          taskId: "saved-id",
          sourceType: source.type,
          sourceId: source.id,
          sourceOrder: 0,
          status: "done",
          textPreview: "Send proposal",
          dueDate: "2026-10-01",
          body: [
            {
              type: "paragraph",
              content: [{ type: "text", text: "Send proposal" }],
            },
          ],
        },
      ]);
    });
    mocks.useTaskStorageOptional.mockReturnValue(storage);
    mocks.enhancedDocGet.mockResolvedValue({
      status: "ok",
      data: {
        id: source.id,
        session_id: "session",
        title: "Summary",
        template_id: "",
        sort_order: 0,
        markdown: "# Actions\n- [ ] Send proposal",
      },
    });
    mocks.sessionSummaryGet.mockResolvedValue({
      status: "ok",
      data: "# Actions\n- [ ] Send proposal",
    });
    renderHook(() => useEnhancedNote(source.id, undefined, sessionId));
    const options = mocks.useIndexQuery.mock.calls[0][0] as unknown as {
      queryFn: () => Promise<{ content: string }>;
    };
    let settled = false;
    const result = options.queryFn().then((note) => {
      settled = true;
      return note;
    });
    await vi.waitFor(() =>
      expect(storage.loadSource).toHaveBeenCalledWith(source),
    );
    expect(settled).toBe(false);
    finishLoad();
    const note = await result;
    const tasks = extractTasksFromContent(JSON.parse(note.content), source);
    expect(tasks).toHaveLength(1);
    expect(tasks[0]).toMatchObject({ taskId: "saved-id", status: "done" });
  },
);

it("hydrates raw notes from uncached canonical tasks on every reopened source", async () => {
  const source = { type: "session_raw_note", id: "session" };
  const storage = createInMemoryTaskStorage();
  const saved = [
    {
      taskId: "saved-task",
      sourceType: source.type,
      sourceId: source.id,
      sourceOrder: 0,
      status: "done" as const,
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
  mocks.useTaskStorageOptional.mockReturnValue(storage);
  mocks.sessionGet.mockResolvedValue({
    status: "ok",
    data: {
      meta: {
        id: source.id,
        title: "Tasks",
        created_at: "2026-09-20",
        tags: [],
      },
      note_markdown: "# Tasks\n- [ ] Send proposal",
    },
  });
  for (let cycle = 0; cycle < 3; cycle++) {
    mocks.useIndexQuery.mockClear();
    let finish!: () => void;
    const pending = new Promise<void>((resolve) => {
      finish = resolve;
    });
    storage.loadSource = vi.fn(async () => {
      await pending;
      return saved;
    });
    const hook = renderHook(() => useSession(source.id));
    const options = mocks.useIndexQuery.mock.calls[0][0] as unknown as {
      queryFn: () => Promise<{ raw_md: string }>;
    };
    let settled = false;
    const loading = options.queryFn().then((value) => {
      settled = true;
      return value;
    });
    await vi.waitFor(() =>
      expect(storage.loadSource).toHaveBeenCalledWith(source),
    );
    expect(settled).toBe(false);
    finish();
    const note = await loading;
    expect(
      extractTasksFromContent(JSON.parse(note.raw_md), source),
    ).toMatchObject([{ taskId: "saved-task", status: "done" }]);
    expect(storage.getTasksForSource(source)).toEqual([]);
    hook.unmount();
  }
});
