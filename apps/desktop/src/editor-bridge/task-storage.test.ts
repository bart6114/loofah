import { beforeEach, describe, expect, it, vi } from "vitest";

import { md2json } from "@hypr/editor/markdown";
import {
  normalizeTaskContent,
  hydrateTaskContent,
  extractTasksFromContent,
} from "@hypr/editor/tasks";
import type { TaskRecord, TaskSource } from "@hypr/editor/tasks";

const mocks = vi.hoisted(() => ({
  subscribeIndexChanged: vi.fn(() => () => {}),
  sessionListTasks: vi.fn(() => Promise.resolve({ status: "ok", data: [] })),
}));

vi.mock("~/shared/index-query", () => ({
  subscribeIndexChanged: mocks.subscribeIndexChanged,
}));

vi.mock("~/types/tauri.gen", () => ({
  commands: { sessionListTasks: mocks.sessionListTasks },
}));

import {
  createStoreBackedTaskStorage,
  type TaskItem,
  type TaskStorageDependencies,
} from "./task-storage";

const sessionSource: TaskSource = { type: "session_raw_note", id: "session-1" };
const task: TaskRecord = {
  taskId: "task-1",
  sourceId: "session-1",
  sourceType: "session_raw_note",
  sourceOrder: 0,
  status: "todo",
  textPreview: "Follow up",
  body: [
    {
      type: "paragraph",
      content: [{ type: "text", text: "Follow up" }],
    },
  ],
  dueDate: "2026-07-12",
};

function taskItem(overrides: Partial<TaskItem> = {}): TaskItem {
  return {
    id: "task-1",
    source_type: "session_raw_note",
    source_id: "session-1",
    source_order: 0,
    status: "todo",
    text: "Follow up",
    body: [
      { type: "paragraph", content: [{ type: "text", text: "Follow up" }] },
    ],
    due_at: "2026-07-12",
    assignee: "",
    created_at: "2026-07-10T10:00:00.000Z",
    updated_at: "2026-07-10T10:00:00.000Z",
    ...overrides,
  };
}

function createHarness(initialTasks: TaskItem[] = []) {
  const listTasks = vi.fn().mockResolvedValue(initialTasks);
  const replaceTasks = vi.fn().mockResolvedValue(undefined);
  const removeTasks = vi.fn().mockResolvedValue(undefined);
  const moveTasks = vi.fn().mockResolvedValue(undefined);
  const enqueueWrite = vi.fn(
    async (_key: string, write: () => Promise<void>) => {
      await write();
    },
  );

  // Stands in for the `index-changed` bus: records what each source subscribed with and
  // lets a test fire the matching subscribers the way a Rust `IndexEntity::Tasks` emit would.
  const busSubscribers = new Map<string, () => void>();
  const subscribeTasksChanged = vi.fn(
    (source: TaskSource, onChange: () => void) => {
      const sourceKey = `${source.type}:${source.id}`;
      busSubscribers.set(sourceKey, onChange);
      return () => {
        busSubscribers.delete(sourceKey);
      };
    },
  );
  const emitTasksChanged = (source: TaskSource) => {
    busSubscribers.get(`${source.type}:${source.id}`)?.();
  };

  const dependencies: TaskStorageDependencies = {
    listTasks,
    replaceTasks,
    removeTasks,
    moveTasks,
    enqueueWrite,
    subscribeTasksChanged,
  };

  return {
    dependencies,
    emitTasksChanged,
    enqueueWrite,
    busSubscribers,
    listTasks,
    replaceTasks,
    removeTasks,
    moveTasks,
    subscribeTasksChanged,
  };
}

