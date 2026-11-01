import { act, renderHook } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";

import { useSummaryEditLock } from "./useSummaryEditLock";

import {
  beginSummaryActivity,
  useSummaryActivity,
} from "~/session/summary-activity";

const mocks = vi.hoisted(() => ({
  mode: "inactive",
  starting: false,
  status: "idle",
  label: "main",
}));
vi.mock("@hypr/plugin-windows", () => ({
  getCurrentWebviewWindowLabel: () => mocks.label,
}));
vi.mock("~/stt/contexts", () => ({
  useListener: (select: (state: any) => unknown) =>
    select({
      getSessionMode: () => mocks.mode,
      live: { sessionId: "session", loading: mocks.starting },
    }),
}));
vi.mock("~/ai/contexts", () => ({
  useAITask: (select: (state: any) => unknown) =>
    select({ tasks: { "session-enhance": { status: mocks.status } } }),
}));

beforeEach(() => {
  Object.assign(mocks, {
    mode: "inactive",
    starting: false,
    status: "idle",
    label: "main",
  });
  useSummaryActivity.setState({
    sessions: {},
    remoteSessions: {},
    remoteReady: false,
  });
});

it("covers native startup, recording, finalization, batch processing, and generation", () => {
  const { result, rerender } = renderHook(() => useSummaryEditLock("session"));
  expect(result.current).toBeNull();
  mocks.starting = true;
  rerender();
  expect(result.current).toBe("recording");
  mocks.starting = false;
  for (const mode of ["active", "finalizing", "running_batch"]) {
    mocks.mode = mode;
    rerender();
    expect(result.current).toBe(mode === "active" ? "recording" : "processing");
  }
  mocks.mode = "inactive";
  mocks.status = "generating";
  rerender();
  expect(result.current).toBe("processing");
  for (const status of ["success", "error", "idle"]) {
    mocks.status = status;
    rerender();
    expect(result.current).toBeNull();
  }
});

it("stays locked through the inactive post-capture handoff and isolates sessions", () => {
  const { result } = renderHook(() => useSummaryEditLock("session"));
  let activity!: ReturnType<typeof beginSummaryActivity>;
  act(() => {
    activity = beginSummaryActivity("session", "processing");
  });
  expect(result.current).toBe("processing");
  act(() => activity.finish());
  expect(result.current).toBeNull();
  act(() => {
    activity = beginSummaryActivity("other", "recording");
  });
  expect(result.current).toBeNull();
  act(() => activity.finish());
});

it("waits for the standalone snapshot and follows its processing state", () => {
  mocks.label = "note-window";
  const { result } = renderHook(() => useSummaryEditLock("session"));
  expect(result.current).toBe("processing");
  act(() =>
    useSummaryActivity.setState({
      remoteReady: true,
      remoteSessions: { session: "recording" },
    }),
  );
  expect(result.current).toBe("recording");
  act(() =>
    useSummaryActivity.setState({ remoteSessions: { session: "processing" } }),
  );
  expect(result.current).toBe("processing");
  act(() => useSummaryActivity.setState({ remoteSessions: {} }));
  expect(result.current).toBeNull();
});

it("does not let a local operation release the main window's lock", () => {
  mocks.label = "note-window";
  useSummaryActivity.setState({
    remoteReady: true,
    remoteSessions: { session: "processing" },
  });
  const { result } = renderHook(() => useSummaryEditLock("session"));
  let activity!: ReturnType<typeof beginSummaryActivity>;
  act(() => {
    activity = beginSummaryActivity("session", "recording");
  });
  expect(result.current).toBe("recording");
  act(() => activity.finish());
  expect(result.current).toBe("processing");
  act(() => useSummaryActivity.setState({ remoteSessions: {} }));
  expect(result.current).toBeNull();
});
