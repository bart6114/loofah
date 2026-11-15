import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  update: vi.fn(),
  show: vi.fn(),
  hide: vi.fn(),
  open: vi.fn(),
  notify: vi.fn(),
  handlers: new Map<string, (event: { payload: unknown }) => void>(),
  meeting: {
    live: { loading: false, status: "inactive", finalizingBySession: {} },
  },
  meetingListeners: new Set<() => void>(),
  config: {
    dictation_enabled: true,
    dictation_shortcut: "Ctrl+Shift+D",
    dictation_trigger_mode: "hold",
    floating_bar_opacity: 0.8,
  },
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
vi.mock("@hypr/plugin-windows", () => ({
  commands: {
    floatingBarUpdate: mocks.update,
    floatingBarShow: mocks.show,
    floatingBarHide: mocks.hide,
    windowShow: mocks.open,
  },
}));
vi.mock("@hypr/plugin-notification", () => ({
  commands: { showNotification: mocks.notify },
}));
vi.mock("@hypr/plugin-permissions", () => ({
  commands: {
    openPermission: vi.fn(),
    checkPermission: vi.fn(),
    requestPermission: vi.fn(),
  },
}));
vi.mock("~/shared/config", () => ({ useConfigValues: () => mocks.config }));
vi.mock("~/store/zustand/listener/instance", () => ({
  listenerStore: {
    getState: () => mocks.meeting,
    subscribe: (handler: () => void) => {
      mocks.meetingListeners.add(handler);
      return () => mocks.meetingListeners.delete(handler);
    },
  },
}));
vi.mock("~/store/zustand/tabs", () => ({
  useTabs: { getState: () => ({ openNew: mocks.open }) },
}));

import { DictationHost } from "./host";
import { useDictationShortcut } from "./state";

const idle = {
  attemptId: 0,
  revision: 0,
  phase: "idle",
  amplitude: 0,
  problem: null,
  canRetry: false,
  canCopy: false,
  busy: false,
};
function mount() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <DictationHost />
    </QueryClientProvider>,
  );
}
async function emit(name: string, payload: unknown = {}) {
  await act(async () => {
    mocks.handlers.get(name)?.({ payload });
  });
}
async function status(overrides: Record<string, unknown>) {
  await emit("dictation-status", {
    ...idle,
    attemptId: 1,
    revision: 1,
    ...overrides,
  });
}

beforeEach(() => {
  useDictationShortcut.setState({ capturing: false, unavailable: false });
  mocks.meeting.live.status = "inactive";
  mocks.config.dictation_trigger_mode = "hold";
  mocks.config.dictation_enabled = true;
  mocks.listen.mockImplementation(async (name, handler) => {
    mocks.handlers.set(name, handler);
    return () => mocks.handlers.delete(name);
  });
  mocks.invoke.mockImplementation(async (command) =>
    command === "dictation_get_status" ? idle : null,
  );
  mocks.update.mockResolvedValue({ status: "ok", data: null });
  mocks.show.mockResolvedValue({ status: "ok", data: null });
  mocks.hide.mockResolvedValue({ status: "ok", data: null });
  mocks.notify.mockResolvedValue({ status: "ok", data: null });
});
afterEach(() => {
  cleanup();
  mocks.handlers.clear();
  vi.clearAllMocks();
});

