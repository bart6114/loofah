import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { createAITaskStore } from ".";
import { TASK_CONFIGS } from "./task-configs";

vi.mock("~/settings/queries", () => ({
  getStoredSettingValues: async () => ({ values: {}, hasValues: new Set() }),
}));
const original = { ...TASK_CONFIGS.enhance };
beforeEach(() => {
  vi.useFakeTimers();
  TASK_CONFIGS.enhance.transformArgs = vi.fn(async () => ({}) as any);
  TASK_CONFIGS.enhance.transforms = [];
  TASK_CONFIGS.enhance.executeWorkflow = vi.fn(async function* () {
    yield { type: "text-delta", text: "summary" } as any;
  });
  TASK_CONFIGS.enhance.onSuccess = vi.fn(async () => {});
});
afterEach(() => {
  Object.assign(TASK_CONFIGS.enhance, original);
  vi.useRealTimers();
});
const terminal = (streamedText = "summary") => ({
  taskType: "enhance" as const,
  status: "success" as const,
  streamedText,
});

it("bounds 100 completed jobs after consumers release them, preserving completion callbacks", async () => {
  const store = createAITaskStore();
  const onComplete = vi.fn();
  for (let i = 0; i < 100; i++) {
    const id = `session-${i}-enhance` as const;
    const release = store.getState().retainTask(id);
    await store.getState().generate(id, {
      model: {} as any,
      taskType: "enhance",
      args: { sessionId: `session-${i}` },
      onComplete,
    });
    expect(store.getState().tasks[id]?.status).toBe("success");
    release();
  }
  expect(onComplete).toHaveBeenCalledTimes(100);
  expect(Object.keys(store.getState().tasks)).toHaveLength(32);
  expect(store.getState().tasks["session-0-enhance"]).toBeUndefined();
  expect(store.getState().tasks["session-99-enhance"]).toBeDefined();
});

it("expires terminal history without starting another job", async () => {
  const store = createAITaskStore();
  store.getState().syncRemoteTask("a-enhance", terminal());
  await vi.advanceTimersByTimeAsync(5 * 60_000);
  expect(store.getState().tasks).toEqual({});
  expect(vi.getTimerCount()).toBe(0);
});

it("bounds estimated text storage while exempting active and observed work", async () => {
  const store = createAITaskStore();
  const release = store.getState().retainTask("visible-enhance");
  const releaseOther = store.getState().retainTask("visible-enhance");
  store
    .getState()
    .syncRemoteTask("visible-enhance", terminal("v".repeat(2 * 1024 * 1024)));
  store
    .getState()
    .syncRemoteTask("active-enhance", { ...terminal(), status: "generating" });
  for (let i = 0; i < 20; i++)
    store
      .getState()
      .syncRemoteTask(`large-${i}-enhance`, terminal("a".repeat(256 * 1024)));
  const history = Object.entries(store.getState().tasks).filter(
    ([id]) => !["visible-enhance", "active-enhance"].includes(id),
  );
  expect(
    history.reduce((sum, [, task]) => sum + 2 * task.streamedText.length, 0),
  ).toBeLessThanOrEqual(2 * 1024 * 1024);
  await vi.advanceTimersByTimeAsync(5 * 60_000);
  release();
  expect(store.getState().tasks["visible-enhance"]).toBeDefined();
  releaseOther();
  releaseOther();
  expect(Object.keys(store.getState().tasks)).toEqual(["active-enhance"]);
});

it("retains observed remote completions until release but applies explicit reset and cancellation", () => {
  const store = createAITaskStore();
  const release = store.getState().retainTask("a-enhance");
  store.getState().syncRemoteTasks({ "a-enhance": terminal() });
  store.getState().syncRemoteTasks({});
  expect(store.getState().tasks["a-enhance"]?.streamedText).toBe("summary");
  store
    .getState()
    .syncRemoteTasks({ "a-enhance": { ...terminal(""), status: "idle" } });
  expect(store.getState().tasks["a-enhance"]?.streamedText).toBe("");
  store.getState().syncRemoteTasks({});
  release();
  expect(store.getState().tasks).toEqual({});
});

it("prunes errors, cancelled and reset entries and stops its scheduler on teardown", async () => {
  const store = createAITaskStore();
  for (let i = 0; i < 100; i++) {
    const id = `job-${i}-enhance` as const;
    store.getState().syncRemoteTask(id, {
      ...terminal(),
      status: "error",
      error: { message: "failed" },
    });
    if (i % 3 === 0) store.getState().cancel(id);
    if (i % 3 === 1) store.getState().reset(id);
  }
  expect(Object.keys(store.getState().tasks).length).toBeLessThanOrEqual(32);
  const stop = store.getState().startHistoryMaintenance();
  stop();
  expect(vi.getTimerCount()).toBe(0);
  const stopAgain = store.getState().startHistoryMaintenance();
  await vi.advanceTimersByTimeAsync(5 * 60_000);
  expect(store.getState().tasks).toEqual({});
  stopAgain();
});
