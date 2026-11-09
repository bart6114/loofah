import { onlineManager } from "@tanstack/react-query";
import { act, cleanup, renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  create: vi.fn(),
  error: vi.fn(),
  device: null as { startPending: ReturnType<typeof vi.fn> } | null,
}));
vi.mock("./application/device-recording-controls", () => ({
  useDeviceStartActions: () => mocks.device,
}));
vi.mock("~/session/queries", () => ({ createSession: mocks.create }));
vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: { error: mocks.error },
}));

import { useNewNoteAndListen } from "./useNewNote";

import { MainHostContext } from "~/main/host-context";
import { VaultHostContext } from "~/shared/application/vault-provider";
import { createListenerStore } from "~/store/zustand/listener";
import { TabsStoreProvider } from "~/store/zustand/tabs/scoped-provider";
import {
  deferred,
  scopedFixture,
} from "~/store/zustand/tabs/scoped-test-utils";
import { createTabsStore } from "~/store/zustand/tabs/store";

function fixture(allowed = true) {
  const f = scopedFixture("same-scope");
  Object.assign(f.scope.client, {
    features: allowed ? ["legacy_desktop_transcript_controls"] : [],
    networkMode: "always",
  });
  const listenerStore = createListenerStore();
  const store = createTabsStore(f.scope.client.scope, () => true);
  store.getState().openCurrent({ type: "empty" });
  const activity = {
    scope: f.scope,
    captureState: () => "idle",
    subscribeCapture: () => () => {},
  };
  const Host = ({ children }: { children: ReactNode }) => (
    <VaultHostContext.Provider
      value={{ scope: f.scope, error: null, retry() {} }}
    >
      <MainHostContext.Provider
        value={{ scope: f.scope, listenerStore, activity }}
      >
        <TabsStoreProvider store={store}>{children}</TabsStoreProvider>
      </MainHostContext.Provider>
    </VaultHostContext.Provider>
  );
  return { ...f, store, Host };
}
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  mocks.device = null;
  onlineManager.setOnline(true);
});

it("refuses unsupported New+Record without creating an ordinary note or changing the tab", () => {
  const f = fixture(false);
  const { result } = renderHook(() => useNewNoteAndListen(), {
    wrapper: f.Host,
  });
  act(() => result.current());
  expect(mocks.create).not.toHaveBeenCalled();
  expect(mocks.error).toHaveBeenCalledOnce();
  expect(f.store.getState().currentTab?.type).toBe("empty");
});
it("preserves login-free offline Local New+Record and current-tab behavior", async () => {
  onlineManager.setOnline(false);
  mocks.create.mockResolvedValueOnce("new-local");
  const f = fixture();
  const { result } = renderHook(
    () => useNewNoteAndListen({ behavior: "current" }),
    { wrapper: f.Host },
  );
  await act(async () => result.current());
  expect(mocks.create).toHaveBeenCalledOnce();
  expect(f.store.getState().currentTab).toMatchObject({
    id: "new-local",
    state: { autoStart: true },
  });
});
it.each([50, 150, 500])(
  "never auto-starts or selects a late creation after retirement (%ims)",
  async (latency) => {
    const pending = deferred<string>();
    mocks.create.mockReturnValueOnce(pending.promise);
    const f = fixture();
    const { result, unmount } = renderHook(() => useNewNoteAndListen(), {
      wrapper: f.Host,
    });
    act(() => result.current());
    unmount();
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, latency));
      pending.resolve("late");
    });
    expect(mocks.create).toHaveBeenCalledOnce();
    expect(f.store.getState().currentTab?.type).toBe("empty");
  },
);
it("does not replace an unrelated tab selected during recording-note creation", async () => {
  const pending = deferred<string>();
  mocks.create.mockReturnValueOnce(pending.promise);
  const f = fixture();
  const { result } = renderHook(
    () => useNewNoteAndListen({ behavior: "current" }),
    { wrapper: f.Host },
  );
  act(() => result.current());
  act(() => f.store.getState().openNew({ type: "sessions", id: "unrelated" }));
  await act(async () => pending.resolve("late"));
  expect(f.store.getState().currentTab).toMatchObject({ id: "unrelated" });
});

it("routes Remote New+Record through the original device host without Local creation or an auto-start tab", async () => {
  const startPending = vi.fn(async () => ({}));
  mocks.device = { startPending };
  onlineManager.setOnline(false);
  const f = fixture(false);
  const { result } = renderHook(() => useNewNoteAndListen(), {
    wrapper: f.Host,
  });
  await act(async () => result.current());
  expect(startPending).toHaveBeenCalledOnce();
  expect(mocks.create).not.toHaveBeenCalled();
  expect(mocks.error).not.toHaveBeenCalled();
  expect(f.store.getState().currentTab?.type).toBe("empty");
});
