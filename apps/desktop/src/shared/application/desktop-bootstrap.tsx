import { Trans } from "@lingui/react/macro";
import { useMutation } from "@tanstack/react-query";
import { useCallback, useState, type ReactNode } from "react";

import { DesktopStartupPreferences } from "./desktop-startup-preferences";
import {
  DeviceRecordingHost,
  type DeviceRecordingDestination,
} from "./device-recording-host";
import { DeviceRecovery } from "./device-recovery";
import { applicationResult } from "./errors";
import { NativeRemoteConnection } from "./remote";
import { ServerConnection } from "./server-preview";
import { remoteVaultScope } from "./vault-client";

import { commands, type DesktopConnectionProfile } from "~/types/tauri.gen";

export async function loadDesktopBootstrap() {
  try {
    return {
      info: applicationResult(await commands.applicationDesktopBootstrap()),
      error: null,
    };
  } catch (error) {
    return {
      info: null,
      error: error instanceof Error ? error : new Error(String(error)),
    };
  }
}

export function DesktopBootstrapRoot({
  state,
  children,
}: {
  state: Awaited<ReturnType<typeof loadDesktopBootstrap>>;
  children: ReactNode;
}) {
  if (state.info?.startup.selection.kind === "local") return children;
  if (state.info?.connection)
    return <RemoteDesktop connection={state.info.connection} />;
  return (
    <div className="bg-background flex h-screen flex-col gap-4 px-6 pt-12 pb-6">
      <h1 className="text-xl font-semibold">
        <Trans>Unable to open the saved startup choice</Trans>
      </h1>
      <p role="alert">{state.error?.message}</p>
      <DesktopStartupPreferences />
      <DeviceRecovery />
    </div>
  );
}

function RemoteDesktop({
  connection,
}: {
  connection: DesktopConnectionProfile;
}) {
  const [destination, setDestination] =
    useState<DeviceRecordingDestination | null>(null);
  const remember = useMutation({
    scope: { id: `desktop-destination:${connection.id}` },
    mutationFn: async ({
      nativeId,
      member,
      vault,
    }: {
      nativeId: string;
      member: string;
      vault: string;
    }) =>
      applicationResult(
        await commands.applicationRememberDesktopDestination(
          nativeId,
          connection.id,
          member,
          vault,
        ),
      ),
    onSuccess: (_value, target) =>
      setDestination({
        scopeKey: remoteVaultScope(
          connection.server,
          target.member,
          target.vault,
        ),
        scope: {
          server: connection.server,
          member_id: target.member,
          vault_id: target.vault,
        },
        connectionId: target.nativeId,
      }),
  });
  const rememberDestination = remember.mutate;
  const saveDestination = useCallback(
    (native: NativeRemoteConnection, member: string, vault: string) => {
      if (native.info.server === connection.server)
        rememberDestination({
          nativeId: native.info.connection_id,
          member,
          vault,
        });
    },
    [connection.server, rememberDestination],
  );
  const openConnection = useCallback(
    () =>
      NativeRemoteConnection.connectDesktopProfile(
        connection.name,
        connection.id,
      ),
    [connection.name, connection.id],
  );
  return (
    <div className="bg-background flex h-screen min-h-0 flex-col gap-4 px-6 pt-10 pb-4">
      <header
        className="flex items-center justify-between gap-3"
        data-tauri-drag-region
      >
        <div className="flex flex-col gap-1">
          <span className="text-sm font-semibold">
            Loofah · {connection.name}
          </span>
          <span className="text-muted-foreground text-xs">
            <Trans>Experimental server workspace</Trans>
          </span>
        </div>
        <DesktopStartupPreferences />
      </header>
      {remember.error && (
        <p role="alert" className="text-destructive text-sm">
          {remember.error.message}
        </p>
      )}
      <DeviceRecordingHost profileId={connection.id} destination={destination}>
        <ServerConnection
          initialServer={connection.server}
          autoConnect
          connectToServer={openConnection}
          preferredMemberId={connection.member_id}
          preferredVaultId={connection.vault_id}
          onDestination={saveDestination}
        />
      </DeviceRecordingHost>
    </div>
  );
}
