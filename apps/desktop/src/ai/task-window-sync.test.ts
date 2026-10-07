import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
const mocks = vi.hoisted(() => ({
  listen: vi.fn(),
  emitTo: vi.fn(),
  stop: vi.fn(),
  emit: vi.fn(),
  label: "note-window",
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: mocks.listen,
  emitTo: mocks.emitTo,
  emit: mocks.emit,
}));
vi.mock("@hypr/plugin-windows", () => ({
  getCurrentWebviewWindowLabel: () => mocks.label,
}));
vi.mock("~/services/enhancer", () => ({ getEnhancerService: vi.fn() }));
import { act, render } from "@testing-library/react";
import { createElement } from "react";
import { createStore } from "zustand/vanilla";

import { AITaskWindowSyncBridge, requestMainEnhance } from "./task-window-sync";

describe("detached summary requests", () => {
  let respond: (event: { payload: unknown }) => void;
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.label = "note-window";
    mocks.listen.mockImplementation(async (_event, callback) => {
      respond = callback;
      return mocks.stop;
    });
    mocks.emitTo.mockResolvedValue(undefined);
  });
  afterEach(() => vi.useRealTimers());
  it("waits for the matching start result and removes the listener", async () => {
    const pending = requestMainEnhance("session-1", {
      targetNoteId: "legacy-summary",
    });
    await vi.waitFor(() => expect(mocks.emitTo).toHaveBeenCalled());
    const payload = mocks.emitTo.mock.calls[0][2];
    expect(payload.sourceLabel).toBe("note-window");
    expect(payload.opts).toEqual({ targetNoteId: "legacy-summary" });
    respond({
      payload: {
        requestId: payload.requestId,
        result: { type: "started", noteId: "summary-1" },
      },
    });
    await expect(pending).resolves.toEqual({
      type: "started",
      noteId: "summary-1",
    });
    expect(mocks.stop).toHaveBeenCalledOnce();
  });
  it("propagates source validation errors", async () => {
    const pending = requestMainEnhance("session-1");
    const assertion = expect(pending).rejects.toThrow(
      "Add a note or transcript",
    );
    await vi.waitFor(() => expect(mocks.emitTo).toHaveBeenCalled());
    respond({
      payload: {
        requestId: mocks.emitTo.mock.calls[0][2].requestId,
        error: "Add a note or transcript",
      },
    });
    await assertion;
    expect(mocks.stop).toHaveBeenCalledOnce();
  });
});

it("does not broadcast unrelated task changes and sends the final summary snapshot", async () => {
  mocks.label = "main";
  mocks.emit.mockReset().mockResolvedValue(undefined);
  mocks.listen.mockResolvedValue(() => {});
  const task = {
    taskType: "enhance",
    status: "generating",
    streamedText: "first",
    sessionId: "s1",
  };
  const store = createStore<any>(() => ({
    tasks: { summary: task },
    cancel: vi.fn(),
  }));
  const view = render(createElement(AITaskWindowSyncBridge, { store }));
  expect(mocks.emit).toHaveBeenCalledTimes(1);
  act(() =>
    store.setState({
      tasks: {
        summary: task,
        title: { taskType: "title", streamedText: "title" },
      },
    }),
  );
  expect(mocks.emit).toHaveBeenCalledTimes(1);
  act(() =>
    store.setState({
      tasks: {
        summary: { ...task, status: "success", streamedText: "complete" },
      },
    }),
  );
  expect(mocks.emit).toHaveBeenCalledTimes(2);
  expect(mocks.emit.mock.lastCall?.[1].tasks.summary).toMatchObject({
    status: "success",
    streamedText: "complete",
  });
  view.unmount();
  mocks.label = "note-window";
});

it("broadcasts pruned history, preserves a detached observer until release, and cleans late listeners", async () => {
  vi.useFakeTimers();
  const { createAITaskStore } = await import("~/store/zustand/ai-task");
  const main = createAITaskStore();
  const remote = createAITaskStore();
  const release = remote.getState().retainTask("summary-enhance");
  const callbacks = new Map<string, (event: any) => void>();
  const cleanups: ReturnType<typeof vi.fn>[] = [];
  const resolves: (() => void)[] = [];
  mocks.listen.mockImplementation((event, handler) => {
    callbacks.set(event, handler);
    const cleanup = vi.fn();
    cleanups.push(cleanup);
    return new Promise<() => void>((resolve) => {
      resolves.push(() => resolve(cleanup));
    });
  });
  mocks.emit.mockImplementation(async (_event, payload) =>
    remote.getState().syncRemoteTasks(payload.tasks),
  );
  mocks.label = "main";
  const view = render(createElement(AITaskWindowSyncBridge, { store: main }));
  act(() =>
    main.getState().syncRemoteTask("summary-enhance", {
      taskType: "enhance",
      status: "success",
      streamedText: "complete",
    }),
  );
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5 * 60_000);
  });
  expect(main.getState().tasks).toEqual({});
  expect(remote.getState().tasks["summary-enhance"]?.streamedText).toBe(
    "complete",
  );
  release();
  expect(remote.getState().tasks).toEqual({});
  view.unmount();
  await act(async () => {
    resolves.forEach((resolve) => resolve());
  });
  cleanups.forEach((cleanup) => expect(cleanup).toHaveBeenCalledOnce());
  expect(vi.getTimerCount()).toBe(0);
  mocks.label = "note-window";
  vi.useRealTimers();
});
