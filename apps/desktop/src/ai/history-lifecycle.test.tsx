import { act, renderHook } from "@testing-library/react";
import { StrictMode, type ReactNode } from "react";
import { afterEach, expect, it, vi } from "vitest";

import { AITaskProvider } from "./contexts";
import { useAITaskTask } from "./hooks/useAITaskTask";

import { createAITaskStore } from "~/store/zustand/ai-task";

vi.mock("~/settings/queries", () => ({
  getStoredSettingValues: async () => ({ values: {}, hasValues: new Set() }),
}));
afterEach(() => vi.useRealTimers());

it("preserves observed completion and error states, then releases observers and the expiry timer on unmount", async () => {
  vi.useFakeTimers();
  const store = createAITaskStore();
  const onSuccess = vi.fn();
  const onError = vi.fn();
  const wrapper = ({ children }: { children: ReactNode }) => (
    <StrictMode>
      <AITaskProvider store={store}>{children}</AITaskProvider>
    </StrictMode>
  );
  const hook = renderHook(
    () => useAITaskTask("summary-enhance", "enhance", { onSuccess, onError }),
    { wrapper },
  );
  act(() =>
    store.getState().syncRemoteTask("summary-enhance", {
      taskType: "enhance",
      status: "generating",
      streamedText: "streaming",
    }),
  );
  act(() =>
    store.getState().syncRemoteTask("summary-enhance", {
      taskType: "enhance",
      status: "success",
      streamedText: "completed",
    }),
  );
  expect(onSuccess).toHaveBeenCalledOnce();
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5 * 60_000);
  });
  expect(hook.result.current.streamedText).toBe("completed");
  act(() =>
    store.getState().syncRemoteTask("summary-enhance", {
      taskType: "enhance",
      status: "generating",
      streamedText: "",
    }),
  );
  act(() =>
    store.getState().syncRemoteTask("summary-enhance", {
      taskType: "enhance",
      status: "error",
      streamedText: "",
      error: { message: "persistence failed" },
    }),
  );
  expect(onError).toHaveBeenCalledOnce();
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5 * 60_000);
  });
  expect(hook.result.current.error?.message).toBe("persistence failed");
  hook.unmount();
  expect(store.getState().tasks).toEqual({});
  expect(vi.getTimerCount()).toBe(0);
});
