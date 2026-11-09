import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  available: false,
  listen: vi.fn(),
  start: vi.fn(),
  model: vi.fn(),
  stop: vi.fn(),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: mocks.listen,
  emitTo: vi.fn(),
}));
vi.mock("~/main/use-legacy-activity", () => {
  const isAvailable = () => mocks.available;
  const listener = {
    getState: () => ({
      live: { sessionId: null, loading: false, status: "inactive" },
    }),
  };
  const access = () => ({ available: mocks.available, isAvailable, listener });
  return { useLegacyDesktopAccess: access, useLegacySessionActivity: access };
});
vi.mock("./useStartListening", () => ({
  useStartListening: () => {
    mocks.model();
    return mocks.start;
  },
}));
import { MainListenerControlBridge } from "./window-control";

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  mocks.available = false;
});
it("does not subscribe or mount local model hooks before admission, then follows Local readiness", async () => {
  mocks.listen.mockResolvedValue(() => {});
  const { rerender } = render(<MainListenerControlBridge />);
  expect(mocks.listen).not.toHaveBeenCalled();
  expect(mocks.model).not.toHaveBeenCalled();
  mocks.available = true;
  rerender(<MainListenerControlBridge />);
  await waitFor(() => expect(mocks.listen).toHaveBeenCalledOnce());
  await act(async () =>
    mocks.listen.mock.calls[0][1]({
      payload: { action: "start", requestId: "one", sessionId: "same-id" },
    }),
  );
  await waitFor(() => expect(mocks.start).toHaveBeenCalledOnce());
});
it("drops a late queued listener request after admission is revoked", async () => {
  mocks.available = true;
  mocks.listen.mockResolvedValue(() => {});
  render(<MainListenerControlBridge />);
  await waitFor(() => expect(mocks.listen).toHaveBeenCalledOnce());
  const receive = mocks.listen.mock.calls[0][1];
  mocks.available = false;
  await act(async () =>
    receive({
      payload: { action: "start", requestId: "late", sessionId: "same-id" },
    }),
  );
  expect(mocks.start).not.toHaveBeenCalled();
  expect(mocks.model).not.toHaveBeenCalled();
});
