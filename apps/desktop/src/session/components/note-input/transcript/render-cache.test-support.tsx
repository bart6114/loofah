import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";

import type {
  VaultClient,
  VaultClientEvent,
} from "~/shared/application/vault-client";
import { VaultHostContext } from "~/shared/application/vault-provider";
import { VaultScope } from "~/shared/application/vault-scope";

export function createRenderCacheHost(
  scopeId = "local:transcript",
  networkMode: "always" | "online" = "always",
  overrides: Partial<VaultClient> = {},
) {
  let receive: ((event: VaultClientEvent) => void) | undefined;
  const scope = new VaultScope({
    scope: scopeId,
    networkMode,
    canEdit: true,
    features: [],
    subscribe: async (listener: (event: VaultClientEvent) => void) => {
      receive = listener;
      listener({ kind: "connection", state: "connected", error: null });
      return () => {
        receive = undefined;
      };
    },
    ...overrides,
  } as unknown as VaultClient);
  scope.queryClient.setDefaultOptions({ queries: { retry: false } });
  const rootClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  void scope.start();
  function Host({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={rootClient}>
        <VaultHostContext.Provider
          value={{ scope, error: null, retry: () => {} }}
        >
          {children}
        </VaultHostContext.Provider>
      </QueryClientProvider>
    );
  }
  return {
    scope,
    rootClient,
    Host,
    emit: (state: ReturnType<VaultScope["getSnapshot"]>["state"]) =>
      receive?.({ kind: "connection", state, error: null }),
    dispose: () => {
      scope.dispose();
      rootClient.clear();
    },
  };
}
