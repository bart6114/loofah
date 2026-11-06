import { useQuery } from "@tanstack/react-query";
import {
  createContext,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";

import { noteEditorDrafts } from "./note-editor-draft";
import { connectLocalVault } from "./vault-client";
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
  const [scope, setScope] = useState<VaultScope | null>(null);
  useEffect(() => {
    if (!client.data) return;
    const current = new VaultScope(client.data);
    noteEditorDrafts.retainScope(client.data.scope);
    setScope(current);
    void current.start().catch(() => {});
    return () => {
      current.dispose();
      noteEditorDrafts.releaseScope(current.client.scope);
    };
  }, [client.data]);
  return (
    <VaultHostContext.Provider
      value={{
        scope: scope?.client === client.data ? scope : null,
        error: client.error,
        retry: () => {
          void client.refetch();
        },
      }}
    >
      {children}
    </VaultHostContext.Provider>
  );
}
