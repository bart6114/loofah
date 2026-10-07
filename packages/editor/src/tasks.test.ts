import { describe, expect, it } from "vitest";

import { md2json } from "./markdown";
import { createInMemoryTaskStorage } from "./task-storage";
import {
  createTaskItemNode,
  extractTasksFromContent,
  getNextTaskStatus,
  hydrateTaskContent,
  moveOpenTasksBetweenContents,
  normalizeTaskContent,
} from "./tasks";

describe("task content", () => {
  it("toggles task status through the simple checked state", () => {
    expect(getNextTaskStatus("todo")).toBe("done");
    expect(getNextTaskStatus("in_progress")).toBe("done");
    expect(getNextTaskStatus("done")).toBe("todo");
  });

  it("assigns unique task ids and task item ids to missing and duplicated task items", () => {
    const content = normalizeTaskContent({
      type: "doc",
      content: [
        {
          type: "taskList",
          content: [
            {
              type: "taskItem",
              attrs: { checked: false, taskId: "duplicate-task" },
              content: [
                {
                  type: "paragraph",
                  content: [{ type: "text", text: "First task" }],
                },
              ],
            },
            {
              type: "taskItem",
              attrs: { checked: true, taskId: "duplicate-task" },
              content: [
                {
                  type: "paragraph",
                  content: [{ type: "text", text: "Second task" }],
                },
              ],
            },
            {
              type: "taskItem",
              attrs: { checked: false },
              content: [
                {
                  type: "paragraph",
                  content: [{ type: "text", text: "Third task" }],
                },
              ],
            },
          ],
        },
      ],
    });

    const taskItems =
      content?.content?.[0]?.content?.filter(
        (node) => node.type === "taskItem",
      ) ?? [];
    const taskIds = taskItems.map((node) => node.attrs?.taskId);
    const taskItemIds = taskItems.map((node) => node.attrs?.taskItemId);

    expect(taskIds).toHaveLength(3);
    expect(taskIds[0]).toBe("duplicate-task");
    expect(taskIds[1]).toEqual(expect.any(String));
    expect(taskIds[2]).toEqual(expect.any(String));
    expect(new Set(taskIds).size).toBe(3);
    expect(taskItemIds).toHaveLength(3);
    expect(taskItemIds[0]).toEqual(expect.any(String));
    expect(taskItemIds[1]).toEqual(expect.any(String));
    expect(taskItemIds[2]).toEqual(expect.any(String));
    expect(new Set(taskItemIds).size).toBe(3);
  });

  it("extracts canonical task rows with full body content", () => {
    const source = { type: "daily_note", id: "2026-04-06" };
    const content = normalizeTaskContent({
      type: "doc",
      content: [
        {
          type: "taskList",
          content: [
            {
              type: "taskItem",
              attrs: { checked: false, taskId: "task-1" },
              content: [
                {
                  type: "paragraph",
                  content: [
                    { type: "text", text: "Parent task" },
                    { type: "hardBreak" },
                    {
                      type: "text",
                      marks: [{ type: "bold" }],
                      text: "still here",
                    },
                  ],
                },
                {
                  type: "blockquote",
                  content: [
                    {
                      type: "paragraph",
                      content: [{ type: "text", text: "Nested detail" }],
                    },
                  ],
                },
              ],
            },
          ],
        },
      ],
    });

    const tasks = extractTasksFromContent(content!, source);

    expect(tasks).toHaveLength(1);
    expect(tasks[0]).toMatchObject({
      taskId: "task-1",
      sourceId: source.id,
      sourceType: source.type,
      sourceOrder: 0,
      status: "todo",
      textPreview: "Parent task still here",
    });
    expect(createTaskItemNode(tasks[0]!)).toMatchObject({
      type: "taskItem",
      attrs: {
        checked: false,
        taskId: "task-1",
        taskItemId: expect.any(String),
      },
      content: tasks[0]?.body,
    });
  });

  it("preserves in-progress status when extracting and rebuilding tasks", () => {
    const source = { type: "daily_note", id: "2026-04-06" };
    const content = normalizeTaskContent({
      type: "doc",
      content: [
        {
          type: "taskList",
          content: [
            {
              type: "taskItem",
              attrs: {
                status: "in_progress",
                checked: false,
                taskId: "task-2",
              },
              content: [
                {
                  type: "paragraph",
                  content: [{ type: "text", text: "Working on it" }],
                },
              ],
            },
          ],
        },
      ],
    });

    const [task] = extractTasksFromContent(content!, source);

    expect(task).toMatchObject({
      taskId: "task-2",
      status: "in_progress",
      textPreview: "Working on it",
    });
    expect(createTaskItemNode(task!)).toMatchObject({
      type: "taskItem",
      attrs: {
        status: "in_progress",
        checked: false,
        taskId: "task-2",
        taskItemId: expect.any(String),
      },
    });
  });

  it("hydrates a source from canonical tasks and removes moved-away tasks", () => {
    const source = { type: "daily_note", id: "2026-04-06" };
    const foreignTask = {
      taskId: "task-foreign",
      sourceId: "2026-04-05",
      sourceType: "daily_note",
      sourceOrder: 0,
      status: "todo" as const,
      textPreview: "Foreign task",
      body: [
        {
          type: "paragraph",
          content: [{ type: "text", text: "Foreign task" }],
        },
      ],
    };
    const sourceTask = {
      taskId: "task-1",
      sourceId: source.id,
      sourceType: source.type,
      sourceOrder: 0,
      status: "done" as const,
      textPreview: "Hydrated task",
      dueDate: "2026-04-12",
      body: [
        {
          type: "paragraph",
          content: [{ type: "text", text: "Hydrated task" }],
        },
      ],
    };
    const content = normalizeTaskContent({
      type: "doc",
      content: [
        {
          type: "taskList",
          content: [
            {
              type: "taskItem",
              attrs: { checked: false, taskId: "task-1" },
              content: [{ type: "paragraph" }],
            },
            {
              type: "taskItem",
              attrs: { checked: false, taskId: "task-foreign" },
              content: [
                {
                  type: "paragraph",
                  content: [{ type: "text", text: "Stale copy" }],
                },
              ],
            },
          ],
        },
      ],
    });

    const hydrated = hydrateTaskContent({
      content: content!,
      sourceTasks: [sourceTask],
      getTask: (taskId) =>
        taskId === foreignTask.taskId ? foreignTask : sourceTask,
    });

    expect(hydrated).toMatchObject({
      type: "doc",
      content: [
        {
          type: "taskList",
          content: [
            {
              type: "taskItem",
              attrs: { checked: true, taskId: "task-1" },
            },
          ],
        },
      ],
    });
  });

  it("moves unfinished daily tasks forward with the same task id and body", () => {
    const previousSource = { type: "daily_note", id: "2026-04-05" };
    const currentSource = { type: "daily_note", id: "2026-04-06" };
    const previousTasks = [
      {
        taskId: "task-1",
        sourceId: previousSource.id,
        sourceType: previousSource.type,
        sourceOrder: 0,
        status: "todo" as const,
        textPreview: "Carry me",
        dueDate: "2026-04-10",
        body: [
          {
            type: "paragraph",
            content: [{ type: "text", text: "Carry me" }],
          },
          {
            type: "blockquote",
            content: [
              {
                type: "paragraph",
                content: [{ type: "text", text: "Keep structure" }],
              },
            ],
          },
        ],
      },
      {
        taskId: "task-2",
        sourceId: previousSource.id,
        sourceType: previousSource.type,
        sourceOrder: 1,
        status: "done" as const,
        textPreview: "Done task",
        body: [{ type: "paragraph" }],
      },
    ];

    const result = moveOpenTasksBetweenContents({
      previousContent: {
        type: "doc",
        content: [
          {
            type: "taskList",
            content: previousTasks.map((task) => createTaskItemNode(task)),
          },
        ],
      },
      currentContent: {
        type: "doc",
        content: [{ type: "paragraph" }],
      },
      previousTasks,
      currentTasks: [],
      currentSource,
    });

    expect(result).not.toBeNull();
    expect(result?.movedTasks).toHaveLength(1);
    expect(result?.movedTasks[0]).toMatchObject({
      taskId: "task-1",
      sourceId: currentSource.id,
      sourceType: currentSource.type,
      sourceOrder: 0,
      dueDate: "2026-04-10",
      body: previousTasks[0]?.body,
    });
    expect(result?.previousContent).toMatchObject({
      type: "doc",
      content: [
        {
          type: "taskList",
          content: [
            {
              type: "taskItem",
              attrs: { taskId: "task-2" },
            },
          ],
        },
      ],
    });
    expect(result?.currentContent.content?.[1]).toMatchObject({
      type: "taskList",
      content: [
        {
          type: "taskItem",
          attrs: { taskId: "task-1", checked: false },
        },
      ],
    });
  });
});

