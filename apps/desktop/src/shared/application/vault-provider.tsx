import { useQuery } from "@tanstack/react-query";
import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";

import { noteDrafts } from "./note-draft";
import { noteEditorDrafts } from "./note-editor-draft";
import type { Change } from "./protocol.gen";
import { connectLocalVault } from "./vault-client";
import type { VaultClient } from "./vault-client";
import { VaultScope } from "./vault-scope";

export const VaultHostContext = createContext<{
  scope: VaultScope | null;
  error: Error | null;
  retry: () => void;
} | null>(null);

export function useVaultHost() {
  const host = useContext(VaultHostContext);
  if (!host) throw new Error("The note editor requires a vault host");
  return host;
}

export function LocalVaultProvider({ children }: { children: ReactNode }) {
  const client = useQuery({
    queryKey: ["application", "local-vault"],
    queryFn: connectLocalVault,
    networkMode: "always",
    staleTime: Infinity,
    retry: 4,
  });
  return (
    <VaultClientProvider
      client={client.data}
      error={client.error}
      retry={() => {
        void client.refetch();
      }}
    >
      {children}
    </VaultClientProvider>
  );
}

export function VaultClientProvider({
  client,
  error = null,
  retry = () => {},
  onChange,
  children,
}: {
  client: VaultClient | undefined;
  error?: Error | null;
  retry?: () => void;
  onChange?: (change: Change) => void;
  children: ReactNode;
}) {
  const changed = useRef({ client, onChange });
  changed.current = { client, onChange };
  const [scope, setScope] = useState<VaultScope | null>(null);
  useEffect(() => {
    if (!client) return;
    const current = new VaultScope(client);
    noteDrafts.retainScope(client.scope);
    noteEditorDrafts.retainScope(client.scope);
    const stop = current.onChange((event) => {
      if (changed.current.client === client) changed.current.onChange?.(event);
    });
    setScope(current);
    void current.start().catch(() => {});
    return () => {
      stop();
      current.dispose();
      noteDrafts.releaseScope(current.client.scope);
      noteEditorDrafts.releaseScope(current.client.scope);
    };
  }, [client]);
  return (
    <VaultHostContext.Provider
      value={{
        scope: scope?.client === client ? scope : null,
        error,
        retry,
      }}
    >
      {children}
    </VaultHostContext.Provider>
  );
}
