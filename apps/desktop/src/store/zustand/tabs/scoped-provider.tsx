import { createContext, useContext, useEffect, useState } from "react";
import { useStore } from "zustand";

import type { TabsDeviceState } from "./device-state";
import { ScopedTabsController } from "./scoped-controller";
import type { ScopedTabsStore, TabsState } from "./store";

import type { VaultScope } from "~/shared/application/vault-scope";
import {
  createTimelineSelectionStore,
  TimelineSelectionContext,
} from "~/store/zustand/timeline-selection";

const ScopedTabsContext = createContext<ScopedTabsController | null>(null);
const TabsStoreContext = createContext<ScopedTabsStore | null>(null);

export function TabsStoreProvider({
  store,
  children,
}: {
  store: ScopedTabsStore;
  children?: React.ReactNode;
}) {
  const [current, setCurrent] = useState<{
    owner: ScopedTabsStore;
    selection: ReturnType<typeof createTimelineSelectionStore>;
  } | null>(null);
  useEffect(() => {
    const selection = createTimelineSelectionStore();
    setCurrent({ owner: store, selection });
    return () => selection.dispose();
  }, [store]);
  if (current?.owner !== store) return null;
  return (
    <TabsStoreContext.Provider value={store}>
      <TimelineSelectionContext.Provider value={current.selection}>
        {children}
      </TimelineSelectionContext.Provider>
    </TabsStoreContext.Provider>
  );
}

export function ScopedTabsProvider({
  scope,
  deviceState,
  managePins,
  children,
}: {
  scope: VaultScope;
  deviceState: TabsDeviceState;
  managePins: boolean;
  children: React.ReactNode;
}) {
  const [current, setCurrent] = useState<{
    controller: ScopedTabsController;
    deviceState: TabsDeviceState;
    managePins: boolean;
  } | null>(null);
  useEffect(() => {
    const next = new ScopedTabsController(scope, deviceState, managePins);
    setCurrent({ controller: next, deviceState, managePins });
    void next.start().catch(console.error);
    return () => next.dispose();
  }, [scope, deviceState, managePins]);
  if (
    current?.controller.scope !== scope ||
    current.deviceState !== deviceState ||
    current.managePins !== managePins
  )
    return null;
  return (
    <ScopedTabsContext.Provider value={current.controller}>
      <TabsStoreProvider store={current.controller.store}>
        {children}
      </TabsStoreProvider>
    </ScopedTabsContext.Provider>
  );
}

export function useScopedTabsController() {
  const controller = useContext(ScopedTabsContext);
  if (!controller) throw new Error("Tabs require an immutable vault host");
  return controller;
}

export function useScopedTabsStore() {
  const store = useContext(TabsStoreContext);
  if (!store) throw new Error("Tabs require an immutable vault host");
  return store;
}

export function useScopedTabs(): TabsState;
export function useScopedTabs<T>(selector: (state: TabsState) => T): T;
export function useScopedTabs(selector?: (state: TabsState) => unknown) {
  return useStore(useScopedTabsStore(), selector ?? ((state) => state));
}
