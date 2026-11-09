import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  message: vi.fn(),
  error: vi.fn(),
  dismiss: vi.fn(),
  openCurrent: vi.fn(),
  restoreDeletedSession: vi.fn(),
}));

vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: {
    message: mocks.message,
    error: mocks.error,
    dismiss: mocks.dismiss,
  },
}));

vi.mock("~/store/zustand/tabs", () => ({
  useTabs: (selector: (state: { openCurrent: () => void }) => unknown) =>
    selector({ openCurrent: mocks.openCurrent }),
}));

import { UndoDeleteToast } from "./undo-delete-toast";

import { useUndoDelete } from "~/store/zustand/undo-delete";
const capturedScope = {
  client: { scope: "local:undo" },
  queryClient: new QueryClient(),
};

const addDeletion = useUndoDelete.getState().addDeletion;

describe("UndoDeleteToast", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    useUndoDelete.setState({
      pendingDeletions: {},
      addDeletion: (scope, data, options) =>
        addDeletion(scope, data, { owner: capturedScope, ...options }),
    });
    mocks.message.mockClear();
    mocks.error.mockClear();
    mocks.dismiss.mockClear();
    mocks.openCurrent.mockClear();
    mocks.restoreDeletedSession.mockClear();
  });

  afterEach(() => {
    for (const pending of Object.values(
      useUndoDelete.getState().pendingDeletions,
    )) {
      if (pending.timeoutId) clearTimeout(pending.timeoutId);
    }
    useUndoDelete.setState({ pendingDeletions: {} });
    cleanup();
    vi.useRealTimers();
  });

  it("renders undo deletion through Sonner", () => {
    act(() => {
      useUndoDelete.getState().addDeletion(
        "local:undo",
        {
          session: { id: "session-1", title: "Design sync" },
          tombstone: "tombstone",
          deletedAt: Date.now(),
        },
        {
          restore: async (assert) => {
            assert();
            await mocks.restoreDeletedSession({ session: { id: "session-1" } });
          },
        },
      );
    });

    const queryClient = new QueryClient();
    const view = render(
      <QueryClientProvider client={queryClient}>
        <UndoDeleteToast />
      </QueryClientProvider>,
    );

    expect(mocks.message).toHaveBeenCalledWith(
      "Design sync deleted",
      expect.objectContaining({
        id: `undo-delete:${JSON.stringify(["local:undo", "session", "session-1"])}`,
        duration: Infinity,
        action: expect.objectContaining({ label: "Undo" }),
      }),
    );
    expect(mocks.message.mock.calls[0][1]).not.toHaveProperty("cancel");
    expect(mocks.message.mock.calls[0][1]).toEqual(
      expect.objectContaining({
        description: expect.objectContaining({
          props: expect.objectContaining({
            className: expect.stringContaining("undo-delete-toast-gauge"),
            style: expect.objectContaining({
              "--undo-delete-duration": "5000ms",
              "--undo-delete-progress": 1,
            }),
          }),
        }),
        descriptionClassName: expect.stringContaining("bottom-0"),
      }),
    );

    view.unmount();
    expect(mocks.dismiss).toHaveBeenCalledWith(
      `undo-delete:${JSON.stringify(["local:undo", "session", "session-1"])}`,
    );
  });

  it("dismisses the toast and reopens the tab before the restore resolves", async () => {
    let release!: () => void;
    mocks.restoreDeletedSession.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          release = resolve;
        }),
    );

    act(() => {
      useUndoDelete.getState().addDeletion(
        "local:undo",
        {
          session: { id: "session-1", title: "Design sync" },
          tombstone: "tombstone",
          deletedAt: Date.now(),
        },
        {
          restore: async (assert) => {
            assert();
            await mocks.restoreDeletedSession({ session: { id: "session-1" } });
          },
        },
      );
    });

    const queryClient = new QueryClient();
    render(
      <QueryClientProvider client={queryClient}>
        <UndoDeleteToast />
      </QueryClientProvider>,
    );

    const options =
      mocks.message.mock.calls[mocks.message.mock.calls.length - 1][1];
    act(() => {
      options.action.onClick();
    });

    expect(mocks.restoreDeletedSession).toHaveBeenCalledOnce();
    expect(mocks.openCurrent).toHaveBeenCalledWith({
      type: "sessions",
      id: "session-1",
    });
    expect(useUndoDelete.getState().pendingDeletions).toEqual({});
    await act(async () => {
      release();
      await Promise.resolve();
    });
  });

  it("updates a batch toast when later deletions join the batch", async () => {
    const queryClient = new QueryClient();
    render(
      <QueryClientProvider client={queryClient}>
        <UndoDeleteToast />
      </QueryClientProvider>,
    );

    act(() => {
      useUndoDelete.getState().addDeletion(
        "local:undo",
        {
          session: { id: "session-1", title: "Design sync" },
          tombstone: "tombstone-1",
          deletedAt: Date.now(),
        },
        {
          batchId: "batch-1",
          restore: async (assert) => {
            assert();
            await mocks.restoreDeletedSession({ session: { id: "session-1" } });
          },
        },
      );
    });

    expect(mocks.message).toHaveBeenLastCalledWith(
      "1 note deleted",
      expect.objectContaining({
        id: `undo-delete:${JSON.stringify(["local:undo", "batch", "batch-1"])}`,
      }),
    );

    act(() => {
      useUndoDelete.getState().addDeletion(
        "local:undo",
        {
          session: { id: "session-2", title: "Weekly review" },
          tombstone: "tombstone-2",
          deletedAt: Date.now(),
        },
        {
          batchId: "batch-1",
          restore: async (assert) => {
            assert();
            await mocks.restoreDeletedSession({ session: { id: "session-2" } });
          },
        },
      );
    });

    expect(mocks.message).toHaveBeenLastCalledWith(
      "2 notes deleted",
      expect.objectContaining({
        id: `undo-delete:${JSON.stringify(["local:undo", "batch", "batch-1"])}`,
      }),
    );

    const options =
      mocks.message.mock.calls[mocks.message.mock.calls.length - 1][1];
    await act(async () => {
      options.action.onClick();
      await Promise.resolve();
    });

    expect(mocks.restoreDeletedSession).toHaveBeenCalledTimes(2);
    expect(mocks.restoreDeletedSession).toHaveBeenNthCalledWith(
      1,
      expect.objectContaining({
        session: expect.objectContaining({ id: "session-1" }),
      }),
    );
    expect(mocks.restoreDeletedSession).toHaveBeenNthCalledWith(
      2,
      expect.objectContaining({
        session: expect.objectContaining({ id: "session-2" }),
      }),
    );
  });
  it("does not display or undo another scope's same-ID tombstone", () => {
    const other = vi.fn(async () => {});
    act(() =>
      useUndoDelete.getState().addDeletion(
        "remote:other",
        {
          session: { id: "session-1", title: "Private other" },
          tombstone: "other",
          deletedAt: 1,
        },
        { restore: other },
      ),
    );
    render(<UndoDeleteToast />);
    expect(mocks.message).not.toHaveBeenCalled();
    expect(other).not.toHaveBeenCalled();
  });

  it("never lets a stale Sonner action clear a replacement same-ID tombstone", () => {
    const first = vi.fn(async () => {}),
      second = vi.fn(async () => {});
    const data = {
      session: { id: "same-id", title: "First" },
      tombstone: "same-time",
      deletedAt: 1,
    };
    act(() =>
      useUndoDelete
        .getState()
        .addDeletion("local:undo", data, { restore: first }),
    );
    render(<UndoDeleteToast />);
    const old = mocks.message.mock.calls[0][1].action.onClick;
    act(() =>
      useUndoDelete
        .getState()
        .addDeletion(
          "local:undo",
          { ...data, session: { id: "same-id", title: "Second" } },
          { restore: second },
        ),
    );
    const newer = Object.values(useUndoDelete.getState().pendingDeletions)[0];
    act(() => {
      void old();
    });
    expect(first).not.toHaveBeenCalled();
    expect(second).not.toHaveBeenCalled();
    expect(Object.values(useUndoDelete.getState().pendingDeletions)).toEqual([
      newer,
    ]);
  });

  it("latches a replacement deletion while Undo waits and never re-admits the old tombstone", async () => {
    let release!: () => void;
    const waiting = new Promise<void>((resolve) => {
      release = resolve;
    });
    const restored = vi.fn();
    const data = {
      session: { id: "same-id", title: "Old" },
      tombstone: "old",
      deletedAt: 1,
    };
    act(() =>
      useUndoDelete.getState().addDeletion("local:undo", data, {
        restore: async (assert) => {
          await waiting;
          assert();
          restored();
        },
      }),
    );
    render(<UndoDeleteToast />);
    act(() => {
      void mocks.message.mock.calls[0][1].action.onClick();
    });
    act(() => {
      useUndoDelete
        .getState()
        .addDeletion(
          "local:undo",
          { ...data, tombstone: "new" },
          { restore: async () => {} },
        );
      useUndoDelete.getState().clearDeletion("local:undo", "same-id");
    });
    await act(async () => {
      release();
      await Promise.resolve();
    });
    expect(restored).not.toHaveBeenCalled();
    expect(useUndoDelete.getState().pendingDeletions).toEqual({});
  });
});

vi.mock("~/main/use-legacy-activity", async () => {
  const { legacyActivityFixture } =
    await import("~/main/legacy-activity.test-support");
  return {
    useLegacySessionActivity: () => legacyActivityFixture(),
    useLegacyDesktopAccess: () => ({
      ...legacyActivityFixture(),
      scope: capturedScope,
    }),
  };
});

vi.mock("~/session/hooks/useDeleteSession", () => ({
  useSessionDeletionAccess: () => ({
    scope: capturedScope,
    available: true,
    isAvailable: () => true,
    shared: false,
  }),
}));
