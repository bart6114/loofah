import { useMemo } from "react";

import type { TaskStorage } from "@hypr/editor/task-storage";
import {
  createTaskSourceKey,
  isSameTask,
  type TaskRecord,
  type TaskSource,
} from "@hypr/editor/tasks";

import { taskItemToRecord, taskRecordToInput } from "./task-records";

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
  const sourceRequests = new Map<string, number>();
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
    const lifetime = sourceListeners.get(sourceKey);
    if (!lifetime?.size) return Promise.resolve();
    const request = (sourceRequests.get(sourceKey) ?? 0) + 1;
    sourceRequests.set(sourceKey, request);
    return dependencies
      .listTasks(source.type, source.id)
      .then((items) => {
        if (
          sourceListeners.get(sourceKey) !== lifetime ||
          !lifetime.size ||
          sourceRequests.get(sourceKey) !== request
        )
          return;
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
      const key = createTaskSourceKey(source);
      const lifetime = sourceListeners.get(key);
      const request = (sourceRequests.get(key) ?? 0) + 1;
      if (lifetime?.size) sourceRequests.set(key, request);
      const items = await dependencies.listTasks(source.type, source.id);
      if (
        lifetime?.size &&
        sourceListeners.get(key) === lifetime &&
        sourceRequests.get(key) === request &&
        updateSourceSnapshot(source, items)
      ) {
        lifetime.forEach((notify) => notify());
      }
      return items
        .map(taskItemToRecord)
        .filter((task): task is TaskRecord => task !== null);
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
        listeners.add(listener);
        // A bus refresh can only ever *read*: it lands in `updateSourceSnapshot`, which
        // notifies listeners solely when the data actually differs, so our own writes
        // (which echo back as `tasks` events) settle instead of looping.
        sourceBusUnsubscribes.set(
          sourceKey,
          dependencies.subscribeTasksChanged(source, () => {
            if (sourceListeners.get(sourceKey) === listeners)
              void refreshSource(source);
          }),
        );
        void refreshSource(source);
      }

      listeners.add(listener);
      const currentListeners = listeners;
      return () => {
        currentListeners.delete(listener);
        queueMicrotask(() => {
          if (
            currentListeners.size ||
            sourceListeners.get(sourceKey) !== currentListeners
          )
            return;
          sourceListeners.delete(sourceKey);
          sourceRequests.delete(sourceKey);
          sourceBusUnsubscribes.get(sourceKey)?.();
          sourceBusUnsubscribes.delete(sourceKey);
          for (const task of sourceSnapshots.get(sourceKey) ?? []) {
            const cached = taskSnapshots.get(task.taskId);
            if (
              cached?.sourceType === source.type &&
              cached.sourceId === source.id
            ) {
              taskSnapshots.delete(task.taskId);
            }
          }
          sourceSnapshots.delete(sourceKey);
        });
      };
    },
    getTask(taskId) {
      return taskSnapshots.get(taskId) ?? null;
    },
    upsertTasksForSource(source, tasks) {
      const cached = sourceSnapshots.get(createTaskSourceKey(source));
      if (cached && areSameTaskSets(cached, tasks)) return;

      persist(async () => {
        // An editor can mount before its subscription's read settles. Keep metadata
        // absent from Markdown (such as due dates) when that first sync writes tasks.
        const previous =
          cached ??
          (await dependencies.listTasks(source.type, source.id))
            .map(taskItemToRecord)
            .filter((task): task is TaskRecord => task !== null);
        const previousById = new Map(
          previous.map((task) => [task.taskId, task]),
        );
        const next = cached
          ? tasks
          : tasks.map((task) => ({
              ...task,
              dueDate: task.dueDate ?? previousById.get(task.taskId)?.dueDate,
            }));
        if (areSameTaskSets(previous, next)) return;
        await dependencies.replaceTasks(
          source.type,
          source.id,
          next.map(taskRecordToInput),
        );
      }, [source]);
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

function areSameTaskSets(left: TaskRecord[], right: TaskRecord[]) {
  if (left.length !== right.length) {
    return false;
  }

  return left.every((task, index) => isSameTask(task, right[index]!));
}
