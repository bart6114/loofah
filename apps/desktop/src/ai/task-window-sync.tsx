import { emit, emitTo, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect } from "react";

import { getCurrentWebviewWindowLabel } from "@hypr/plugin-windows";

import { type EnhanceResult, getEnhancerService } from "~/services/enhancer";
import type { AITaskStore } from "~/store/zustand/ai-task";
import type { RemoteTaskState, TaskState } from "~/store/zustand/ai-task/tasks";

const TASK_SYNC_EVENT = "hypr:ai-task-sync";
const TASK_SYNC_REQUEST_EVENT = "hypr:ai-task-sync-request";
const TASK_CANCEL_EVENT = "hypr:ai-task-cancel";
const TASK_ENHANCE_EVENT = "hypr:ai-task-enhance";
const TASK_ENHANCE_RESULT_EVENT = "hypr:ai-task-enhance-result";
type TaskEnhanceResultPayload = {
  requestId: string;
  result?: EnhanceResult;
  error?: string;
};

type TaskSyncPayload = {
  sourceLabel: string;
  tasks: Record<string, RemoteTaskState>;
};

type TaskSyncRequestPayload = {
  sourceLabel: string;
};

type TaskCancelPayload = {
  taskId: string;
};

type TaskEnhancePayload = {
  sessionId: string;
  requestId?: string;
  sourceLabel?: string;
  auto?: "regenerate" | "if_empty";
  opts?: {
    isAuto?: boolean;
    targetNoteId?: string;
  };
};

export function isMainAITaskHostWindow() {
  return getCurrentWebviewWindowLabel() === "main";
}

export async function requestMainAITaskCancel(taskId: string) {
  await emitTo("main", TASK_CANCEL_EVENT, {
    taskId,
  } satisfies TaskCancelPayload);
}

export async function requestMainEnhance(
  sessionId: string,
  opts?: TaskEnhancePayload["opts"],
) {
  const requestId = crypto.randomUUID();
  const sourceLabel = getCurrentWebviewWindowLabel();
  let active = true;
  let unlisten: UnlistenFn | undefined;
  let timeout: ReturnType<typeof setTimeout> | undefined;
  try {
    return await new Promise<EnhanceResult>((resolve, reject) => {
      timeout = setTimeout(
        () => reject(new Error("Summary did not start. Please try again.")),
        30_000,
      );
      void listen<TaskEnhanceResultPayload>(
        TASK_ENHANCE_RESULT_EVENT,
        ({ payload }) => {
          if (payload.requestId !== requestId) return;
          if (payload.error) reject(new Error(payload.error));
          else if (payload.result) resolve(payload.result);
        },
      )
        .then(async (stop) => {
          if (!active) {
            stop();
            return;
          }
          unlisten = stop;
          await emitTo("main", TASK_ENHANCE_EVENT, {
            sessionId,
            opts,
            requestId,
            sourceLabel,
          } satisfies TaskEnhancePayload);
        })
        .catch(reject);
    });
  } finally {
    active = false;
    if (timeout) clearTimeout(timeout);
    unlisten?.();
  }
}

export async function requestMainAutoEnhance(
  sessionId: string,
  auto: NonNullable<TaskEnhancePayload["auto"]>,
) {
  await emitTo("main", TASK_ENHANCE_EVENT, {
    sessionId,
    auto,
  } satisfies TaskEnhancePayload);
}

export function AITaskWindowSyncBridge({ store }: { store: AITaskStore }) {
  const isMain = isMainAITaskHostWindow();

  if (isMain) {
    return <MainAITaskWindowSyncBridge store={store} />;
  }

  return <RemoteAITaskWindowSyncBridge store={store} />;
}

