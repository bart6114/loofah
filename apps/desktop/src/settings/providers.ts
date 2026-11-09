import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import type { AiProviderEntry, JsonValue } from "@hypr/plugin-settings";
import { commands as store2Commands } from "@hypr/plugin-store2";

import {
  fetchConfig,
  useConfigStoreState,
  writeConfigValues,
} from "~/shared/config/store";

export type AiProviderType = "llm" | "stt";

export type AiProviderConfig = {
  type: AiProviderType;
  base_url: string;
  api_key: string;
};

const PROVIDER_SECRET_SCOPE = "ai-provider-api-keys";
const MACOS_KEYCHAIN_ACCESS_ERROR_PREFIX =
  "macOS couldn't access your login Keychain.";
const EMPTY_PROVIDER_API_KEYS: Record<string, string> = {};
const EMPTY_PROVIDER_ENTRIES: Partial<Record<string, AiProviderEntry>> = {};

export function useAiProviders(
  type: AiProviderType,
  enabled = true,
): Record<string, AiProviderConfig> {
  return useAiProvidersState(type, enabled).providers;
}

export function useAiProvidersState(
  type: AiProviderType,
  enabled = true,
): {
  providers: Record<string, AiProviderConfig>;
  isReady: boolean;
} {
  const { snapshot, isLoading } = useConfigStoreState();
  const providers = enabled
    ? parseAiProviders(
        snapshot?.config.ai_providers ?? EMPTY_PROVIDER_ENTRIES,
        type,
      )
    : {};
  const providerIds = Object.keys(providers).sort();
  const secureApiKeysQuery = useQuery({
    queryKey: ["ai-provider-api-keys", type, providerIds],
    queryFn: () => loadSecureAiProviderApiKeys(providerIds, type),
    enabled: enabled && !isLoading,
    staleTime: Infinity,
  });
  const secureApiKeys = secureApiKeysQuery.data ?? EMPTY_PROVIDER_API_KEYS;

  return {
    providers: Object.fromEntries(
      Object.entries(providers).map(([rowId, provider]) => [
        rowId,
        {
          ...provider,
          api_key: secureApiKeys[rowId] ?? provider.api_key,
        },
      ]),
    ),
    isReady: enabled && !isLoading && secureApiKeysQuery.data !== undefined,
  };
}

export function useAiProvider(
  type: AiProviderType,
  providerId: string | null | undefined,
  enabled = true,
): AiProviderConfig | undefined {
  const providers = useAiProviders(type, enabled);
  return providerId ? providers[providerRowId(type, providerId)] : undefined;
}

export async function getStoredAiProvider(
  type: AiProviderType,
  providerId: string,
): Promise<AiProviderConfig | undefined> {
  const config = await fetchConfig();
  const provider = parseAiProviders(config.ai_providers, type)[
    providerRowId(type, providerId)
  ];
  if (!provider) return undefined;

  const secureApiKey = await getProviderApiKey(type, providerId);
  return {
    ...provider,
    api_key: secureApiKey ?? provider.api_key,
  };
}

export async function setAiProvider(
  type: AiProviderType,
  providerId: string,
  changes: Partial<Pick<AiProviderConfig, "base_url" | "api_key">>,
): Promise<void> {
  const rowId = providerRowId(type, providerId);
  const previousApiKey = await getProviderApiKey(type, providerId);

  try {
    const config = await fetchConfig();
    const current = parseAiProviders(config.ai_providers, type)[rowId];
    const next: AiProviderConfig = {
      type,
      base_url: changes.base_url ?? current?.base_url ?? "",
      api_key: changes.api_key ?? previousApiKey ?? "",
    };
    await setProviderApiKey(type, providerId, next.api_key);

    const entries: Partial<Record<string, AiProviderEntry>> = {
      ...config.ai_providers,
      [rowId]: { type, base_url: next.base_url },
    };
    await writeConfigValues({ ai_providers: entries as JsonValue });
  } catch (error) {
    await setProviderApiKey(type, providerId, previousApiKey ?? "");
    throw error;
  }
}

export function useSetAiProvider(type: AiProviderType, providerId: string) {
  const queryClient = useQueryClient();

  return useMutation({
    mutationKey: ["set-ai-provider", type, providerId],
    mutationFn: (
      changes: Partial<Pick<AiProviderConfig, "base_url" | "api_key">>,
    ) => setAiProvider(type, providerId, changes),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({
          queryKey: ["ai-provider-api-keys", type],
        }),
        queryClient.invalidateQueries({
          queryKey: ["default-ai-selection", type],
        }),
      ]);
    },
  });
}

export function isKeychainAccessError(error: unknown): boolean {
  return (
    error instanceof Error &&
    error.message.startsWith(MACOS_KEYCHAIN_ACCESS_ERROR_PREFIX)
  );
}

export async function repairKeychainAccess(): Promise<void> {
  const result = await store2Commands.repairKeychainAccess();
  if (result.status === "error") {
    throw new Error(result.error);
  }
}

export async function loadSecureAiProviderApiKeys(
  providerRowIds: string[],
  type: AiProviderType,
): Promise<Record<string, string>> {
  const apiKeys: Record<string, string> = {};

  for (const rowId of providerRowIds) {
    const providerId = rowId.slice(`${type}:`.length);
    const apiKey = await getProviderApiKey(type, providerId);
    if (apiKey) {
      apiKeys[rowId] = apiKey;
    }
  }

  return apiKeys;
}

export function parseAiProviders(
  entries: Partial<Record<string, AiProviderEntry>>,
  type: AiProviderType,
): Record<string, AiProviderConfig> {
  const result: Record<string, AiProviderConfig> = {};
  const prefix = `${type}:`;

  for (const [rowId, entry] of Object.entries(entries)) {
    if (!rowId.startsWith(prefix) || rowId.length === prefix.length) continue;
    if (!entry || entry.type !== type) continue;
    result[rowId] = {
      type,
      base_url: typeof entry.base_url === "string" ? entry.base_url : "",
      api_key: "",
    };
  }

  return result;
}

async function getProviderApiKey(
  type: AiProviderType,
  providerId: string,
): Promise<string | null> {
  const result = await store2Commands.getSecret(
    PROVIDER_SECRET_SCOPE,
    providerRowId(type, providerId),
  );
  if (result.status === "error") {
    throw new Error(result.error);
  }
  return result.data;
}

async function setProviderApiKey(
  type: AiProviderType,
  providerId: string,
  apiKey: string,
): Promise<void> {
  const key = providerRowId(type, providerId);
  const result = apiKey
    ? await store2Commands.setSecret(PROVIDER_SECRET_SCOPE, key, apiKey)
    : await store2Commands.deleteSecret(PROVIDER_SECRET_SCOPE, key);
  if (result.status === "error") {
    throw new Error(result.error);
  }
}

function providerRowId(type: AiProviderType, providerId: string): string {
  return `${type}:${providerId}`;
}
