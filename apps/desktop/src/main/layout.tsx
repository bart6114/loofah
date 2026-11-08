import { Trans } from "@lingui/react/macro";
import { useRouteContext } from "@tanstack/react-router";
import { useEffect, useMemo, useState } from "react";

import { MainHostContext, type MainCaptureActivity } from "./host-context";
import {
  ClassicMainServices,
  createLocalMainCaptureActivity,
} from "./lifecycle";

import { AITaskProvider } from "~/ai/contexts";
import { NotificationProvider } from "~/contexts/notifications";
import { ShellProvider } from "~/contexts/shell";
import { takePendingWelcomeSession } from "~/onboarding/welcome-note";
import { SearchEngineProvider } from "~/search/contexts/engine";
import { deviceVaultState } from "~/shared/application/device-vault-state";
import {
  LocalVaultProvider,
  VaultHostContext,
  useVaultHost,
} from "~/shared/application/vault-provider";
import type { VaultScope } from "~/shared/application/vault-scope";
import { useNavigationEvents } from "~/shared/hooks/useNavigationEvents";
import { OpenNoteDialogProvider } from "~/shared/open-note-dialog";
import type { AITaskStore } from "~/store/zustand/ai-task";
import type { ListenerStore } from "~/store/zustand/listener";
import type { TabsDeviceState } from "~/store/zustand/tabs/device-state";
import { ScopedTabsProvider } from "~/store/zustand/tabs/scoped-provider";
import { ListenerContextProvider, useListenerStore } from "~/stt/contexts";

export function ClassicMainLayout({
  children,
  includeServices = true,
}: {
  children: React.ReactNode;
  includeServices?: boolean;
}) {
  const { aiTaskStore } = useRouteContext({ from: "__root__" });
  const listenerStore = useListenerStore();
  if (!aiTaskStore) return null;
  return (
    <LocalVaultProvider>
      <LocalMainHost
        aiTaskStore={aiTaskStore}
        listenerStore={listenerStore}
        includeServices={includeServices}
      >
        {children}
      </LocalMainHost>
    </LocalVaultProvider>
  );
}

function LocalMainHost({
  children,
  aiTaskStore,
  listenerStore,
  includeServices,
}: {
  children: React.ReactNode;
  aiTaskStore: AITaskStore;
  listenerStore: ListenerStore;
  includeServices: boolean;
}) {
  const { scope, error, retry } = useVaultHost();
  const activity = useMemo(
    () =>
      scope ? createLocalMainCaptureActivity(scope, listenerStore) : undefined,
    [scope, listenerStore],
  );
  if (!scope)
    return (
      <div role={error ? "alert" : "status"}>
        {error ? (
          <>
            <p>{error.message}</p>
            <button onClick={retry}>
              <Trans>Retry</Trans>
            </button>
          </>
        ) : (
          <Trans>Loading content…</Trans>
        )}
      </div>
    );
  return (
    <CapturedMainLayout
      scope={scope}
      aiTaskStore={aiTaskStore}
      listenerStore={listenerStore}
      managePins={includeServices}
      activity={activity}
      takeInitialSession={takePendingWelcomeSession}
    >
      <NotificationProvider>
        <MainNavigationEvents />
        {includeServices ? <ClassicMainServices /> : null}
        {children}
      </NotificationProvider>
    </CapturedMainLayout>
  );
}

const noop = () => {};

export function CapturedMainLayout({
  scope,
  aiTaskStore,
  listenerStore,
  managePins = true,
  deviceState = deviceVaultState,
  activity,
  takeInitialSession,
  children,
}: {
  scope: VaultScope;
  aiTaskStore: AITaskStore;
  listenerStore: ListenerStore;
  managePins?: boolean;
  deviceState?: TabsDeviceState;
  activity?: MainCaptureActivity;
  takeInitialSession?: () => string | null;
  children: React.ReactNode;
}) {
  const [owner, setOwner] = useState<{
    scope: VaultScope;
    aiTaskStore: AITaskStore;
    listenerStore: ListenerStore;
  } | null>(null);
  useEffect(() => {
    setOwner({ scope, aiTaskStore, listenerStore });
  }, [scope, aiTaskStore, listenerStore]);
  // The AI provider pins its store on mount, so replaced host inputs must remount together.
  if (
    owner?.scope !== scope ||
    owner.aiTaskStore !== aiTaskStore ||
    owner.listenerStore !== listenerStore
  )
    return null;
  return (
    <VaultHostContext.Provider value={{ scope, error: null, retry: noop }}>
      <ScopedTabsProvider
        scope={scope}
        deviceState={deviceState}
        managePins={managePins}
      >
        <MainHostContext.Provider
          value={{ scope, activity, takeInitialSession }}
        >
          <ListenerContextProvider store={listenerStore}>
            <SearchEngineProvider>
              <OpenNoteDialogProvider>
                <ShellProvider>
                  <AITaskProvider store={aiTaskStore}>
                    {children}
                  </AITaskProvider>
                </ShellProvider>
              </OpenNoteDialogProvider>
            </SearchEngineProvider>
          </ListenerContextProvider>
        </MainHostContext.Provider>
      </ScopedTabsProvider>
    </VaultHostContext.Provider>
  );
}

function MainNavigationEvents() {
  useNavigationEvents();
  return null;
}
