import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
const mocks = vi.hoisted(() => ({
  listen: vi.fn(),
  emitTo: vi.fn(),
  stop: vi.fn(),
  emit: vi.fn(),
  label: "note-window",
  service: null as unknown,
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: mocks.listen,
  emitTo: mocks.emitTo,
  emit: mocks.emit,
}));
vi.mock("@hypr/plugin-windows", () => ({
  getCurrentWebviewWindowLabel: () => mocks.label,
}));
vi.mock("~/services/enhancer", () => ({
  getEnhancerService: () => mocks.service,
}));
import { act, render } from "@testing-library/react";
import { createElement } from "react";
import { createStore } from "zustand/vanilla";

import {
  AITaskWindowSyncBridge,
  requestMainEnhance,
  requestMainAutoEnhance,
} from "./task-window-sync";

import {
  beginSummaryActivity,
  useSummaryActivity,
} from "~/session/summary-activity";
import type { VaultScope } from "~/shared/application/vault-scope";

describe("detached summary requests", () => {
  let respond: (event: { payload: unknown }) => void;
  const owner = {
    client: { scope: "local:captured", canEdit: true },
    getSnapshot: () => ({ state: "connected" }),
  } as VaultScope;
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
  async function discover() {
    await vi.waitFor(() => expect(mocks.emitTo).toHaveBeenCalledTimes(1));
    const payload = mocks.emitTo.mock.calls[0][2];
    expect(payload).toMatchObject({
      scope: "local:captured",
      kind: "owner",
      sourceLabel: "note-window",
    });
    respond({
      payload: { requestId: payload.requestId, ownerId: "incarnation-1" },
    });
    await vi.waitFor(() => expect(mocks.emitTo).toHaveBeenCalledTimes(2));
    return mocks.emitTo.mock.calls[1][2];
  }
  it("discovers the exact owner before dispatch and waits for its matching result", async () => {
    const pending = requestMainEnhance(owner, "session-1", {
      targetNoteId: "legacy-summary",
    });
    const payload = await discover();
    expect(payload).toMatchObject({
      kind: "generate",
      ownerId: "incarnation-1",
      opts: { targetNoteId: "legacy-summary" },
    });
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
    expect(mocks.stop).toHaveBeenCalledTimes(2);
  });
  it("keeps automatic requests pending until the captured owner accepts the queue", async () => {
    let finished = false;
    const pending = requestMainAutoEnhance(
      owner,
      "session-1",
      "regenerate",
    ).then(() => {
      finished = true;
    });
    const payload = await discover();
    expect(finished).toBe(false);
    expect(payload.auto).toBe("regenerate");
    respond({ payload: { requestId: payload.requestId, queued: true } });
    await pending;
    expect(finished).toBe(true);
  });
  it("does not dispatch when source ownership changes during discovery", async () => {
    let current = true;
    const pending = requestMainEnhance(
      owner,
      "session-1",
      undefined,
      () => current,
    );
    const rejected = expect(pending).rejects.toThrow("no longer available");
    await vi.waitFor(() => expect(mocks.emitTo).toHaveBeenCalledOnce());
    current = false;
    respond({
      payload: {
        requestId: mocks.emitTo.mock.calls[0][2].requestId,
        ownerId: "incarnation-1",
      },
    });
    await rejected;
    expect(mocks.emitTo).toHaveBeenCalledOnce();
  });
  it("propagates unknown-outcome errors without automatically resubmitting", async () => {
    const pending = requestMainEnhance(owner, "session-1");
    const rejected = expect(pending).rejects.toThrow("Review saved requests");
    const payload = await discover();
    respond({
      payload: { requestId: payload.requestId, error: "Review saved requests" },
    });
    await rejected;
    expect(mocks.emitTo).toHaveBeenCalledTimes(2);
  });
});

it("rejects a generation addressed to an old main owner even when scope strings match", async () => {
  const callbacks = new Map<string, (event: { payload: unknown }) => void>();
  mocks.label = "main";
  mocks.listen.mockImplementation(async (event, callback) => {
    callbacks.set(event, callback);
    return () => {};
  });
  mocks.emitTo.mockReset().mockResolvedValue(undefined);
  const enhance = vi.fn();
  mocks.service = {
    scope: { client: { scope: "local:same" } },
    ownerId: "old",
    assertCurrent: vi.fn(),
    enhance,
  };
  const store = createStore<any>(() => ({ tasks: {}, cancel: vi.fn() }));
  const view = render(createElement(AITaskWindowSyncBridge, { store }));
  await act(async () => {});
  const payload = {
    scope: "local:same",
    sessionId: "same",
    sourceLabel: "note-window",
    requestId: crypto.randomUUID(),
  };
  await act(async () =>
    callbacks.get("hypr:ai-task-enhance")?.({
      payload: { ...payload, kind: "owner" },
    }),
  );
  expect(mocks.emitTo.mock.lastCall?.[2].ownerId).toBe("old");
  const replacement = vi.fn();
  mocks.service = {
    scope: { client: { scope: "local:same" } },
    ownerId: "replacement",
    assertCurrent: vi.fn(),
    enhance: replacement,
  };
  await act(async () =>
    callbacks.get("hypr:ai-task-enhance")?.({
      payload: { ...payload, kind: "generate", ownerId: "old" },
    }),
  );
  expect(enhance).not.toHaveBeenCalled();
  expect(replacement).not.toHaveBeenCalled();
  expect(mocks.emitTo.mock.lastCall?.[2].error).toContain("owner changed");
  view.unmount();
  mocks.service = null;
  mocks.label = "note-window";
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

it("includes activity in initial snapshots and broadcasts queue handoffs", async () => {
  mocks.label = "main";
  mocks.emit.mockReset().mockResolvedValue(undefined);
  mocks.listen.mockResolvedValue(() => {});
  const activity = beginSummaryActivity("locked", "recording");
  const store = createStore<any>(() => ({ tasks: {}, cancel: vi.fn() }));
  const view = render(createElement(AITaskWindowSyncBridge, { store }));
  expect(mocks.emit.mock.lastCall?.[1].summaryActivity).toEqual({
    locked: "recording",
  });
  act(() => activity.setPhase("processing"));
  expect(mocks.emit.mock.lastCall?.[1].summaryActivity).toEqual({
    locked: "processing",
  });
  act(() => activity.finish());
  expect(mocks.emit.mock.lastCall?.[1].summaryActivity).toEqual({});
  view.unmount();
  mocks.label = "note-window";
});

it("hydrates standalone activity before allowing summary edits", async () => {
  mocks.label = "note-window";
  const callbacks = new Map<string, (event: any) => void>();
  mocks.listen.mockImplementation(async (event, callback) => {
    callbacks.set(event, callback);
    return () => {};
  });
  const syncRemoteTasks = vi.fn();
  const store = createStore<any>(() => ({ tasks: {}, syncRemoteTasks }));
  const view = render(createElement(AITaskWindowSyncBridge, { store }));
  await act(async () => {});
  act(() =>
    callbacks.get("hypr:ai-task-sync")?.({
      payload: {
        sourceLabel: "main",
        tasks: {},
        summaryActivity: { locked: "processing" },
      },
    }),
  );
  expect(useSummaryActivity.getState()).toMatchObject({
    remoteReady: true,
    remoteSessions: { locked: "processing" },
  });
  act(() =>
    callbacks.get("hypr:ai-task-sync")?.({
      payload: {
        sourceLabel: "main",
        tasks: {},
        summaryActivity: {},
      },
    }),
  );
  expect(useSummaryActivity.getState().remoteSessions).toEqual({});
  view.unmount();
});