describe("in-memory task storage", () => {
  it("reconciles tasks per source without duplicating other sources", () => {
    const storage = createInMemoryTaskStorage();

    const dailySource = { type: "daily_note", id: "2026-04-06" };
    const enhancedSource = { type: "enhanced_note", id: "enhanced-1" };

    storage.upsertTasksForSource(dailySource, [
      {
        taskId: "task-1",
        sourceId: dailySource.id,
        sourceType: dailySource.type,
        sourceOrder: 0,
        status: "todo",
        textPreview: "Daily task",
        body: [{ type: "paragraph" }],
      },
    ]);
    storage.upsertTasksForSource(enhancedSource, [
      {
        taskId: "task-2",
        sourceId: enhancedSource.id,
        sourceType: enhancedSource.type,
        sourceOrder: 0,
        status: "todo",
        textPreview: "Enhanced task",
        body: [{ type: "paragraph" }],
      },
    ]);
    storage.upsertTasksForSource(dailySource, [
      {
        taskId: "task-1",
        sourceId: dailySource.id,
        sourceType: dailySource.type,
        sourceOrder: 0,
        status: "done",
        textPreview: "Daily task updated",
        body: [{ type: "paragraph" }],
        dueDate: "2026-04-12",
      },
    ]);

    expect(storage.getTasksForSource(dailySource)).toMatchObject([
      {
        taskId: "task-1",
        status: "done",
        textPreview: "Daily task updated",
        dueDate: "2026-04-12",
      },
    ]);
    expect(storage.getTasksForSource(enhancedSource)).toMatchObject([
      {
        taskId: "task-2",
        status: "todo",
      },
    ]);
  });

  it("moves tasks to a new source with updated source order", () => {
    const storage = createInMemoryTaskStorage();
    const previousSource = { type: "daily_note", id: "2026-04-05" };
    const currentSource = { type: "daily_note", id: "2026-04-06" };

    storage.upsertTasksForSource(previousSource, [
      {
        taskId: "task-1",
        sourceId: previousSource.id,
        sourceType: previousSource.type,
        sourceOrder: 0,
        status: "todo",
        textPreview: "Carry me",
        body: [{ type: "paragraph" }],
      },
    ]);

    storage.moveTasksToSource(["task-1"], currentSource, 3);

    expect(storage.getTask("task-1")).toMatchObject({
      sourceId: currentSource.id,
      sourceType: currentSource.type,
      sourceOrder: 3,
    });
    expect(storage.getTasksForSource(previousSource)).toHaveLength(0);
    expect(storage.getTasksForSource(currentSource)).toMatchObject([
      {
        taskId: "task-1",
        sourceOrder: 3,
      },
    ]);
  });
});

