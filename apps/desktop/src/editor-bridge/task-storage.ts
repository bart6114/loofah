import { useMemo } from "react";

import type { JSONContent } from "@hypr/editor/note";
import type { TaskStorage } from "@hypr/editor/task-storage";
import {
  createTaskSourceKey,
  isSameTask,
  type TaskRecord,
  type TaskSource,
} from "@hypr/editor/tasks";

import { subscribeIndexChanged } from "~/shared/index-query";
import { enqueueDatabaseWrite } from "~/shared/write-queue";
import {
  commands,
  type Result,
  type TaskInput,
  type TaskItem,
} from "~/types/tauri.gen";

export type { TaskInput, TaskItem };

export type TaskStorageDependencies = {
  listTasks: (sourceType: string, sourceId: string) => Promise<TaskItem[]>;
  replaceTasks: (
    sourceType: string,
    sourceId: string,
    tasks: TaskInput[],
  ) => Promise<void>;
  removeTasks: (
    sourceType: string,
    sourceId: string,
    taskIds: string[],
  ) => Promise<void>;
  moveTasks: (
    taskIds: string[],
    sourceType: string,
    sourceId: string,
    insertionOrder: number,
  ) => Promise<void>;
  enqueueWrite: (key: string, write: () => Promise<void>) => Promise<void>;
  subscribeTasksChanged: (
    source: TaskSource,
    onChange: () => void,
  ) => () => void;
};

// Notes and summaries carry their session ID; template document sources need a wider subscription.
function taskEventIds(source: TaskSource): readonly string[] | undefined {
  return ["session_raw_note", "session_summary"].includes(source.type)
    ? [source.id]
    : undefined;
}

const emptyTasks: TaskRecord[] = [];

function unwrap<T>(result: Result<T, string>): T {
  if (result.status === "error") {
    throw new Error(result.error);
  }
  return result.data;
}

const defaultDependencies: TaskStorageDependencies = {
  listTasks: async (sourceType, sourceId) =>
    unwrap(await commands.sessionListTasks(sourceType, sourceId)),
  replaceTasks: async (sourceType, sourceId, tasks) => {
    unwrap(await commands.sessionReplaceTasks(sourceType, sourceId, tasks));
  },
  removeTasks: async (sourceType, sourceId, taskIds) => {
    unwrap(await commands.sessionRemoveTasks(sourceType, sourceId, taskIds));
  },
  moveTasks: async (taskIds, sourceType, sourceId, insertionOrder) => {
    unwrap(
      await commands.sessionMoveTasks(
        taskIds,
        sourceType,
        sourceId,
        insertionOrder,
      ),
    );
  },
  enqueueWrite: enqueueDatabaseWrite,
  subscribeTasksChanged: (source, onChange) =>
    subscribeIndexChanged("tasks", onChange, taskEventIds(source)),
};

export function useStoreBackedTaskStorage(): TaskStorage {
  return useMemo(() => createStoreBackedTaskStorage(), []);
}

