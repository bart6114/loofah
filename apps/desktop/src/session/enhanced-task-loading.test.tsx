import { renderHook } from "@testing-library/react";
import { expect, it, vi } from "vitest";

import { createInMemoryTaskStorage } from "@hypr/editor/task-storage";
import { extractTasksFromContent } from "@hypr/editor/tasks";

const mocks = vi.hoisted(() => ({
  useIndexQuery: vi.fn((_options: unknown) => ({ data: null })),
  useTaskStorageOptional: vi.fn(),
  enhancedDocGet: vi.fn(),
  sessionSummaryGet: vi.fn(),
}));
vi.mock("~/shared/index-query", () => ({ useIndexQuery: mocks.useIndexQuery }));
vi.mock("~/types/tauri.gen", () => ({
  commands: {
    enhancedDocGet: mocks.enhancedDocGet,
    sessionSummaryGet: mocks.sessionSummaryGet,
  },
}));
vi.mock("@hypr/editor/task-storage", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@hypr/editor/task-storage")>()),
  useTaskStorageOptional: mocks.useTaskStorageOptional,
}));

import { useEnhancedNote } from "./queries";

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
