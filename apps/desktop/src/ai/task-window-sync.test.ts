import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
const mocks = vi.hoisted(() => ({
  listen: vi.fn(),
  emitTo: vi.fn(),
  stop: vi.fn(),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: mocks.listen,
  emitTo: mocks.emitTo,
  emit: vi.fn(),
}));
vi.mock("@hypr/plugin-windows", () => ({
  getCurrentWebviewWindowLabel: () => "note-window",
}));
vi.mock("~/services/enhancer", () => ({ getEnhancerService: vi.fn() }));
import { requestMainEnhance } from "./task-window-sync";

describe("detached summary requests", () => {
  let respond: (event: { payload: unknown }) => void;
  beforeEach(() => {
    vi.clearAllMocks();
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
