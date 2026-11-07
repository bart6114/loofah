import type { StateCreator } from "zustand";
import { createStore } from "zustand/vanilla";

import { type BasicActions, type BasicState, createBasicSlice } from "./basic";
import {
  createLifecycleSlice,
  type LifecycleActions,
  lifecycleMiddleware,
  type LifecycleState,
} from "./lifecycle";
import {
  createNavigationSlice,
  type NavigationActions,
  navigationMiddleware,
  type NavigationState,
} from "./navigation";
import {
  createRecentlyOpenedSlice,
  type RecentlyOpenedActions,
  type RecentlyOpenedState,
} from "./recently-opened";
import {
  createRestoreSlice,
  type RestoreActions,
  restoreMiddleware,
  type RestoreState,
} from "./restore";
import { createStateUpdaterSlice, type StateBasicActions } from "./state";

import { wrapSliceWithLogging } from "~/store/zustand/shared";

export type TabsState = BasicState &
  NavigationState &
  LifecycleState &
  RestoreState &
  RecentlyOpenedState &
  BasicActions &
  StateBasicActions &
  NavigationActions &
  LifecycleActions &
  RestoreActions &
  RecentlyOpenedActions;

export const tabsState: StateCreator<TabsState> = restoreMiddleware(
  lifecycleMiddleware(
    navigationMiddleware((set, get) => ({
      ...wrapSliceWithLogging("basic", createBasicSlice(set, get)),
      ...wrapSliceWithLogging("state", createStateUpdaterSlice(set, get)),
      ...wrapSliceWithLogging("navigation", createNavigationSlice(set, get)),
      ...wrapSliceWithLogging("lifecycle", createLifecycleSlice(set, get)),
      ...wrapSliceWithLogging("restore", createRestoreSlice(set, get)),
      ...wrapSliceWithLogging(
        "recentlyOpened",
        createRecentlyOpenedSlice(set, get),
      ),
    })),
  ),
);

export function createTabsStore(scope: string, isActive: () => boolean) {
  const store = createStore<TabsState>()((set, get, api) =>
    tabsState(
      (partial, replace) => {
        if (isActive()) set(partial, replace as false);
      },
      get,
      api,
    ),
  );
  return {
    ...store,
    scope,
    setState: ((partial, replace) => {
      if (isActive()) store.setState(partial, replace as false);
    }) as typeof store.setState,
    clear: () => store.setState(store.getInitialState(), true),
  } as const;
}

export type ScopedTabsStore = ReturnType<typeof createTabsStore>;
