import { describe, expect, it, vi } from "vitest";

import { initializeDesktopTabs } from "./desktop-tab-lifecycle";

import { createSessionTab } from "~/store/zustand/tabs/test-utils";

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
});