// Tasks are file-canonical in `sessions/<id>/tasks.json`, read and written through the
// session-store commands. A source is fetched on first subscribe, refetched after our own
// writes land, and refetched whenever the index bus reports a `tasks` change -- the last one
// matters because `replace_tasks` is a whole-source replace, so a second window (or an
// external editor) working from a stale snapshot would otherwise revert the other window's
// edits on its next write.
export function createStoreBackedTaskStorage(
  dependencies: TaskStorageDependencies = defaultDependencies,
): TaskStorage {
  const sourceSnapshots = new Map<string, TaskRecord[]>();
  const taskSnapshots = new Map<string, TaskRecord>();
  const sourceListeners = new Map<string, Set<() => void>>();
  const sourceBusUnsubscribes = new Map<string, () => void>();

  const updateSourceSnapshot = (
    source: TaskSource,
    items: TaskItem[],
  ): boolean => {
    const sourceKey = createTaskSourceKey(source);
    const previousTasks = sourceSnapshots.get(sourceKey) ?? emptyTasks;
    const nextTasks = items
      .map(taskItemToRecord)
      .filter((task): task is TaskRecord => task !== null);
    if (areSameTaskSets(previousTasks, nextTasks)) {
      return false;
    }

    sourceSnapshots.set(sourceKey, nextTasks);
    const nextTaskIds = new Set(nextTasks.map((task) => task.taskId));
    previousTasks.forEach((task) => {
      const cachedTask = taskSnapshots.get(task.taskId);
      if (
        !nextTaskIds.has(task.taskId) &&
        cachedTask?.sourceType === source.type &&
        cachedTask.sourceId === source.id
      ) {
        taskSnapshots.delete(task.taskId);
      }
    });
    nextTasks.forEach((task) => {
      const previousTask = taskSnapshots.get(task.taskId);
      taskSnapshots.set(
        task.taskId,
        previousTask && isSameTask(previousTask, task) ? previousTask : task,
      );
    });
    return true;
  };

  const refreshSource = (source: TaskSource): Promise<void> => {
    const sourceKey = createTaskSourceKey(source);
    return dependencies
      .listTasks(source.type, source.id)
      .then((items) => {
        if (updateSourceSnapshot(source, items)) {
          sourceListeners.get(sourceKey)?.forEach((notify) => notify());
        }
      })
      .catch((error) => {
        console.error(`[tasks] failed to load ${sourceKey}`, error);
      });
  };

  const persist = (
    write: () => Promise<void>,
    affectedSources: TaskSource[],
  ) => {
    void dependencies
      .enqueueWrite("tasks", write)
      .then(() => {
        affectedSources.forEach((source) => void refreshSource(source));
      })
      .catch((error) => {
        console.error("[tasks] failed to persist task changes", error);
      });
  };

  return {
    async loadSource(source) {
      const items = await dependencies.listTasks(source.type, source.id);
      if (updateSourceSnapshot(source, items)) {
        sourceListeners
          .get(createTaskSourceKey(source))
          ?.forEach((notify) => notify());
      }
    },
    getTasksForSource(source) {
      return sourceSnapshots.get(createTaskSourceKey(source)) ?? emptyTasks;
    },
    subscribeSource(source, listener) {
      const sourceKey = createTaskSourceKey(source);
      let listeners = sourceListeners.get(sourceKey);
      if (!listeners) {
        listeners = new Set();
        sourceListeners.set(sourceKey, listeners);
        // A bus refresh can only ever *read*: it lands in `updateSourceSnapshot`, which
        // notifies listeners solely when the data actually differs, so our own writes
        // (which echo back as `tasks` events) settle instead of looping.
        sourceBusUnsubscribes.set(
          sourceKey,
          dependencies.subscribeTasksChanged(source, () => {
            void refreshSource(source);
          }),
        );
        void refreshSource(source);
      }

      listeners.add(listener);
      const currentListeners = listeners;
      return () => {
        currentListeners.delete(listener);
        if (
          currentListeners.size === 0 &&
          sourceListeners.get(sourceKey) === currentListeners
        ) {
          sourceListeners.delete(sourceKey);
          sourceBusUnsubscribes.get(sourceKey)?.();
          sourceBusUnsubscribes.delete(sourceKey);
        }
      };
    },
    getTask(taskId) {
      return taskSnapshots.get(taskId) ?? null;
    },
    upsertTasksForSource(source, tasks) {
      const currentTasks =
        sourceSnapshots.get(createTaskSourceKey(source)) ?? emptyTasks;
      if (areSameTaskSets(currentTasks, tasks)) {
        return;
      }

      persist(
        () =>
          dependencies.replaceTasks(
            source.type,
            source.id,
            tasks.map(taskRecordToInput),
          ),
        [source],
      );
    },
    removeTasksForSource(source, taskIds) {
      if (taskIds.length === 0) {
        return;
      }

      persist(
        () => dependencies.removeTasks(source.type, source.id, taskIds),
        [source],
      );
    },
    moveTasksToSource(taskIds, nextSource, insertionOrder) {
      if (taskIds.length === 0) {
        return;
      }

      const affectedSources = new Map<string, TaskSource>([
        [createTaskSourceKey(nextSource), nextSource],
      ]);
      taskIds.forEach((taskId) => {
        const task = taskSnapshots.get(taskId);
        if (task) {
          const source = { type: task.sourceType, id: task.sourceId };
          affectedSources.set(createTaskSourceKey(source), source);
        }
      });

      persist(
        () =>
          dependencies.moveTasks(
            taskIds,
            nextSource.type,
            nextSource.id,
            insertionOrder,
          ),
        [...affectedSources.values()],
      );
    },
  };
}

function taskRecordToInput(task: TaskRecord): TaskInput {
  return {
    id: task.taskId,
    source_order: task.sourceOrder,
    status: task.status,
    text: task.textPreview,
    body: task.body as TaskInput["body"],
    due_at: task.dueDate ?? "",
  };
}

function taskItemToRecord(item: TaskItem): TaskRecord | null {
  const sourceOrder = Number(item.source_order);
  if (
    !item.id ||
    !item.source_id ||
    !item.source_type ||
    !Number.isFinite(sourceOrder) ||
    (item.status !== "todo" &&
      item.status !== "in_progress" &&
      item.status !== "done")
  ) {
    return null;
  }

  const body = parseTaskBody(item.body, item.text);
  return {
    taskId: item.id,
    sourceId: item.source_id,
    sourceType: item.source_type,
    sourceOrder,
    status: item.status,
    textPreview: item.text || getTextPreview(body),
    body,
    dueDate: item.due_at || undefined,
  };
}

function areSameTaskSets(left: TaskRecord[], right: TaskRecord[]) {
  if (left.length !== right.length) {
    return false;
  }

  return left.every((task, index) => isSameTask(task, right[index]!));
}

function parseTaskBody(body: unknown, legacyText: unknown): JSONContent[] {
  if (Array.isArray(body)) {
    return body as JSONContent[];
  }

  if (typeof legacyText === "string" && legacyText) {
    return [
      {
        type: "paragraph",
        content: [{ type: "text", text: legacyText }],
      },
    ];
  }

  return [{ type: "paragraph" }];
}

function getTextPreview(body: JSONContent[]): string {
  const firstParagraph = body.find((node) => node.type === "paragraph");
  return getNodeText(firstParagraph).trim();
}

function getNodeText(node: JSONContent | undefined): string {
  if (!node) {
    return "";
  }

  if (typeof node.text === "string") {
    return node.text;
  }

  return (node.content ?? []).map((child) => getNodeText(child)).join(" ");
}
