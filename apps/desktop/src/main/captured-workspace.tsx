import { Trans } from "@lingui/react/macro";
import { useMemo, type ReactNode } from "react";

import type { MainCaptureActivity } from "./host-context";
import { CapturedMainLayout } from "./layout";
import { useCapturedMainLifecycle } from "./lifecycle";
import { CapturedMainShellFrame } from "./shell-frame";

import { useVaultReadScope } from "~/shared/application/use-vault-read-scope";
import type { VaultClient } from "~/shared/application/vault-client";
import {
  VaultClientProvider,
  useVaultHost,
} from "~/shared/application/vault-provider";
import type { VaultScope } from "~/shared/application/vault-scope";
import { UndoDeleteToast } from "~/sidebar/toast/undo-delete-toast";
import { createAITaskStore } from "~/store/zustand/ai-task";
import { createListenerStore } from "~/store/zustand/listener";
import type { TabsDeviceState } from "~/store/zustand/tabs/device-state";

export function MainWorkspace({
  client,
  deviceState,
  managePins = true,
  auxiliary,
  onCatalogChanged,
}: {
  client: VaultClient;
  onCatalogChanged?: () => void;
  deviceState?: TabsDeviceState;
  managePins?: boolean;
  auxiliary?: ReactNode;
}) {
  return (
    <VaultClientProvider
      client={client}
      onChange={(change) => {
        if (change.kind === "catalog_changed") onCatalogChanged?.();
      }}
    >
      <ConnectedWorkspace
        deviceState={deviceState}
        managePins={managePins}
        auxiliary={auxiliary}
      />
    </VaultClientProvider>
  );
}

function ConnectedWorkspace({
  deviceState,
  managePins,
  auxiliary,
}: {
  deviceState?: TabsDeviceState;
  managePins: boolean;
  auxiliary?: ReactNode;
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
    <CapturedMainWorkspace
      scope={scope}
      deviceState={deviceState}
      managePins={managePins}
      auxiliary={auxiliary}
    />
  );
}

export function CapturedMainWorkspace({
  scope,
  activity,
  deviceState,
  managePins = true,
  auxiliary,
}: {
  scope: VaultScope;
  activity?: MainCaptureActivity;
  deviceState?: TabsDeviceState;
  managePins?: boolean;
  auxiliary?: ReactNode;
}) {
  const stores = useMemo(
    () => ({ scope, ai: createAITaskStore(), listener: createListenerStore() }),
    [scope],
  );
  return (
    <CapturedMainLayout
      scope={scope}
      aiTaskStore={stores.ai}
      listenerStore={stores.listener}
      activity={activity}
      deviceState={deviceState}
      managePins={managePins}
    >
      <WorkspaceContent auxiliary={auxiliary} />
    </CapturedMainLayout>
  );
}

function WorkspaceContent({ auxiliary }: { auxiliary?: ReactNode }) {
  const owner = useVaultReadScope();
  if (!owner.current())
    return (
      <div role="alert">
        <Trans>Vault unavailable</Trans>
      </div>
    );
  if (!owner.readable)
    return (
      <div role="status">
        <Trans>Loading content…</Trans>
      </div>
    );
  return <WorkspaceShell auxiliary={auxiliary} />;
}

function WorkspaceShell({ auxiliary }: { auxiliary?: ReactNode }) {
  useCapturedMainLifecycle();
  return (
    <>
      <CapturedMainShellFrame auxiliary={auxiliary} />
      <UndoDeleteToast />
    </>
  );
}