describe("DictationHost", () => {
  it("holds an early release until start returns and stops that attempt", async () => {
    let resolveStart!: (value: unknown) => void;
    const started = new Promise((resolve) => {
      resolveStart = resolve;
    });
    mocks.invoke.mockImplementation(async (command) => {
      if (command === "dictation_get_status") return idle;
      if (command === "dictation_start") return started;
      if (command === "dictation_stop")
        return {
          ...idle,
          attemptId: 1,
          revision: 2,
          phase: "transcribing",
          busy: true,
        };
      return null;
    });
    mount();
    await emit("plugin:windows:dictation-shortcut-down");
    await emit("plugin:windows:dictation-shortcut-up");
    expect(mocks.invoke).not.toHaveBeenCalledWith(
      "dictation_stop",
      expect.anything(),
    );
    await act(async () =>
      resolveStart({
        ...idle,
        attemptId: 1,
        revision: 1,
        phase: "starting",
        busy: true,
      }),
    );
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("dictation_stop", {
        attemptId: 1,
      }),
    );
  });
  it("does not start from shortcut capture or disabled dictation", async () => {
    mocks.config.dictation_enabled = false;
    mount();
    await emit("plugin:windows:dictation-shortcut-down");
    expect(mocks.invoke).not.toHaveBeenCalledWith("dictation_start", {});
    expect(mocks.invoke).toHaveBeenCalledWith(
      "plugin:windows|dictation_shortcut_configure",
      { shortcut: "" },
    );
  });
  it("ignores activation while the user edits a shortcut", async () => {
    mount();
    act(() => useDictationShortcut.setState({ capturing: true }));
    await emit("plugin:windows:dictation-shortcut-down");
    expect(mocks.invoke).not.toHaveBeenCalledWith("dictation_start", {});
  });
  it("ignores stale snapshots and actions from an older attempt", async () => {
    mount();
    await status({ attemptId: 2, revision: 5, phase: "recording", busy: true });
    await status({
      attemptId: 1,
      revision: 3,
      phase: "error",
      problem: "insertionFailed",
    });
    await emit("plugin:windows:dictation-action", {
      attemptId: 1,
      action: "cancel",
    });
    expect(mocks.invoke).not.toHaveBeenCalledWith(
      "dictation_cancel",
      expect.anything(),
    );
    await waitFor(() =>
      expect(mocks.update.mock.lastCall?.[0].title).toBe("Listening…"),
    );
  });
  it("offers direct copy recovery without the tray or a hide timer", async () => {
    mount();
    await status({ phase: "error", problem: "insertionFailed", canCopy: true });
    await waitFor(() => expect(mocks.update).toHaveBeenCalled());
    const state = mocks.update.mock.lastCall?.[0];
    expect(state.dictation.actions).toEqual([
      { id: "copy", label: "Copy text" },
      { id: "discard", label: "Discard" },
    ]);
    expect(mocks.hide).not.toHaveBeenCalled();
  });
  it("offers a single-line field hint with safe dismissal", async () => {
    mocks.invoke.mockImplementation(async () => idle);
    mount();
    await status({ phase: "error", problem: "targetUnavailable" });
    await waitFor(() => expect(mocks.update).toHaveBeenCalled());
    const state = mocks.update.mock.lastCall?.[0];
    expect(state.title).toBe("Click a text field to record into");
    expect(state.dictation.detail).toBeNull();
    expect(state.dictation.actions).toEqual([
      { id: "dismiss", label: "Dismiss" },
    ]);
    await emit("plugin:windows:dictation-action", {
      attemptId: 1,
      action: "dismiss",
    });
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("dictation_discard", {
        attemptId: 1,
      }),
    );
  });
  it("does not let dismissal cancel recording or clear recovery", async () => {
    mount();
    for (const [index, next] of [
      { phase: "recording", busy: true },
      { phase: "error", canRetry: true },
      { phase: "error", canCopy: true },
    ].entries()) {
      await status({
        ...next,
        revision: index + 1,
      });
      await emit("plugin:windows:dictation-action", {
        attemptId: 1,
        action: "dismiss",
      });
    }
    expect(mocks.invoke).not.toHaveBeenCalledWith(
      "dictation_discard",
      expect.anything(),
    );
    expect(mocks.invoke).not.toHaveBeenCalledWith(
      "dictation_cancel",
      expect.anything(),
    );
  });
  it("success shows a completion state without redundant copy controls", async () => {
    mount();
    await status({ phase: "done", canCopy: true });
    await waitFor(() =>
      expect(mocks.update.mock.lastCall?.[0].dictation).toEqual({
        attemptId: 1,
        phase: "done",
        actions: [],
        detail: null,
      }),
    );
    expect(mocks.update.mock.lastCall?.[0].title).toBe("Done");
    await waitFor(() => expect(mocks.hide).toHaveBeenCalled(), {
      timeout: 1500,
    });
  });
  it("confirms copying then dismisses recovery controls", async () => {
    mocks.invoke.mockImplementation(async (command) =>
      command === "dictation_get_status"
        ? idle
        : {
            ...idle,
            attemptId: 1,
            revision: 2,
            phase: "copied",
            canCopy: true,
          },
    );
    mount();
    await status({ phase: "ready", canCopy: true });
    await emit("plugin:windows:dictation-action", {
      attemptId: 1,
      action: "copy",
    });
    await waitFor(() =>
      expect(mocks.update.mock.lastCall?.[0].title).toBe("Copied"),
    );
    expect(mocks.update.mock.lastCall?.[0].dictation.actions).toEqual([]);
    await waitFor(() => expect(mocks.hide).toHaveBeenCalled(), {
      timeout: 1500,
    });
  });
  it("does not let a completion timer hide a new meeting", async () => {
    mount();
    await status({ phase: "done", canCopy: true });
    await waitFor(() => expect(mocks.update).toHaveBeenCalled());
    act(() => {
      mocks.meeting.live.status = "active";
      mocks.meetingListeners.forEach((handler) => handler());
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 800));
    });
    expect(mocks.hide).not.toHaveBeenCalled();
  });
  it("drops queued overlay updates after a meeting takes ownership", async () => {
    let resolveUpdate!: (result: unknown) => void;
    mocks.update.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveUpdate = resolve;
        }),
    );
    mount();
    await status({ phase: "recording", busy: true });
    await waitFor(() => expect(mocks.update).toHaveBeenCalledTimes(1));
    await status({ phase: "transcribing", busy: true, revision: 2 });
    act(() => {
      mocks.meeting.live.status = "active";
      mocks.meetingListeners.forEach((handler) => handler());
    });
    await act(async () => resolveUpdate({ status: "ok", data: null }));
    expect(mocks.update).toHaveBeenCalledTimes(1);
    expect(mocks.show).not.toHaveBeenCalled();
  });
  it.each(["meetingActive", "targetUnavailable"])(
    "explains a blocked shortcut during a meeting (%s) without changing its bar",
    async (problem) => {
      mocks.meeting.live.status = "active";
      mocks.invoke.mockImplementation(async (command) =>
        command === "dictation_get_status"
          ? idle
          : { ...idle, attemptId: 1, revision: 1, phase: "error", problem },
      );
      mount();
      await emit("plugin:windows:dictation-shortcut-down");
      await waitFor(() =>
        expect(mocks.notify).toHaveBeenCalledWith(
          expect.objectContaining({
            title: "A meeting is recording",
            message: "Try dictation after the meeting finishes.",
          }),
        ),
      );
      await status({ phase: "error", problem, revision: 2 });
      expect(mocks.notify).toHaveBeenCalledTimes(1);
      expect(mocks.update).not.toHaveBeenCalled();
      expect(mocks.hide).not.toHaveBeenCalled();
    },
  );
  it("does not report a previous dictation error when a meeting starts", async () => {
    mount();
    await status({ phase: "error", problem: "insertionFailed", canCopy: true });
    act(() => {
      mocks.meeting.live.status = "active";
      mocks.meetingListeners.forEach((handler) => handler());
    });
    await status({
      phase: "error",
      problem: "insertionFailed",
      canCopy: true,
      revision: 2,
    });
    expect(mocks.notify).not.toHaveBeenCalled();
  });
  it("keeps retry results manual instead of inserting into a changed target", async () => {
    mocks.invoke.mockImplementation(async (command) =>
      command === "dictation_get_status"
        ? idle
        : { ...idle, attemptId: 1, revision: 2, phase: "ready", canCopy: true },
    );
    mount();
    await status({
      phase: "error",
      problem: "transcriptionFailed",
      canRetry: true,
    });
    await emit("plugin:windows:dictation-action", {
      attemptId: 1,
      action: "retry",
    });
    expect(mocks.invoke).toHaveBeenCalledWith("dictation_retry", {
      attemptId: 1,
    });
    await waitFor(() =>
      expect(mocks.update.mock.lastCall?.[0].dictation.actions[0]).toEqual({
        id: "copy",
        label: "Copy text",
      }),
    );
  });
});