describe("store-backed task storage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("publishes stable source snapshots from the initial command fetch", async () => {
    const harness = createHarness([taskItem()]);
    const storage = createStoreBackedTaskStorage(harness.dependencies);
    const listener = vi.fn();

    storage.subscribeSource(sessionSource, listener);
    expect(storage.getTasksForSource(sessionSource)).toEqual([]);
    await vi.waitFor(() => expect(listener).toHaveBeenCalledOnce());

    expect(harness.listTasks).toHaveBeenCalledWith(
      "session_raw_note",
      "session-1",
    );
    const firstSnapshot = storage.getTasksForSource(sessionSource);
    expect(firstSnapshot).toEqual([task]);
    expect(storage.getTask("task-1")).toBe(firstSnapshot[0]);
  });

  it("fetches once per source and keeps the snapshot stable across identical refetches", async () => {
    const harness = createHarness([taskItem()]);
    const storage = createStoreBackedTaskStorage(harness.dependencies);
    const listener = vi.fn();
    const otherListener = vi.fn();

    storage.subscribeSource(sessionSource, listener);
    storage.subscribeSource(sessionSource, otherListener);
    await vi.waitFor(() => expect(listener).toHaveBeenCalledOnce());
    expect(harness.listTasks).toHaveBeenCalledOnce();
    const firstSnapshot = storage.getTasksForSource(sessionSource);

    // an own write refetches; identical data must not re-notify or swap the snapshot
    storage.removeTasksForSource(sessionSource, ["task-ghost"]);
    await vi.waitFor(() => expect(harness.listTasks).toHaveBeenCalledTimes(2));
    expect(storage.getTasksForSource(sessionSource)).toBe(firstSnapshot);
    expect(listener).toHaveBeenCalledOnce();
  });

  it("refetches after an own write and notifies on changed data", async () => {
    const harness = createHarness([taskItem()]);
    const storage = createStoreBackedTaskStorage(harness.dependencies);
    const listener = vi.fn();

    storage.subscribeSource(sessionSource, listener);
    await vi.waitFor(() => expect(listener).toHaveBeenCalledOnce());

    harness.listTasks.mockResolvedValue([
      taskItem({ text: "Updated follow up" }),
    ]);
    storage.upsertTasksForSource(sessionSource, [
      { ...task, textPreview: "Updated follow up" },
    ]);

    await vi.waitFor(() => expect(listener).toHaveBeenCalledTimes(2));
    expect(harness.replaceTasks).toHaveBeenCalledOnce();
    expect(storage.getTask("task-1")?.textPreview).toBe("Updated follow up");
  });

  it("replaces the source's task list through the serialized write queue", async () => {
    const harness = createHarness();
    const storage = createStoreBackedTaskStorage(harness.dependencies);

    storage.upsertTasksForSource(sessionSource, [task]);

    await vi.waitFor(() => expect(harness.replaceTasks).toHaveBeenCalledOnce());
    expect(harness.enqueueWrite).toHaveBeenCalledWith(
      "tasks",
      expect.any(Function),
    );
    expect(harness.replaceTasks).toHaveBeenCalledWith(
      "session_raw_note",
      "session-1",
      [
        {
          id: "task-1",
          source_order: 0,
          status: "todo",
          text: "Follow up",
          body: [
            {
              type: "paragraph",
              content: [{ type: "text", text: "Follow up" }],
            },
          ],
          due_at: "2026-07-12",
        },
      ],
    );
  });

  it("skips writes when the committed source snapshot is unchanged", async () => {
    const harness = createHarness([taskItem()]);
    const storage = createStoreBackedTaskStorage(harness.dependencies);
    const listener = vi.fn();
    storage.subscribeSource(sessionSource, listener);
    await vi.waitFor(() => expect(listener).toHaveBeenCalledOnce());

    storage.upsertTasksForSource(sessionSource, [task]);

    expect(harness.enqueueWrite).not.toHaveBeenCalled();
    expect(harness.replaceTasks).not.toHaveBeenCalled();
  });

  it("scopes removals to the source and moves tasks in one batch", async () => {
    const harness = createHarness();
    const storage = createStoreBackedTaskStorage(harness.dependencies);

    storage.removeTasksForSource(sessionSource, ["task-1"]);
    storage.moveTasksToSource(
      ["task-1", "task-2"],
      { type: "enhanced_note", id: "note-1" },
      4,
    );

    await vi.waitFor(() => expect(harness.moveTasks).toHaveBeenCalledOnce());
    expect(harness.removeTasks).toHaveBeenCalledWith(
      "session_raw_note",
      "session-1",
      ["task-1"],
    );
    expect(harness.moveTasks).toHaveBeenCalledWith(
      ["task-1", "task-2"],
      "enhanced_note",
      "note-1",
      4,
    );
  });

  it("refetches the moved tasks' previous sources after a move", async () => {
    const harness = createHarness([taskItem()]);
    const storage = createStoreBackedTaskStorage(harness.dependencies);
    const listener = vi.fn();
    storage.subscribeSource(sessionSource, listener);
    await vi.waitFor(() => expect(listener).toHaveBeenCalledOnce());
    harness.listTasks.mockClear();

    storage.moveTasksToSource(
      ["task-1"],
      { type: "enhanced_note", id: "note-1" },
      0,
    );

    await vi.waitFor(() => expect(harness.listTasks).toHaveBeenCalledTimes(2));
    const fetched = harness.listTasks.mock.calls.map((call) => call.join(":"));
    expect(fetched).toContain("enhanced_note:note-1");
    expect(fetched).toContain("session_raw_note:session-1");
  });

  // Regression: `replace_tasks` is a whole-source replace, so a window holding a snapshot
  // from before another writer's change reverts that change on its next write. Before the
  // bus subscription existed, a source was only ever refetched on first subscribe and after
  // this storage's own writes -- an external `tasks` change was invisible until remount.
  it("refetches a source when the index bus reports an external tasks change", async () => {
    const harness = createHarness([taskItem()]);
    const storage = createStoreBackedTaskStorage(harness.dependencies);
    const listener = vi.fn();

    storage.subscribeSource(sessionSource, listener);
    await vi.waitFor(() => expect(listener).toHaveBeenCalledOnce());
    expect(harness.listTasks).toHaveBeenCalledOnce();

    // Another window ticks the checkbox; the file (and the index) now say "done".
    harness.listTasks.mockResolvedValue([taskItem({ status: "done" })]);
    harness.emitTasksChanged(sessionSource);

    await vi.waitFor(() => expect(listener).toHaveBeenCalledTimes(2));
    expect(harness.listTasks).toHaveBeenCalledTimes(2);
    expect(storage.getTasksForSource(sessionSource)[0]?.status).toBe("done");
    expect(storage.getTask("task-1")?.status).toBe("done");
  });

  it("scopes the bus subscription to the source and drops it on teardown", async () => {
    const harness = createHarness();
    const storage = createStoreBackedTaskStorage(harness.dependencies);

    const unsubscribe = storage.subscribeSource(sessionSource, vi.fn());
    expect(harness.subscribeTasksChanged).toHaveBeenCalledOnce();
    expect(harness.subscribeTasksChanged.mock.calls[0]?.[0]).toEqual(
      sessionSource,
    );
    expect(harness.busSubscribers.size).toBe(1);

    unsubscribe();
    expect(harness.busSubscribers.size).toBe(0);

    // A late event after teardown must not resurrect the source.
    harness.listTasks.mockClear();
    harness.emitTasksChanged(sessionSource);
    await Promise.resolve();
    expect(harness.listTasks).not.toHaveBeenCalled();
  });

  it("settles instead of looping when our own write echoes back off the bus", async () => {
    const harness = createHarness([taskItem()]);
    const storage = createStoreBackedTaskStorage(harness.dependencies);
    const listener = vi.fn();

    storage.subscribeSource(sessionSource, listener);
    await vi.waitFor(() => expect(listener).toHaveBeenCalledOnce());

    harness.listTasks.mockResolvedValue([
      taskItem({ text: "Updated follow up" }),
    ]);
    storage.upsertTasksForSource(sessionSource, [
      { ...task, textPreview: "Updated follow up" },
    ]);
    await vi.waitFor(() => expect(listener).toHaveBeenCalledTimes(2));

    // The Rust write-through emits `tasks` for this session too; the refresh it triggers
    // reads back what we just wrote, so nothing changes and no further write is issued.
    harness.replaceTasks.mockClear();
    harness.emitTasksChanged(sessionSource);
    await vi.waitFor(() => expect(harness.listTasks).toHaveBeenCalledTimes(3));

    expect(listener).toHaveBeenCalledTimes(2);
    expect(harness.replaceTasks).not.toHaveBeenCalled();
  });

  it("drops malformed items instead of exposing invalid task records", async () => {
    const harness = createHarness([taskItem({ id: "", status: "unknown" })]);
    const storage = createStoreBackedTaskStorage(harness.dependencies);
    const listener = vi.fn();
    storage.subscribeSource(sessionSource, listener);

    await vi.waitFor(() => expect(harness.listTasks).toHaveBeenCalledOnce());
    await Promise.resolve();
    expect(storage.getTasksForSource(sessionSource)).toEqual([]);
    expect(storage.getTask("task-1")).toBeNull();
    expect(listener).not.toHaveBeenCalled();
  });
});

