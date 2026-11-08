import { emit, emitTo, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect } from "react";

import { getCurrentWebviewWindowLabel } from "@hypr/plugin-windows";

import { type EnhanceResult, getEnhancerService } from "~/services/enhancer";
import {
  useSummaryActivity,
  type SummaryActivity,
} from "~/session/summary-activity";
import type { VaultScope } from "~/shared/application/vault-scope";
import { createAsyncListenerScope } from "~/shared/async-listener-scope";
import type { AITaskStore } from "~/store/zustand/ai-task";
import type { RemoteTaskState, TaskState } from "~/store/zustand/ai-task/tasks";

const TASK_SYNC_EVENT = "hypr:ai-task-sync";
const TASK_SYNC_REQUEST_EVENT = "hypr:ai-task-sync-request";
const TASK_CANCEL_EVENT = "hypr:ai-task-cancel";
const TASK_ENHANCE_EVENT = "hypr:ai-task-enhance";
const TASK_ENHANCE_RESULT_EVENT = "hypr:ai-task-enhance-result";
type TaskEnhanceResultPayload = {
  requestId: string;
  ownerId?: string;
  result?: EnhanceResult;
  queued?: boolean;
  error?: string;
};

type TaskSyncPayload = {
  sourceLabel: string;
  tasks: Record<string, RemoteTaskState>;
  summaryActivity: Record<string, SummaryActivity>;
};

type TaskSyncRequestPayload = {
  sourceLabel: string;
};

type TaskCancelPayload = {
  taskId: string;
};

type TaskEnhancePayload = {
  scope: string;
  kind: "owner" | "generate";
  ownerId?: string;
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
  scope: VaultScope,
  sessionId: string,
  opts?: TaskEnhancePayload["opts"],
  current = () => true,
) {
  const response = await requestEnhance(scope, sessionId, { opts }, current);
  if (!response.result)
    throw new Error("Summary did not start. Please try again.");
  return response.result;
}

async function requestEnhance(
  scope: VaultScope,
  sessionId: string,
  request: Pick<TaskEnhancePayload, "auto" | "opts">,
  current: () => boolean,
) {
  const check = () => {
    if (
      !current() ||
      scope.getSnapshot().state !== "connected" ||
      !scope.client.canEdit
    )
      throw new Error("The summary owner is no longer available.");
  };
  check();
  const owner = await exchangeEnhance(
    scope.client.scope,
    sessionId,
    { kind: "owner" },
    check,
  );
  check();
  if (!owner.ownerId)
    throw new Error("The summary owner could not be confirmed.");
  const response = await exchangeEnhance(
    scope.client.scope,
    sessionId,
    { kind: "generate", ownerId: owner.ownerId, ...request },
    check,
  );
  check();
  return response;
}

