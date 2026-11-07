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
    it("waits for captured scoped restoration before startup callbacks", async () => {
      const tabs = [createSessionTab({ id: "restored-session" })];
      const restore = vi.fn().mockResolvedValue(undefined);
      const onInitialized = vi.fn();
      const onZeroTabs = vi.fn();
      await initializeDesktopTabs({
        getTabs: () => tabs,
        restore,
        isActive: () => true,
        onInitialized,
        onZeroTabs,
      });
      expect(restore).toHaveBeenCalledOnce();
      expect(onInitialized).toHaveBeenCalledOnce();
      expect(onZeroTabs).not.toHaveBeenCalled();
    });

    it("opens the default tab after restoration finds no tabs", async () => {
      const onZeroTabs = vi.fn();
      await initializeDesktopTabs({
        getTabs: () => [],
        restore: async () => {},
        isActive: () => true,
        onZeroTabs,
      });
      expect(onZeroTabs).toHaveBeenCalledOnce();
    });

    it("does not apply delayed startup callbacks after host teardown", async () => {
      let finish!: () => void;
      let active = true;
      const onInitialized = vi.fn(),
        onZeroTabs = vi.fn();
      const startup = initializeDesktopTabs({
        getTabs: () => [],
        restore: () =>
          new Promise<void>((resolve) => {
            finish = resolve;
          }),
        isActive: () => active,
        onInitialized,
        onZeroTabs,
      });
      active = false;
      finish();
      await startup;
      expect(onInitialized).not.toHaveBeenCalled();
      expect(onZeroTabs).not.toHaveBeenCalled();
    });

    it("checks the captured host again if initialization retires it", async () => {
      let active = true;
      const onZeroTabs = vi.fn();
      await initializeDesktopTabs({
        getTabs: () => [],
        restore: async () => {},
        isActive: () => active,
        onInitialized: () => {
          active = false;
        },
        onZeroTabs,
      });
      expect(onZeroTabs).not.toHaveBeenCalled();
    });
  });

  describe("createSessionTabCloseHandler", () => {
    it("cleans up empty sessions on close", async () => {
      const invalidateSessionResource = vi.fn();
      const deleteSessionFn = vi.fn().mockResolvedValue({ session: {} });
      const handler = createSessionTabCloseHandler({
        invalidateSessionResource,
        getSessionMode: vi.fn().mockReturnValue(null),
        isSessionEmptyFn: vi.fn().mockResolvedValue(true),
        deleteSessionFn,
      });

      handler(createSessionTab({ id: "session-1" }));

      await flushAsyncCleanup();

      expect(invalidateSessionResource).toHaveBeenCalledWith("session-1");
      expect(deleteSessionFn).toHaveBeenCalledWith("session-1");
    });

    it("keeps sessions that contain stored data", async () => {
      const invalidateSessionResource = vi.fn();
      const deleteSessionFn = vi.fn();
      const isSessionEmptyFn = vi.fn().mockResolvedValue(false);
      const handler = createSessionTabCloseHandler({
        invalidateSessionResource,
        getSessionMode: vi.fn().mockReturnValue(null),
        isSessionEmptyFn,
        deleteSessionFn,
      });

      handler(createSessionTab({ id: "session-1" }));

      await flushAsyncCleanup();

      expect(deleteSessionFn).not.toHaveBeenCalled();
      expect(invalidateSessionResource).not.toHaveBeenCalled();
    });

    it("does not invalidate when the delete loses a race", async () => {
      const invalidateSessionResource = vi.fn();
      const deleteSessionFn = vi.fn().mockResolvedValue(null);
      const handler = createSessionTabCloseHandler({
        invalidateSessionResource,
        getSessionMode: vi.fn().mockReturnValue(null),
        isSessionEmptyFn: vi.fn().mockResolvedValue(true),
        deleteSessionFn,
      });

      handler(createSessionTab({ id: "session-1" }));

      await flushAsyncCleanup();

      expect(deleteSessionFn).toHaveBeenCalledWith("session-1");
      expect(invalidateSessionResource).not.toHaveBeenCalled();
    });

    it("skips cleanup for non-inactive sessions and non-session tabs", () => {
      for (const sessionMode of ["active", "finalizing", "running_batch"]) {
        const invalidateSessionResource = vi.fn();
        const deleteSessionFn = vi.fn();
        const handler = createSessionTabCloseHandler({
          invalidateSessionResource,
          getSessionMode: vi.fn().mockReturnValue(sessionMode),
          isSessionEmptyFn: vi.fn().mockResolvedValue(true),
          deleteSessionFn,
        });

        handler(createSessionTab({ id: `session-${sessionMode}` }));
        handler(createSettingsTab());

        expect(invalidateSessionResource).not.toHaveBeenCalled();
        expect(deleteSessionFn).not.toHaveBeenCalled();
      }
    });
  });
});