it("rehydrates background tasks by text before assigning IDs, without duplication or status loss", () => {
  const source = { type: "enhanced_note", id: "summary" };
  const initial = normalizeTaskContent(
    md2json("- [ ] Send proposal\n- [ ] Send proposal"),
  )!;
  const saved = extractTasksFromContent(initial, source).map((task, index) => ({
    ...task,
    status: index === 0 ? ("done" as const) : ("in_progress" as const),
    dueDate: "2026-10-01",
  }));
  for (let attempt = 0; attempt < 2; attempt++) {
    const reopened = normalizeTaskContent(
      hydrateTaskContent({
        content: md2json("- [ ] Send proposal\n- [ ] Send proposal"),
        sourceTasks: saved,
        getTask: (id) => saved.find((task) => task.taskId === id) ?? null,
      }),
    )!;
    const actual = extractTasksFromContent(
      reopened,
      source,
      new Map(saved.map((task) => [task.taskId, task])),
    );
    expect(actual).toEqual(saved);
  }
});

it("reuses nested generated task identities without appending duplicate children", () => {
  const source = { type: "enhanced_note", id: "summary" };
  const markdown = "- [ ] Prepare proposal\n  - [ ] Check figures";
  const saved = extractTasksFromContent(
    normalizeTaskContent(md2json(markdown))!,
    source,
  );
  const reopened = normalizeTaskContent(
    hydrateTaskContent({
      content: md2json(markdown),
      sourceTasks: saved,
      getTask: (id) => saved.find((task) => task.taskId === id) ?? null,
    }),
  )!;
  const actual = extractTasksFromContent(reopened, source);
  expect(actual.map((task) => task.taskId)).toEqual(
    saved.map((task) => task.taskId),
  );
  expect(reopened.content).toHaveLength(1);
  expect(reopened.content![0].content).toHaveLength(1);
});