async function exchangeEnhance(
  scope: string,
  sessionId: string,
  request: Pick<TaskEnhancePayload, "kind" | "ownerId" | "auto" | "opts">,
  check: () => void,
) {
  const requestId = crypto.randomUUID();
  const sourceLabel = getCurrentWebviewWindowLabel();
  let active = true;
  let unlisten: UnlistenFn | undefined;
  let timeout: ReturnType<typeof setTimeout> | undefined;
  try {
    return await new Promise<TaskEnhanceResultPayload>((resolve, reject) => {
      timeout = setTimeout(
        () =>
          reject(
            new Error(
              "Summary did not start. Review saved requests before trying again.",
            ),
          ),
        30_000,
      );
      void listen<TaskEnhanceResultPayload>(
        TASK_ENHANCE_RESULT_EVENT,
        ({ payload }) => {
          if (payload.requestId !== requestId) return;
          try {
            check();
            if (payload.error) reject(new Error(payload.error));
            else if (payload.ownerId || payload.result || payload.queued)
              resolve(payload);
          } catch (error) {
            reject(error);
          }
        },
      )
        .then(async (stop) => {
          if (!active) {
            stop();
            return;
          }
          unlisten = stop;
          check();
          await emitTo("main", TASK_ENHANCE_EVENT, {
            scope,
            sessionId,
            ...request,
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
  scope: VaultScope,
  sessionId: string,
  auto: NonNullable<TaskEnhancePayload["auto"]>,
  current = () => true,
) {
  await requestEnhance(scope, sessionId, { auto }, current);
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
    const scope = createAsyncListenerScope();

    const emitSnapshot = () => {
      void emit(TASK_SYNC_EVENT, {
        sourceLabel,
        tasks: serializeEnhanceTasks(store.getState().tasks),
        summaryActivity: useSummaryActivity.getState().sessions,
      } satisfies TaskSyncPayload);
    };

    let previousTasks = Object.values(store.getState().tasks).filter(
      (task) => task.taskType === "enhance",
    );
    const unsubscribe = store.subscribe((state) => {
      const nextTasks = Object.values(state.tasks).filter(
        (task) => task.taskType === "enhance",
      );
      if (
        nextTasks.length === previousTasks.length &&
        nextTasks.every((task, index) => task === previousTasks[index])
      )
        return;
      previousTasks = nextTasks;
      emitSnapshot();
    });
    const unsubscribeActivity = useSummaryActivity.subscribe(emitSnapshot);
    emitSnapshot();

    void scope
      .add(() =>
        listen<TaskSyncRequestPayload>(TASK_SYNC_REQUEST_EVENT, (event) => {
          if (!scope.active || !isTaskSyncRequestPayload(event.payload)) {
            return;
          }

          void emitTo(event.payload.sourceLabel, TASK_SYNC_EVENT, {
            sourceLabel,
            tasks: serializeEnhanceTasks(store.getState().tasks),
            summaryActivity: useSummaryActivity.getState().sessions,
          } satisfies TaskSyncPayload);
        }),
      )
      .catch((error) =>
        console.error("[ai-sync] listener setup failed", error),
      );

    void scope
      .add(() =>
        listen<TaskCancelPayload>(TASK_CANCEL_EVENT, (event) => {
          if (!scope.active || !isTaskCancelPayload(event.payload)) {
            return;
          }

          store.getState().cancel(event.payload.taskId);
        }),
      )
      .catch((error) =>
        console.error("[ai-sync] listener setup failed", error),
      );

    void scope
      .add(() =>
        listen<TaskEnhancePayload>(TASK_ENHANCE_EVENT, (event) => {
          if (!scope.active || !isTaskEnhancePayload(event.payload)) {
            return;
          }

          const { sessionId, auto, opts, requestId, sourceLabel } =
            event.payload;
          void (async () => {
            const service = getEnhancerService();
            if (
              !service ||
              service.scope.client.scope !== event.payload.scope
            ) {
              throw new Error("The summary owner is no longer available.");
            }
            service.assertCurrent();
            if (event.payload.kind === "owner") {
              if (requestId && sourceLabel)
                await emitTo(sourceLabel, TASK_ENHANCE_RESULT_EVENT, {
                  requestId,
                  ownerId: service.ownerId,
                } satisfies TaskEnhanceResultPayload);
              return;
            }
            if (service.ownerId !== event.payload.ownerId)
              throw new Error(
                "The summary owner changed. Review saved requests before trying again.",
              );

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
            if (auto && requestId && sourceLabel) {
              await emitTo(sourceLabel, TASK_ENHANCE_RESULT_EVENT, {
                requestId,
                queued: true,
              } satisfies TaskEnhanceResultPayload);
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
        }),
      )
      .catch((error) =>
        console.error("[ai-sync] listener setup failed", error),
      );

    return () => {
      scope.dispose();
      unsubscribe();
      unsubscribeActivity();
    };
  }, [store]);

  return null;
}

function RemoteAITaskWindowSyncBridge({ store }: { store: AITaskStore }) {
  useEffect(() => {
    const sourceLabel = getCurrentWebviewWindowLabel();
    const scope = createAsyncListenerScope();

    void scope
      .add(() =>
        listen<TaskSyncPayload>(TASK_SYNC_EVENT, (event) => {
          if (
            !scope.active ||
            !isTaskSyncPayload(event.payload) ||
            event.payload.sourceLabel === sourceLabel
          ) {
            return;
          }

          store.getState().syncRemoteTasks(event.payload.tasks);
          useSummaryActivity.setState({
            remoteSessions: event.payload.summaryActivity,
            remoteReady: true,
          });
        }),
      )
      .then(() => {
        if (scope.active) {
          void emitTo("main", TASK_SYNC_REQUEST_EVENT, {
            sourceLabel,
          } satisfies TaskSyncRequestPayload);
        }
      })
      .catch((error) =>
        console.error("[ai-sync] listener setup failed", error),
      );

    return () => {
      scope.dispose();
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
          completedAt: task.completedAt,
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
    typeof candidate.tasks === "object" &&
    Boolean(candidate.summaryActivity) &&
    typeof candidate.summaryActivity === "object" &&
    Object.values(candidate.summaryActivity!).every(
      (phase) => phase === "recording" || phase === "processing",
    )
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
  return (
    typeof candidate.sessionId === "string" &&
    typeof candidate.scope === "string" &&
    candidate.scope.length > 0 &&
    (candidate.kind === "owner" ||
      (candidate.kind === "generate" &&
        typeof candidate.ownerId === "string")) &&
    typeof candidate.requestId === "string" &&
    typeof candidate.sourceLabel === "string"
  );
}
