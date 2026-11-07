import "./test-matchers";

import { beforeEach, describe, expect, test } from "vitest";

import type { Tab } from "./schema";
import { tabsTestStore as useTabs } from "./test-utils";
import {
  createSessionTab,
  createSettingsTab,
  resetTabsStore,
} from "./test-utils";

describe("State Updater Actions", () => {
  beforeEach(() => {
    resetTabsStore();
  });

  describe("updateSessionTabState", () => {
    test("updates matching session tab and current tab state", () => {
      const tab = createSessionTab({ active: true });
      useTabs.getState().openNew(tab);

      useTabs.getState().updateSessionTabState(tab, {
        ...tab.state,
        view: { type: "enhanced", id: "note-1" },
      });

      const state = useTabs.getState();
      expect(state.tabs[0]).toMatchObject({
        id: tab.id,
        state: { view: { type: "enhanced", id: "note-1" }, autoStart: null },
      });
      expect(useTabs.getState()).toHaveCurrentTab({
        id: tab.id,
        state: { view: { type: "enhanced", id: "note-1" }, autoStart: null },
      });
      expect(useTabs.getState()).toHaveLastHistoryEntry({
        state: { view: { type: "enhanced", id: "note-1" }, autoStart: null },
      });
    });

    test("updates only matching tab instances", () => {
      const tab = createSessionTab({ active: false });
      const active = createSessionTab({ active: true });
      useTabs.getState().openNew(tab);
      useTabs.getState().openNew(active);

      useTabs.getState().updateSessionTabState(tab, {
        ...tab.state,
        view: { type: "enhanced", id: "note-1" },
      });

      const state = useTabs.getState();
      expect(state.tabs[0]).toMatchObject({
        id: tab.id,
        state: { view: { type: "enhanced", id: "note-1" } },
      });
      expect(state.tabs[1]).toMatchObject({
        id: active.id,
        state: { view: null, autoStart: null },
      });
      expect(useTabs.getState()).toHaveLastHistoryEntry({
        id: active.id,
        state: { view: null, autoStart: null },
      });
    });

    test("no-op when tab types mismatch", () => {
      const session = createSessionTab({ active: true });
      const settings = createSettingsTab();
      useTabs.getState().openNew(session);
      useTabs.getState().openNew(settings);

      useTabs
        .getState()
        .updateSessionTabState(settings as Tab, { view: "enhanced" } as any);

      const state = useTabs.getState();
      expect(state.tabs[0]).toMatchObject({
        id: session.id,
        state: { view: null, autoStart: null },
      });
      expect(state.tabs[1]).toMatchObject({ type: "settings" });
    });
  });
});
