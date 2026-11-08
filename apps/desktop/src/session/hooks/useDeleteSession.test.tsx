import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { DeletedSessionData } from "~/store/zustand/undo-delete";

const mocks = vi.hoisted(() => {
  const deletedSessionData: DeletedSessionData = {
    session: {
      id: "session-1",
      title: "Deleted note",
    },
    tombstone: "2026-01-01T00:00:00Z",
    deletedAt: 1,
  };

  return {
    allowed: true,
    capture: "idle",
    addDeletion: vi.fn(),
    clearDeletion: vi.fn(),
    pendingDeletions: {} as Record<string, { data: DeletedSessionData }>,
    emitTo: vi.fn(() => Promise.resolve()),
    deleteSessionFolder: vi.fn(() =>
      Promise.resolve({ status: "ok" as const, data: null }),
    ),
    getAllWebviewWindows: vi.fn<
      () => Promise<Array<{ label: string; close: () => Promise<void> }>>
    >(() => Promise.resolve([])),
    getCurrentWebviewWindowLabel: vi.fn(() => "main"),
    invalidateResource: vi.fn(),
    openCurrent: vi.fn(),
    openTabs: [] as Array<{ type: string; id: string }>,
    listenerGetState: vi.fn(),
    listenerStop: vi.fn(),
    listen: vi.fn(),
    softDeleteSession: vi.fn<() => Promise<DeletedSessionData | null>>(() =>
      Promise.resolve(deletedSessionData),
    ),
    toastError: vi.fn(),
    toastWarning: vi.fn(),
    deletedSessionData,
  };
});

vi.mock("@tauri-apps/api/event", () => ({
  emitTo: mocks.emitTo,
  listen: mocks.listen,
}));

vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getAllWebviewWindows: mocks.getAllWebviewWindows,
}));

vi.mock("@hypr/plugin-windows", () => ({
  getCurrentWebviewWindowLabel: mocks.getCurrentWebviewWindowLabel,
}));

vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: { error: mocks.toastError, warning: mocks.toastWarning },
}));

vi.mock("~/session/queries", () => ({
  softDeleteSession: mocks.softDeleteSession,
}));

// The hard-delete command must stay unreachable from the delete flow: `session_delete`
// trashes the folder, and `delete_session_folder` is an untrashed `remove_dir_all`.
vi.mock("@hypr/plugin-fs-sync", () => ({
  commands: { deleteSessionFolder: mocks.deleteSessionFolder },
}));

vi.mock("~/store/zustand/listener/instance", () => ({
  listenerStore: {
    getState: mocks.listenerGetState,
  },
}));

vi.mock("~/store/zustand/tabs", () => {
  const getState = () => ({
    tabs: mocks.openTabs,
    invalidateResource: mocks.invalidateResource,
    openCurrent: mocks.openCurrent,
  });
  const useTabs = (selector: (state: ReturnType<typeof getState>) => unknown) =>
    selector(getState());
  useTabs.getState = getState;
  return { useTabs, useTabsStore: () => ({ getState }) };
});

vi.mock("~/store/zustand/undo-delete", () => {
  const getState = () => ({
    pendingDeletions: mocks.pendingDeletions,
    addDeletion: mocks.addDeletion,
    clearDeletion: mocks.clearDeletion,
  });
  const useUndoDelete = (
    selector: (state: ReturnType<typeof getState>) => unknown,
  ) => selector(getState());
  useUndoDelete.getState = getState;
  return { useUndoDelete };
});

import {
  useDeleteSession,
  useRemoteSessionDeletionUndoListener,
} from "./useDeleteSession";