function MainAITaskWindowSyncBridge({ store }: { store: AITaskStore }) {
  useEffect(() => {
    const sourceLabel = getCurrentWebviewWindowLabel();
    let active = true;
    let syncRequestUnlisten: UnlistenFn | null = null;
    let cancelUnlisten: UnlistenFn | null = null;
    let enhanceUnlisten: UnlistenFn | null = null;

    const emitSnapshot = () => {
      void emit(TASK_SYNC_EVENT, {
        sourceLabel,
        tasks: serializeEnhanceTasks(store.getState().tasks),
      } satisfies TaskSyncPayload);
    };

    const unsubscribe = store.subscribe(emitSnapshot);
    emitSnapshot();

    void listen<TaskSyncRequestPayload>(TASK_SYNC_REQUEST_EVENT, (event) => {
      if (!active || !isTaskSyncRequestPayload(event.payload)) {
        return;
      }

      void emitTo(event.payload.sourceLabel, TASK_SYNC_EVENT, {
        sourceLabel,
        tasks: serializeEnhanceTasks(store.getState().tasks),
      } satisfies TaskSyncPayload);
    }).then((unlisten) => {
      if (active) {
        syncRequestUnlisten = unlisten;
      } else {
        unlisten();
      }
    });

    void listen<TaskCancelPayload>(TASK_CANCEL_EVENT, (event) => {
      if (!active || !isTaskCancelPayload(event.payload)) {
        return;
      }

      store.getState().cancel(event.payload.taskId);
    }).then((unlisten) => {
      if (active) {
        cancelUnlisten = unlisten;
      } else {
        unlisten();
      }
    });

    void listen<TaskEnhancePayload>(TASK_ENHANCE_EVENT, (event) => {
      if (!active || !isTaskEnhancePayload(event.payload)) {
        return;
      }

      const { sessionId, auto, opts, requestId, sourceLabel } = event.payload;
      void (async () => {
        const service = getEnhancerService();
        if (!service) {
          throw new Error("Summary service is unavailable. Please try again.");
        }

        if (auto === "regenerate") {
          await service.resetEnhanceTasks(sessionId);
          service.queueAutoEnhance(sessionId);
        } else if (auto === "if_empty") {
          await service.queueAutoEnhanceIfSummaryEmpty(sessionId);
        } else {
          const result = await service.enhance(sessionId, opts);
          if (requestId && sourceLabel) {
            await emitTo(sourceLabel, TASK_ENHANCE_RESULT_EVENT, {
              requestId,
              result,
            } satisfies TaskEnhanceResultPayload);
          }
        }
      })().catch((error) => {
        console.error("[enhancer] remote enhancement failed", error);
        if (requestId && sourceLabel) {
          void emitTo(sourceLabel, TASK_ENHANCE_RESULT_EVENT, {
            requestId,
            error: error instanceof Error ? error.message : String(error),
          } satisfies TaskEnhanceResultPayload);
        }
      });
    }).then((unlisten) => {
      if (active) {
        enhanceUnlisten = unlisten;
      } else {
        unlisten();
      }
    });

    return () => {
      active = false;
      unsubscribe();
      syncRequestUnlisten?.();
      cancelUnlisten?.();
      enhanceUnlisten?.();
    };
  }, [store]);

  return null;
}

function RemoteAITaskWindowSyncBridge({ store }: { store: AITaskStore }) {
  useEffect(() => {
    const sourceLabel = getCurrentWebviewWindowLabel();
    let active = true;
    let syncUnlisten: UnlistenFn | null = null;

    void listen<TaskSyncPayload>(TASK_SYNC_EVENT, (event) => {
      if (
        !active ||
        !isTaskSyncPayload(event.payload) ||
        event.payload.sourceLabel === sourceLabel
      ) {
        return;
      }

      store.getState().syncRemoteTasks(event.payload.tasks);
    }).then((unlisten) => {
      if (active) {
        syncUnlisten = unlisten;
        void emitTo("main", TASK_SYNC_REQUEST_EVENT, {
          sourceLabel,
        } satisfies TaskSyncRequestPayload);
      } else {
        unlisten();
      }
    });

    return () => {
      active = false;
      syncUnlisten?.();
    };
  }, [store]);

  return null;
}

function serializeEnhanceTasks(tasks: Record<string, TaskState>) {
  return Object.fromEntries(
    Object.entries(tasks)
      .filter(([, task]) => task.taskType === "enhance")
      .map(([taskId, task]) => [
        taskId,
        {
          taskType: task.taskType,
          status: task.status,
          streamedText: task.streamedText,
          error: task.error
            ? { name: task.error.name, message: task.error.message }
            : undefined,
          currentStep: task.currentStep,
          sessionId: task.sessionId,
        } satisfies RemoteTaskState,
      ]),
  );
}

function isTaskSyncPayload(payload: unknown): payload is TaskSyncPayload {
  if (!payload || typeof payload !== "object") {
    return false;
  }

  const candidate = payload as Partial<TaskSyncPayload>;
  return (
    typeof candidate.sourceLabel === "string" &&
    Boolean(candidate.tasks) &&
    typeof candidate.tasks === "object"
  );
}

function isTaskSyncRequestPayload(
  payload: unknown,
): payload is TaskSyncRequestPayload {
  if (!payload || typeof payload !== "object") {
    return false;
  }

  return (
    typeof (payload as Partial<TaskSyncRequestPayload>).sourceLabel === "string"
  );
}

function isTaskCancelPayload(payload: unknown): payload is TaskCancelPayload {
  if (!payload || typeof payload !== "object") {
    return false;
  }

  return typeof (payload as Partial<TaskCancelPayload>).taskId === "string";
}

function isTaskEnhancePayload(payload: unknown): payload is TaskEnhancePayload {
  if (!payload || typeof payload !== "object") {
    return false;
  }

  const candidate = payload as Partial<TaskEnhancePayload>;
  return typeof candidate.sessionId === "string";
}
