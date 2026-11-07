import { createContext, useContext, useEffect, useState } from "react";
import { useStore } from "zustand";

import type { TabsDeviceState } from "./device-state";
import { ScopedTabsController } from "./scoped-controller";
import type { TabsState } from "./store";

import type { VaultScope } from "~/shared/application/vault-scope";

const ScopedTabsContext = createContext<ScopedTabsController | null>(null);

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
      {children}
    </ScopedTabsContext.Provider>
  );
}

export function useScopedTabsController() {
  const controller = useContext(ScopedTabsContext);
  if (!controller) throw new Error("Tabs require an immutable vault host");
  return controller;
}

export function useScopedTabsStore() {
  return useScopedTabsController().store;
}

export function useScopedTabs(): TabsState;
export function useScopedTabs<T>(selector: (state: TabsState) => T): T;
export function useScopedTabs(selector?: (state: TabsState) => unknown) {
  return useStore(useScopedTabsStore(), selector ?? ((state) => state));
}
