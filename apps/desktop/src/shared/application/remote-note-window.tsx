import { Trans } from "@lingui/react/macro";
import { useQuery } from "@tanstack/react-query";
import { useEffect, useMemo } from "react";

import { getCurrentWebviewWindowLabel } from "@hypr/plugin-windows";

import { NativeRemoteConnection } from "./remote";
import { useVaultReadScope } from "./use-vault-read-scope";
import { connectRemoteVault } from "./vault-client";
import { VaultClientProvider, useVaultHost } from "./vault-provider";
import type { VaultScope } from "./vault-scope";

import { CapturedMainLayout } from "~/main/layout";
import { StandaloneNoteContent } from "~/routes/app/note.$sessionId";
import { createAITaskStore } from "~/store/zustand/ai-task";
import { createListenerStore } from "~/store/zustand/listener";

// Native consumes the grant once. React remounts must reuse that same claim,
// never reconnect or infer another workspace from the route parameter.
let originalClaim: ReturnType<typeof claim> | undefined;
function claim() {
  return NativeRemoteConnection.claimNoteWindow().then(
    ({ info, connection }) => ({
      connection,
      sessionId: info.session_id,
      client: connectRemoteVault(connection, info.member_id, info.vault),
    }),
  );
}
let owners = 0;

export function RemoteNoteWindow() {
  const bootstrap = useQuery({
    queryKey: ["remote-note-window", getCurrentWebviewWindowLabel()],
    queryFn: () => (originalClaim ??= claim()),
    staleTime: Infinity,
    retry: false,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
    networkMode: "always",
  });
  useEffect(() => {
    owners++;
    return () => {
      owners--;
      queueMicrotask(() => {
        if (owners === 0)
          void originalClaim
            ?.then(({ connection }) => connection.disconnect())
            .catch(() => {});
      });
    };
  }, []);
  if (bootstrap.error)
    return (
      <div role="alert">
        <Trans>
          This note window is no longer connected to its original workspace.
          Open it again from that workspace.
        </Trans>
      </div>
    );
  if (!bootstrap.data)
    return (
      <div role="status">
        <Trans>Opening note…</Trans>
      </div>
    );
  return (
    <VaultClientProvider client={bootstrap.data.client}>
      <WindowWorkspace sessionId={bootstrap.data.sessionId} />
    </VaultClientProvider>
  );
}
function WindowWorkspace({ sessionId }: { sessionId: string }) {
  const { scope, error } = useVaultHost();
  if (!scope)
    return (
      <div role={error ? "alert" : "status"}>
        {error?.message ?? <Trans>Opening note…</Trans>}
      </div>
    );
  return (
    <OriginalWorkspace
      key={scope.client.scope}
      scope={scope}
      sessionId={sessionId}
    />
  );
}
function OriginalWorkspace({
  scope,
  sessionId,
}: {
  scope: VaultScope;
  sessionId: string;
}) {
  const stores = useMemo(
    () => ({ ai: createAITaskStore(), listener: createListenerStore() }),
    [scope],
  );
  return (
    <CapturedMainLayout
      scope={scope}
      aiTaskStore={stores.ai}
      listenerStore={stores.listener}
      managePins={false}
    >
      <OriginalNote sessionId={sessionId} />
    </CapturedMainLayout>
  );
}
function OriginalNote({ sessionId }: { sessionId: string }) {
  const owner = useVaultReadScope();
  if (!owner.current())
    return (
      <div role="alert">
        <Trans>This note's workspace is no longer available.</Trans>
      </div>
    );
  if (!owner.readable)
    return (
      <div role="status">
        <Trans>Opening note…</Trans>
      </div>
    );
  return <StandaloneNoteContent sessionId={sessionId} />;
}
