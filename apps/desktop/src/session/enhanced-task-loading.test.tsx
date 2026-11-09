import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";

import { extractTasksFromContent } from "@hypr/editor/tasks";

import { useSession } from "./queries";
import {
  createSessionSourceHost,
  sessionNote,
} from "./session-source.test-support";

import { taskItemToRecord } from "~/editor-bridge/task-records";

const hosts: ReturnType<typeof createSessionSourceHost>[] = [];
afterEach(() => {
  cleanup();
  hosts.forEach((host) => host.dispose());
  hosts.length = 0;
});
it("hydrates raw rich tasks only from the captured compound snapshot on reopening and task updates", async () => {
  const host = createSessionSourceHost();
  hosts.push(host);
  const snapshot = sessionNote("# Tasks\n- [ ] Send **proposal**");
  snapshot.tasks.tasks = [
    {
      id: "saved-task",
      source_type: "session_raw_note",
      source_id: "session-1",
      source_order: 0,
      created_at: "",
      updated_at: "",
      status: "done",
      text: "Send proposal",
      body: [
        {
          type: "paragraph",
          content: [
            { type: "text", text: "Send " },
            { type: "text", text: "proposal", marks: [{ type: "bold" }] },
          ],
        },
      ],
      due_at: "2026-11-09",
    },
  ];
  host.setNote(snapshot);
  for (let cycle = 0; cycle < 3; cycle++) {
    const view = renderHook(() => useSession("session-1"), {
      wrapper: host.Host,
    });
    await waitFor(() =>
      expect(view.result.current?.raw_md).toContain("saved-task"),
    );
    await waitFor(() => {
      const current = extractTasksFromContent(
        JSON.parse(view.result.current!.raw_md),
        { type: "session_raw_note", id: "session-1" },
      );
      expect(current[0].status).toBe(cycle === 0 ? "done" : "in_progress");
    });
    const tasks = extractTasksFromContent(
      JSON.parse(view.result.current!.raw_md),
      { type: "session_raw_note", id: "session-1" },
      new Map(
        snapshot.tasks.tasks.map((item) => {
          const task = taskItemToRecord(item)!;
          return [task.taskId, task];
        }),
      ),
    );
    expect(tasks).toMatchObject([
      {
        taskId: "saved-task",
        status: cycle === 0 ? "done" : "in_progress",
        dueDate: "2026-11-09",
        body: [
          {
            content: [
              { text: "Send " },
              { text: "proposal", marks: [{ type: "bold" }] },
            ],
          },
        ],
      },
    ]);
    view.unmount();
    snapshot.tasks.tasks[0].status = "in_progress";
    snapshot.tasks.revision = `tasks-${cycle}`;
    host.setNote(structuredClone(snapshot));
  }
});
it("refuses foreign source tasks rather than loading a global source", async () => {
  const host = createSessionSourceHost();
  hosts.push(host);
  const snapshot = sessionNote("- [ ] foreign");
  snapshot.tasks.tasks = [
    {
      id: "foreign",
      source_type: "enhanced_note",
      source_id: "other",
      source_order: 0,
      created_at: "",
      updated_at: "",
      status: "todo",
      text: "foreign",
      body: null,
      due_at: "",
    },
  ];
  host.setNote(snapshot);
  const view = renderHook(() => useSession("session-1"), {
    wrapper: host.Host,
  });
  await waitFor(() => expect(host.readNoteEditor).toHaveBeenCalledOnce());
  expect(view.result.current).toBeNull();
});

it.each(["", "Body"])(
  "keeps opaque future task bodies untouched and refuses projection for %s notes",
  async (markdown) => {
    const host = createSessionSourceHost();
    hosts.push(host);
    const snapshot = sessionNote(markdown);
    snapshot.tasks.tasks = [
      {
        id: "future",
        source_type: "session_raw_note",
        source_id: "session-1",
        source_order: 0,
        status: "todo",
        text: "Keep opaque",
        body: { future: ["payload"] },
        due_at: "",
        created_at: "",
        updated_at: "",
      },
    ];
    host.setNote(snapshot);
    const view = renderHook(() => useSession("session-1"), {
      wrapper: host.Host,
    });
    await waitFor(() => expect(host.readNoteEditor).toHaveBeenCalledOnce());
    expect(view.result.current).toBeNull();
    expect(
      host.scope.queryClient.getQueryData(
        host.scope.noteEditorOptions("session-1").queryKey,
      ),
    ).toEqual(snapshot);
  },
);

it("withdraws an old valid projection when a later snapshot contains opaque task data", async () => {
  const host = createSessionSourceHost();
  hosts.push(host);
  const view = renderHook(() => useSession("session-1"), {
    wrapper: host.Host,
  });
  await waitFor(() =>
    expect(view.result.current?.raw_md).toContain("Original body"),
  );
  const snapshot = sessionNote("Keep original bytes");
  snapshot.tasks.tasks = [
    {
      id: "future",
      source_type: "session_raw_note",
      source_id: "session-1",
      source_order: 0,
      status: "todo",
      text: "Future",
      body: { unknown: true },
      due_at: "",
      created_at: "",
      updated_at: "",
    },
  ];
  host.setNote(snapshot);
  act(() =>
    host.emit({
      kind: "change",
      change: { kind: "index_changed", entity: "tasks", ids: ["session-1"] },
    }),
  );
  await waitFor(() => expect(view.result.current).toBeNull());
});