describe("useDeleteSession", () => {
  beforeEach(() => {
    cleanup();
    vi.clearAllMocks();
    mocks.allowed = true;
    mocks.capture = "idle";
    for (const key of Object.keys(mocks.pendingDeletions)) {
      delete mocks.pendingDeletions[key];
    }
    mocks.addDeletion.mockImplementation((data: DeletedSessionData) => {
      mocks.pendingDeletions[data.session.id] = { data };
    });
    mocks.clearDeletion.mockImplementation((sessionId: string) => {
      delete mocks.pendingDeletions[sessionId];
    });
    mocks.openTabs.length = 0;
    mocks.softDeleteSession.mockResolvedValue(mocks.deletedSessionData);
    mocks.emitTo.mockResolvedValue(undefined);
    mocks.getAllWebviewWindows.mockResolvedValue([]);
    mocks.getCurrentWebviewWindowLabel.mockReturnValue("main");
    mocks.listenerGetState.mockReturnValue({
      live: {
        sessionId: null,
        status: "inactive",
        loading: false,
      },
      stop: mocks.listenerStop,
    });
    mocks.listen.mockResolvedValue(vi.fn());
  });

  // Regression: the undo window used to arm a 5s timer that ran fs-sync's
  // `delete_session_folder` -- a plain `remove_dir_all` with no `.trash` copy -- against a
  // path `session_delete` had already trashed. Anything that recreated the folder inside
  // that window (a sync client pulling it back from another device, a late transcript
  // flush) was destroyed with no undo. The delete is complete when the commit resolves;
  // letting the toast lapse must schedule no filesystem work at all.
  it("schedules no hard delete when the undo window lapses", async () => {
    const { result } = renderHook(() => useDeleteSession());

    act(() => {
      result.current("session-1");
    });

    await waitFor(() => {
      expect(mocks.addDeletion).toHaveBeenCalledOnce();
    });
    expect(mocks.softDeleteSession).toHaveBeenCalledWith(
      "session-1",
      expect.any(String),
      expect.any(Function),
    );

    // No confirm callback is registered, so `confirmDeletion` has nothing to run.
    const onConfirm = mocks.addDeletion.mock.calls[0]?.[1];
    expect(onConfirm).toBeUndefined();
    expect(mocks.deleteSessionFolder).not.toHaveBeenCalled();
  });

  it("adds the undo deletion optimistically in the main window", async () => {
    // Never resolves: the optimistic UI must not wait for the write.
    mocks.softDeleteSession.mockImplementation(() => new Promise(() => {}));
    const { result } = renderHook(() => useDeleteSession());

    act(() => {
      result.current("session-1", { title: "Note" });
    });

    expect(mocks.addDeletion).toHaveBeenCalledWith(
      {
        session: { id: "session-1", title: "Note" },
        tombstone: expect.any(String),
        deletedAt: expect.any(Number),
      },
      undefined,
      undefined,
    );
    expect(mocks.softDeleteSession).toHaveBeenCalledWith(
      "session-1",
      expect.any(String),
      expect.any(Function),
    );
    expect(mocks.invalidateResource).toHaveBeenCalledWith(
      "sessions",
      "session-1",
    );
    expect(mocks.emitTo).not.toHaveBeenCalled();
  });

  it("rolls back the optimistic deletion when the soft delete fails", async () => {
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => undefined);
    mocks.softDeleteSession.mockRejectedValue(new Error("db locked"));
    const { result } = renderHook(() => useDeleteSession());

    act(() => {
      result.current("session-1");
    });

    expect(mocks.addDeletion).toHaveBeenCalledOnce();
    await waitFor(() => {
      expect(mocks.toastError).toHaveBeenCalledOnce();
    });
    expect(mocks.clearDeletion).toHaveBeenCalledWith("session-1");
    expect(mocks.deleteSessionFolder).not.toHaveBeenCalled();
    consoleError.mockRestore();
  });

  it("drops the optimistic toast quietly when the session is already deleted", async () => {
    mocks.softDeleteSession.mockResolvedValue(null);
    const { result } = renderHook(() => useDeleteSession());

    act(() => {
      result.current("session-1");
    });

    expect(mocks.addDeletion).toHaveBeenCalledOnce();
    await waitFor(() => {
      expect(mocks.clearDeletion).toHaveBeenCalledWith("session-1");
    });
    expect(mocks.toastError).not.toHaveBeenCalled();
  });

  // The background-window path (delete commits, `emitTo("main")` fails so main never learns
  // about it) also used to fall back to the hard delete. It has nothing to clean up either.
  it("never hard-deletes the folder when a background window's delete is stranded", async () => {
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => undefined);
    mocks.getCurrentWebviewWindowLabel.mockReturnValue("note-session-1");
    mocks.emitTo.mockRejectedValue(new Error("main window unavailable"));
    const { result } = renderHook(() => useDeleteSession());

    act(() => {
      result.current("session-1");
    });

    await waitFor(() => {
      expect(mocks.emitTo).toHaveBeenCalled();
    });
    expect(mocks.deleteSessionFolder).not.toHaveBeenCalled();
    expect(mocks.addDeletion).not.toHaveBeenCalled();
    consoleError.mockRestore();
  });

  it("stops listening before deleting the active session", async () => {
    mocks.listenerGetState.mockReturnValue({
      live: {
        sessionId: "session-1",
        status: "active",
        loading: false,
      },
      stop: mocks.listenerStop,
    });
    const { result } = renderHook(() => useDeleteSession());

    act(() => {
      result.current("session-1");
    });

    await waitFor(() => {
      expect(mocks.softDeleteSession).toHaveBeenCalledWith(
        "session-1",
        expect.any(String),
        expect.any(Function),
      );
    });
    expect(mocks.listenerStop).toHaveBeenCalledTimes(1);
    expect(mocks.listenerStop.mock.invocationCallOrder[0]).toBeLessThan(
      mocks.softDeleteSession.mock.invocationCallOrder[0],
    );
  });

  it("does not stop listening when deleting an inactive session", () => {
    mocks.listenerGetState.mockReturnValue({
      live: {
        sessionId: "session-2",
        status: "active",
        loading: false,
      },
      stop: mocks.listenerStop,
    });
    const { result } = renderHook(() => useDeleteSession());

    act(() => {
      result.current("session-1");
    });

    expect(mocks.listenerStop).not.toHaveBeenCalled();
  });

  it("forwards undo data to main and closes the matching note window", async () => {
    const close = vi.fn(() => Promise.resolve());
    mocks.getCurrentWebviewWindowLabel.mockReturnValue("note-session-1");
    mocks.getAllWebviewWindows.mockResolvedValue([
      { label: "note-session-1", close },
      { label: "note-session-2", close: vi.fn() },
    ]);
    const { result } = renderHook(() => useDeleteSession());

    act(() => {
      result.current("session-1");
    });

    await waitFor(() => {
      expect(mocks.emitTo).toHaveBeenCalledWith(
        "main",
        "hypr://session-deleted-for-undo",
        {
          sessionId: "session-1",
          data: mocks.deletedSessionData,
        },
      );
      expect(close).toHaveBeenCalled();
    });

    expect(mocks.addDeletion).not.toHaveBeenCalled();
  });

  it("closes the matching note window when deleting from the main window", async () => {
    const close = vi.fn(() => Promise.resolve());
    mocks.getAllWebviewWindows.mockResolvedValue([
      { label: "note-session-1", close },
    ]);
    const { result } = renderHook(() => useDeleteSession());

    act(() => {
      result.current("session-1");
    });

    await waitFor(() => {
      expect(close).toHaveBeenCalled();
    });
  });

  it("still closes the standalone note window when forwarding undo data fails", async () => {
    const close = vi.fn(() => Promise.resolve());
    mocks.getCurrentWebviewWindowLabel.mockReturnValue("note-session-1");
    mocks.emitTo.mockRejectedValue(new Error("main window unavailable"));
    mocks.getAllWebviewWindows.mockResolvedValue([
      { label: "note-session-1", close },
    ]);
    const { result } = renderHook(() => useDeleteSession());

    act(() => {
      result.current("session-1");
    });

    await waitFor(() => {
      expect(close).toHaveBeenCalled();
    });
  });

  it("listens for forwarded standalone note deletions in the main window", async () => {
    let handler:
      | ((event: {
          payload: { sessionId: string; data: DeletedSessionData };
        }) => void)
      | null = null;
    mocks.listen.mockImplementation((_, callback) => {
      handler = callback;
      return Promise.resolve(vi.fn());
    });

    renderHook(() => useRemoteSessionDeletionUndoListener(true));

    await waitFor(() => {
      expect(mocks.listen).toHaveBeenCalledWith(
        "hypr://session-deleted-for-undo",
        expect.any(Function),
      );
    });

    act(() => {
      handler?.({
        payload: {
          sessionId: "session-1",
          data: mocks.deletedSessionData,
        },
      });
    });

    expect(mocks.addDeletion).toHaveBeenCalledWith(mocks.deletedSessionData);
    expect(mocks.invalidateResource).toHaveBeenCalledWith(
      "sessions",
      "session-1",
    );
    expect(mocks.softDeleteSession).not.toHaveBeenCalled();
  });
});

vi.mock("~/main/use-legacy-activity", async () => {
  const { legacyActivityFixture } =
    await import("~/main/legacy-activity.test-support");
  return {
    useLegacyDesktopAccess: () => ({
      ...legacyActivityFixture(mocks.listenerGetState()),
      isAvailable: () => mocks.allowed,
      activity: { captureState: () => mocks.capture },
    }),
  };
});

it.each(["unsupported", "unknown"])(
  "never deletes or stops capture with %s ownership",
  (kind) => {
    cleanup();
    vi.clearAllMocks();
    mocks.allowed = kind !== "unsupported";
    mocks.capture = kind === "unknown" ? "unknown" : "idle";
    const { result } = renderHook(() => useDeleteSession());
    act(() => result.current("same-id"));
    expect(mocks.softDeleteSession).not.toHaveBeenCalled();
    expect(mocks.listenerStop).not.toHaveBeenCalled();
    expect(mocks.invalidateResource).not.toHaveBeenCalled();
  },
);