// Pins the real wiring (not the injected test double) onto the `index-changed` bus: without
// this, `tasks` events stay an entity nothing on the frontend listens for.
describe("store-backed task storage: default index-bus wiring", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.subscribeIndexChanged.mockReturnValue(() => {});
    mocks.sessionListTasks.mockResolvedValue({ status: "ok", data: [] });
  });

  it("subscribes session note sources to `tasks` scoped to the session id", () => {
    createStoreBackedTaskStorage().subscribeSource(sessionSource, vi.fn());

    expect(mocks.subscribeIndexChanged).toHaveBeenCalledWith(
      "tasks",
      expect.any(Function),
      ["session-1"],
    );
  });

  it("subscribes enhanced-note sources unscoped, since Rust keys their events by the owning session id", () => {
    createStoreBackedTaskStorage().subscribeSource(
      { type: "enhanced_note", id: "note-1" },
      vi.fn(),
    );

    expect(mocks.subscribeIndexChanged).toHaveBeenCalledWith(
      "tasks",
      expect.any(Function),
      undefined,
    );
  });
});

it("loads background-generated task IDs and metadata before the first editor sync", async () => {
  const source = { type: "enhanced_note", id: "summary" };
  const saved = taskItem({
    source_type: source.type,
    source_id: source.id,
    status: "done",
  });
  const harness = createHarness([saved]);
  const storage = createStoreBackedTaskStorage(harness.dependencies);
  await storage.loadSource!(source);
  const previous = storage.getTasksForSource(source);
  const reopened = normalizeTaskContent(
    hydrateTaskContent({
      content: md2json("- [ ] Follow up"),
      sourceTasks: previous,
      getTask: storage.getTask,
    }),
  )!;
  storage.upsertTasksForSource(
    source,
    extractTasksFromContent(
      reopened,
      source,
      new Map(previous.map((task) => [task.taskId, task])),
    ),
  );
  expect(harness.replaceTasks).not.toHaveBeenCalled();
  expect(storage.getTasksForSource(source)[0]).toMatchObject({
    taskId: "task-1",
    status: "done",
    dueDate: "2026-07-12",
  });
});
