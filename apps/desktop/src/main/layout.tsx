import { useRouteContext } from "@tanstack/react-router";

import { TaskStorageProvider } from "@hypr/editor/task-storage";

import { ClassicMainServices } from "./lifecycle";

import { AITaskProvider } from "~/ai/contexts";
import { NotificationProvider } from "~/contexts/notifications";
import { ShellProvider } from "~/contexts/shell";
import { useStoreBackedTaskStorage } from "~/editor-bridge/task-storage";
import { SearchEngineProvider } from "~/search/contexts/engine";
import { LocalVaultProvider } from "~/shared/application/vault-provider";
import { OpenNoteDialogProvider } from "~/shared/open-note-dialog";

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
    <SearchEngineProvider>
      <TaskStorageProvider storage={taskStorage}>
        <OpenNoteDialogProvider>
          <ShellProvider>
            <AITaskProvider store={aiTaskStore}>
              <NotificationProvider>
                {includeServices ? <ClassicMainServices /> : null}
                <LocalVaultProvider>{children}</LocalVaultProvider>
              </NotificationProvider>
            </AITaskProvider>
          </ShellProvider>
        </OpenNoteDialogProvider>
      </TaskStorageProvider>
    </SearchEngineProvider>
  );
}
