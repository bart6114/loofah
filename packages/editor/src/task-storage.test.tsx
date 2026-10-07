import { renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { expect, it, vi } from "vitest";

import {
  createInMemoryTaskStorage,
  TaskStorageProvider,
  useTaskRecord,
  useTaskRecords,
} from "./task-storage";

it("keeps source subscriptions stable across ordinary rerenders", () => {
  const storage = createInMemoryTaskStorage();
  const subscribe = vi.spyOn(storage, "subscribeSource");
  const wrapper = ({ children }: { children: ReactNode }) => (
    <TaskStorageProvider storage={storage}>{children}</TaskStorageProvider>
  );
  const hook = renderHook(
    () => {
      useTaskRecords({ type: "session_raw_note", id: "session" });
      useTaskRecord({ type: "session_raw_note", id: "session" }, "task");
    },
    { wrapper },
  );
  for (let i = 0; i < 10; i++) hook.rerender();
  expect(subscribe).toHaveBeenCalledTimes(2);
  hook.unmount();
});
