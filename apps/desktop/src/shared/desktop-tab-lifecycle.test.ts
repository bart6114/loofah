import { describe, expect, it, vi } from "vitest";

import {
  createSessionTabCloseHandler,
  initializeDesktopTabs,
} from "./desktop-tab-lifecycle";

import {
  createSettingsTab,
  createSessionTab,
} from "~/store/zustand/tabs/test-utils";

const flushAsyncCleanup = () =>
  new Promise((resolve) => {
    setTimeout(resolve, 0);
  });

describe("desktop tab lifecycle", () => {
  describe("initializeDesktopTabs", () => {
    it("restores pinned tabs and recent notes without opening a startup tab", async () => {
      const tabs = [createSessionTab({ id: "restored-session" })];
      const openNew = vi.fn();
      const setRecentlyOpenedSessionIds = vi.fn();
      const restorePinnedTabs = vi.fn().mockResolvedValue(undefined);
      const restoreRecentlyOpenedSessionIds = vi
        .fn()
        .mockImplementation(async (set: (ids: string[]) => void) => {
          set(["restored-session"]);
        });

      await initializeDesktopTabs({
        getTabs: () => tabs,
        setRecentlyOpenedSessionIds,
        restorePinnedTabs,
        restoreRecentlyOpenedSessionIds,
        onZeroTabs: null,
        isTauriEnv: true,
      });

      expect(restorePinnedTabs).toHaveBeenCalledTimes(1);
      expect(setRecentlyOpenedSessionIds).toHaveBeenCalledWith([
        "restored-session",
      ]);
      expect(openNew).not.toHaveBeenCalled();
    });

    it("stays on home when startup has no restored tabs", async () => {
      const openNew = vi.fn();

      await initializeDesktopTabs({
        getTabs: () => [],
        setRecentlyOpenedSessionIds: vi.fn(),
        restorePinnedTabs: vi.fn().mockResolvedValue(undefined),
        restoreRecentlyOpenedSessionIds: vi.fn().mockResolvedValue(undefined),
        onZeroTabs: null,
        isTauriEnv: true,
      });

      expect(openNew).not.toHaveBeenCalled();
    });

    it("calls onZeroTabs when startup has no restored tabs", async () => {
      const openNew = vi.fn();
      const onZeroTabs = vi.fn();

      await initializeDesktopTabs({
        getTabs: () => [],
        setRecentlyOpenedSessionIds: vi.fn(),
        restorePinnedTabs: vi.fn().mockResolvedValue(undefined),
        restoreRecentlyOpenedSessionIds: vi.fn().mockResolvedValue(undefined),
        onZeroTabs,
        isTauriEnv: true,
      });

      expect(openNew).not.toHaveBeenCalled();
      expect(onZeroTabs).toHaveBeenCalledTimes(1);
    });

    it("runs startup work even when pinned tabs were restored", async () => {
      const tabs = [createSessionTab({ id: "restored-session" })];
      const onInitialized = vi.fn();
      const onZeroTabs = vi.fn();

      await initializeDesktopTabs({
        getTabs: () => tabs,
        setRecentlyOpenedSessionIds: vi.fn(),
        restorePinnedTabs: vi.fn().mockResolvedValue(undefined),
        restoreRecentlyOpenedSessionIds: vi.fn().mockResolvedValue(undefined),
        onInitialized,
        onZeroTabs,
        isTauriEnv: true,
      });

      expect(onInitialized).toHaveBeenCalledTimes(1);
      expect(onZeroTabs).not.toHaveBeenCalled();
    });
  });

  describe("createSessionTabCloseHandler", () => {
    it("cleans up empty sessions on close", async () => {
      const invalidateSessionResource = vi.fn();
      const discardEmptySessionFn = vi.fn().mockResolvedValue(true);
      const handler = createSessionTabCloseHandler({
        invalidateSessionResource,
        getSessionMode: vi.fn().mockReturnValue(null),
        discardEmptySessionFn,
      });

      handler(createSessionTab({ id: "session-1" }));

      await flushAsyncCleanup();

      expect(invalidateSessionResource).toHaveBeenCalledWith("session-1");
      expect(discardEmptySessionFn).toHaveBeenCalledWith("session-1");
    });

    it("keeps the tab's resource when the store keeps the session", async () => {
      const invalidateSessionResource = vi.fn();
      const discardEmptySessionFn = vi.fn().mockResolvedValue(false);
      const handler = createSessionTabCloseHandler({
        invalidateSessionResource,
        getSessionMode: vi.fn().mockReturnValue(null),
        discardEmptySessionFn,
      });

      handler(createSessionTab({ id: "session-1" }));

      await flushAsyncCleanup();

      expect(discardEmptySessionFn).toHaveBeenCalledWith("session-1");
      expect(invalidateSessionResource).not.toHaveBeenCalled();
    });

    it("skips cleanup for non-inactive sessions and non-session tabs", () => {
      for (const sessionMode of ["active", "finalizing", "running_batch"]) {
        const invalidateSessionResource = vi.fn();
        const discardEmptySessionFn = vi.fn();
        const handler = createSessionTabCloseHandler({
          invalidateSessionResource,
          getSessionMode: vi.fn().mockReturnValue(sessionMode),
          discardEmptySessionFn,
        });

        handler(createSessionTab({ id: `session-${sessionMode}` }));
        handler(createSettingsTab());

        expect(invalidateSessionResource).not.toHaveBeenCalled();
        expect(discardEmptySessionFn).not.toHaveBeenCalled();
      }
    });
  });
});
