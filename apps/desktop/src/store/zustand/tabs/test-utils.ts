import { createElement, type ReactNode } from "react";

import { computeHistoryFlags } from "./navigation";
import type { Tab } from "./schema";
import { TabsStoreProvider } from "./scoped-provider";
import { createTabsStore } from "./store";

import { id } from "~/shared/utils";

type SessionTab = Extract<Tab, { type: "sessions" }>;
type SettingsTab = Extract<Tab, { type: "settings" }>;

type SessionOverrides = Partial<Omit<SessionTab, "type" | "state">> & {
  state?: Partial<SessionTab["state"]>;
};

type SettingsOverrides = Partial<Omit<SettingsTab, "type" | "state">> & {
  state?: Partial<SettingsTab["state"]>;
};

export const createSessionTab = (
  overrides: SessionOverrides = {},
): SessionTab => ({
  type: "sessions",
  id: overrides.id ?? id(),
  active: overrides.active ?? false,
  pinned: overrides.pinned ?? false,
  slotId: id(),
  state: {
    view: null,
    autoStart: null,
    ...overrides.state,
  },
});

export const createSettingsTab = (
  overrides: SettingsOverrides = {},
): SettingsTab => ({
  type: "settings",
  active: overrides.active ?? false,
  pinned: overrides.pinned ?? false,
  slotId: overrides.slotId ?? id(),
  state: {
    tab: "app",
    ...overrides.state,
  },
});

export const tabsTestStore = createTabsStore("local:tabs-test", () => true);
export function TabsTestProvider({ children }: { children: ReactNode }) {
  return createElement(TabsStoreProvider, { store: tabsTestStore }, children);
}

type TabsStore = ReturnType<typeof tabsTestStore.getState>;
type TabsStateSlice = Pick<
  TabsStore,
  | "currentTab"
  | "tabs"
  | "history"
  | "canGoBack"
  | "canGoNext"
  | "onClose"
  | "onEmpty"
  | "closedTabs"
>;

const createDefaultTabsState = (): TabsStateSlice => ({
  currentTab: null,
  tabs: [],
  history: new Map(),
  canGoBack: false,
  canGoNext: false,
  onClose: null,
  onEmpty: null,
  closedTabs: [],
});

export const seedTabsStore = (
  overrides: Partial<TabsStateSlice> = {},
): void => {
  tabsTestStore.clear();
  const state = { ...createDefaultTabsState(), ...overrides };
  tabsTestStore.setState(() => state);
  const flags = computeHistoryFlags(state.history, state.currentTab);
  tabsTestStore.setState(() => flags);
};

export const resetTabsStore = (): void => {
  seedTabsStore();
};
