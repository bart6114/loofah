import { Trans } from "@lingui/react/macro";
import { useRouteContext } from "@tanstack/react-router";

import { TaskStorageProvider } from "@hypr/editor/task-storage";

import { ClassicMainServices } from "./lifecycle";

import { AITaskProvider } from "~/ai/contexts";
import { NotificationProvider } from "~/contexts/notifications";
import { ShellProvider } from "~/contexts/shell";
import { useStoreBackedTaskStorage } from "~/editor-bridge/task-storage";
import { SearchEngineProvider } from "~/search/contexts/engine";
import { deviceVaultState } from "~/shared/application/device-vault-state";
import {
  LocalVaultProvider,
  useVaultHost,
} from "~/shared/application/vault-provider";
import { useNavigationEvents } from "~/shared/hooks/useNavigationEvents";
import { OpenNoteDialogProvider } from "~/shared/open-note-dialog";
import { ScopedTabsProvider } from "~/store/zustand/tabs/scoped-provider";

export function ClassicMainLayout({
  children,
  includeServices = true,
}: {
  children: React.ReactNode;
  includeServices?: boolean;
}) {
  const { aiTaskStore } = useRouteContext({
    from: "__root__",
  });
  const taskStorage = useStoreBackedTaskStorage();

  if (!aiTaskStore) {
    return null;
  }

  return (
    <LocalVaultProvider>
      <LocalTabsHost managePins={includeServices}>
        <SearchEngineProvider>
          <TaskStorageProvider storage={taskStorage}>
            <OpenNoteDialogProvider>
              <ShellProvider>
                <AITaskProvider store={aiTaskStore}>
                  <NotificationProvider>
                    <LocalNavigationEvents />
                    {includeServices ? <ClassicMainServices /> : null}
                    {children}
                  </NotificationProvider>
                </AITaskProvider>
              </ShellProvider>
            </OpenNoteDialogProvider>
          </TaskStorageProvider>
        </SearchEngineProvider>
      </LocalTabsHost>
    </LocalVaultProvider>
  );
}

function LocalTabsHost({
  children,
  managePins,
}: {
  children: React.ReactNode;
  managePins: boolean;
}) {
  const { scope, error, retry } = useVaultHost();
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
    <ScopedTabsProvider
      key={scope.client.scope}
      scope={scope}
      deviceState={deviceVaultState}
      managePins={managePins}
    >
      {children}
    </ScopedTabsProvider>
  );
}

function LocalNavigationEvents() {
  useNavigationEvents();
  return null;
}
