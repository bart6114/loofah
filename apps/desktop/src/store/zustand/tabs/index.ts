import { create } from "zustand";

import {
  pinnedPersistenceMiddleware,
  restorePinnedTabsToStore,
} from "./pinned-persistence";
import {
  recentlyOpenedMiddleware,
  restoreRecentlyOpenedToStore,
} from "./recently-opened";
import { tabsState, type TabsState } from "./store";

export type {
  DailySummaryState,
  SettingsState,
  SettingsTab,
  Tab,
  TabInput,
  TaskResource,
} from "./schema";
export type { StateBasicActions } from "./state";
export {
  isSameTab,
  isTabInputSupported,
  normalizeSettingsTab,
  uniqueIdfromTab,
} from "./schema";
export { restorePinnedTabsToStore, restoreRecentlyOpenedToStore };

type TabsStoreSingleton = ReturnType<typeof createTabsStore>;

const TABS_STORE_KEY = "__hypr_tabs_store__" as const;

const createTabsStore = () =>
  create<TabsState>()(
    recentlyOpenedMiddleware(pinnedPersistenceMiddleware(tabsState)),
  );

const getTabsStore = (): TabsStoreSingleton => {
  if (!import.meta.hot) {
    return createTabsStore();
  }

  const hotData = (import.meta.hot.data ?? {}) as {
    [TABS_STORE_KEY]?: TabsStoreSingleton;
  };

  if (!hotData[TABS_STORE_KEY]) {
    hotData[TABS_STORE_KEY] = createTabsStore();
  }

  return hotData[TABS_STORE_KEY];
};

export const useTabs = getTabsStore();
