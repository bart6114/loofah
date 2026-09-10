import { Trans, useLingui } from "@lingui/react/macro";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useRef, useState } from "react";

import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@hypr/ui/components/ui/select";
import { cn } from "@hypr/utils";

import { useLlmSettings } from "./context";
import { HealthStatusIndicator, useConnectionHealth } from "./health";
import {
  getDefaultLlmSelection,
  getPreferredProviderModel,
  isSameModelSelection,
  shouldShowMissingModelWarning,
} from "./selection";
import { type Provider, PROVIDERS } from "./shared";

import {
  CHATGPT_PROVIDER,
  listChatgptModels,
  useChatgptAccount,
} from "~/ai/chatgpt-account";
import { providerRowId, ProviderIconSlot } from "~/settings/ai/shared";
import { getProviderSelectionBlockers } from "~/settings/ai/shared/eligibility";
import { listAnthropicModels } from "~/settings/ai/shared/list-anthropic";
import { listAzureAIModels } from "~/settings/ai/shared/list-azure-ai";
import { listAzureOpenAIModels } from "~/settings/ai/shared/list-azure-openai";
import { listCloudflareWorkersAIModels } from "~/settings/ai/shared/list-cloudflare-workers-ai";
import { type ListModelsResult } from "~/settings/ai/shared/list-common";
import { listGoogleModels } from "~/settings/ai/shared/list-google";
import { listLMStudioModels } from "~/settings/ai/shared/list-lmstudio";
import { listMistralModels } from "~/settings/ai/shared/list-mistral";
import { listOllamaModels } from "~/settings/ai/shared/list-ollama";
import {
  listGenericModels,
  listOpenAIModels,
} from "~/settings/ai/shared/list-openai";
import { listOpenRouterModels } from "~/settings/ai/shared/list-openrouter";
import { ModelCombobox } from "~/settings/ai/shared/model-combobox";
import { PersistAiSelection } from "~/settings/ai/shared/persist-selection";
import {
  getConfiguredProviderIds,
  getConfiguredProviders,
  getVisibleModelSelection,
} from "~/settings/ai/shared/selection";
import { useAiProvidersState } from "~/settings/providers";
import { setSettingValues, useSettingsReady } from "~/settings/queries";
import { useConfigValues } from "~/shared/config";
import { SettingsAlertToast } from "~/shared/ui/settings-alert";

export function SelectProviderAndModel({
  showAlerts = true,
}: {
  showAlerts?: boolean;
} = {}) {
  const { t } = useLingui();
  const { providers: configuredProviders, isReady: providerSettingsReady } =
    useConfiguredMapping();
  const settingsReady = useSettingsReady();
  const queryClient = useQueryClient();
  const { setAccordionValue } = useLlmSettings();
  const [pendingSelection, setPendingSelection] = useState<{
    provider: string;
    model: string;
    originProvider: string | undefined;
    originModel: string | undefined;
  } | null>(null);
  const [isResolvingProvider, setIsResolvingProvider] = useState(false);

  const { current_llm_model, current_llm_provider } = useConfigValues([
    "current_llm_model",
    "current_llm_provider",
  ] as const);
  const selectedProviderConfigured = current_llm_provider
    ? (configuredProviders[current_llm_provider]?.configured ?? false)
    : false;
  const visibleSelection = getVisibleModelSelection(
    current_llm_provider,
    current_llm_model,
    selectedProviderConfigured || current_llm_provider === CHATGPT_PROVIDER,
  );
  const providerOptions = getConfiguredProviders(
    PROVIDERS,
    configuredProviders,
  );
  if (
    current_llm_provider === CHATGPT_PROVIDER &&
    !providerOptions.some((p) => p.id === CHATGPT_PROVIDER)
  ) {
    providerOptions.push(PROVIDERS.find((p) => p.id === CHATGPT_PROVIDER)!);
  }
  const configuredProviderIds = getConfiguredProviderIds(
    PROVIDERS,
    configuredProviders,
    current_llm_provider,
  );
  const pendingSelectionSettled =
    pendingSelection &&
    isSameModelSelection(
      current_llm_provider,
      current_llm_model,
      pendingSelection.provider,
      pendingSelection.model,
    );
  if (pendingSelectionSettled) {
    setPendingSelection(null);
  }
  const activePendingSelection =
    pendingSelection &&
    !pendingSelectionSettled &&
    isSameModelSelection(
      current_llm_provider,
      current_llm_model,
      pendingSelection.originProvider,
      pendingSelection.originModel,
    )
      ? pendingSelection
      : null;

  const lastSelectedModelsRef = useRef<Record<string, string>>(
    current_llm_provider && current_llm_model
      ? { [current_llm_provider]: current_llm_model }
      : {},
  );
  const selectionRequestRef = useRef(0);

  const persistSelection = (
    provider: string,
    model: string,
    requestId: number,
  ) => {
    void setSettingValues({
      current_llm_provider: provider,
      current_llm_model: model,
    }).catch((error) => {
      console.error("[settings] failed to update LLM selection", error);
      if (selectionRequestRef.current === requestId) {
        setPendingSelection(null);
      }
    });
  };

  const rememberModel = (provider?: string, model?: string) => {
    if (!provider || model === undefined) {
      return;
    }

    lastSelectedModelsRef.current[provider] = model;
  };

  const getCachedModels = (provider: string) => {
    const status = configuredProviders[provider];
    if (!status?.listModels) {
      return [];
    }

    return (
      queryClient.getQueryData<ListModelsResult>([
        "models",
        provider,
        status.listModels,
      ])?.models ?? []
    );
  };

  const fetchModels = async (provider: string) => {
    const status = configuredProviders[provider];
    const listModels = status?.listModels;
    if (!listModels) {
      return [];
    }

    const result = await queryClient.fetchQuery({
      queryKey: ["models", provider, listModels],
      queryFn: async () => await listModels(),
      retry: 3,
      retryDelay: 300,
      staleTime: 1000 * 2,
    });

    return result.models;
  };

  const needsDefaultSelection =
    current_llm_provider !== CHATGPT_PROVIDER &&
    !(visibleSelection.provider && visibleSelection.model);
  const defaultSelectionQuery = useQuery({
    queryKey: [
      "default-ai-selection",
      "llm",
      current_llm_provider ?? "",
      current_llm_model ?? "",
      configuredProviderIds,
      fetchModels,
    ],
    queryFn: async () =>
      await getDefaultLlmSelection(
        configuredProviderIds,
        current_llm_provider,
        current_llm_model,
        fetchModels,
      ),
    enabled:
      !activePendingSelection &&
      providerSettingsReady &&
      needsDefaultSelection &&
      configuredProviderIds.length > 0,
    retry: false,
    staleTime: Infinity,
  });
  const defaultSelection = needsDefaultSelection
    ? defaultSelectionQuery.data
    : null;
  const effectiveSelection = activePendingSelection
    ? {
        provider: activePendingSelection.provider,
        model: activePendingSelection.model,
      }
    : (defaultSelection ?? visibleSelection);

  const health = useConnectionHealth();
  const isConfigured = !!(
    effectiveSelection.provider && effectiveSelection.model
  );
  const hasError =
    isConfigured && !activePendingSelection && health.status === "error";
  const isResolvingSelection =
    isResolvingProvider || defaultSelectionQuery.isFetching;
  const showMissingModelWarning = shouldShowMissingModelWarning({
    isConfigured,
    isResolvingSelection,
    providerSettingsReady,
    settingsReady,
  });
  const alertDescription = showMissingModelWarning
    ? t`Language model is needed to make Loofah summarize and chat about your conversations.`
    : providerSettingsReady &&
        settingsReady &&
        !isResolvingSelection &&
        hasError
      ? health.message
      : undefined;

  const handleProviderChange = (provider: string) => {
    const requestId = ++selectionRequestRef.current;

    const status = configuredProviders[provider];
    if (!status?.listModels) {
      setAccordionValue(provider);
    }

    rememberModel(current_llm_provider, current_llm_model);
    const originSelection = {
      originProvider: current_llm_provider,
      originModel: current_llm_model,
    };
    setPendingSelection({ provider, model: "", ...originSelection });
    setIsResolvingProvider(false);

    const nextModel = getPreferredProviderModel(
      lastSelectedModelsRef.current[provider],
      getCachedModels(provider),
      { allowSavedModelWithoutChoices: provider === "custom" },
    );

    if (nextModel) {
      setPendingSelection({ provider, model: nextModel, ...originSelection });
      rememberModel(provider, nextModel);
      persistSelection(provider, nextModel, requestId);
      return;
    }

    setIsResolvingProvider(true);
    void (async () => {
      let models: string[];
      try {
        models = await fetchModels(provider);
      } catch {
        if (selectionRequestRef.current === requestId) {
          setIsResolvingProvider(false);
          if (provider !== "custom") {
            setPendingSelection(null);
          }
        }
        return;
      }
      const resolvedModel = getPreferredProviderModel(
        lastSelectedModelsRef.current[provider],
        models,
        { allowSavedModelWithoutChoices: provider === "custom" },
      );

      if (selectionRequestRef.current !== requestId) {
        return;
      }

      setIsResolvingProvider(false);
      if (!resolvedModel) {
        if (provider !== "custom") {
          setPendingSelection(null);
        }
        return;
      }

      setPendingSelection({
        provider,
        model: resolvedModel,
        ...originSelection,
      });
      rememberModel(provider, resolvedModel);
      persistSelection(provider, resolvedModel, requestId);
    })();
  };

  const handleModelChange = (model: string) => {
    if (!effectiveSelection.provider) {
      return;
    }

    const requestId = ++selectionRequestRef.current;
    rememberModel(effectiveSelection.provider, model);
    setPendingSelection({
      provider: effectiveSelection.provider,
      model,
      originProvider: current_llm_provider,
      originModel: current_llm_model,
    });
    setIsResolvingProvider(false);
    persistSelection(effectiveSelection.provider, model, requestId);
  };

  return (
    <div className="flex flex-col gap-4">
      {defaultSelection && !activePendingSelection ? (
        <PersistAiSelection
          key={`llm:${defaultSelection.provider}:${defaultSelection.model}`}
          type="llm"
          provider={defaultSelection.provider}
          model={defaultSelection.model}
        />
      ) : null}
      {showAlerts && (
        <SettingsAlertToast
          id="llm-settings-alert"
          description={alertDescription}
          variant={hasError ? "error" : "warning"}
        />
      )}

      <h3 className="text-md font-sans font-semibold">
        <Trans>Model being used</Trans>
      </h3>
      <div className="flex flex-row items-center gap-4">
        <div className="min-w-0 flex-2" data-llm-provider-selector>
          <Select
            value={effectiveSelection.provider}
            onValueChange={handleProviderChange}
          >
            <SelectTrigger className="bg-card shadow-none focus:ring-0">
              <SelectValue placeholder={t`Select a provider`} />
            </SelectTrigger>
            <SelectContent>
              {providerOptions.map((provider) => {
                const configured =
                  configuredProviders[provider.id]?.configured ?? false;

                return (
                  <SelectItem
                    key={provider.id}
                    value={provider.id}
                    disabled={!configured}
                    className={cn([
                      "data-disabled:text-muted-foreground data-disabled:!opacity-100",
                      !configured && "text-muted-foreground",
                    ])}
                  >
                    <div className="flex items-center gap-2">
                      <ProviderIconSlot>{provider.icon}</ProviderIconSlot>
                      <span>{provider.displayName}</span>
                    </div>
                  </SelectItem>
                );
              })}
            </SelectContent>
          </Select>
        </div>

        <span className="text-muted-foreground">/</span>

        <div className="min-w-0 flex-3">
          <ModelCombobox
            providerId={effectiveSelection.provider}
            value={effectiveSelection.model}
            onChange={handleModelChange}
            disabled={!effectiveSelection.provider}
            listModels={
              effectiveSelection.provider
                ? configuredProviders[effectiveSelection.provider]?.listModels
                : undefined
            }
            isConfigured={isConfigured && health.status === "success"}
            suffix={isConfigured ? <HealthStatusIndicator /> : undefined}
          />
        </div>
      </div>
    </div>
  );
}

type ProviderStatus = {
  configured: boolean;
  listModels?: () => Promise<ListModelsResult>;
};

type ProviderConfig = {
  base_url?: unknown;
  api_key?: unknown;
};

export function getLlmProviderStatus({
  provider,
  config,
  isAuthenticated,
  isPaid,
}: {
  provider: Provider;
  config?: ProviderConfig;
  isAuthenticated: boolean;
  isPaid: boolean;
}): ProviderStatus {
  if (provider.id === CHATGPT_PROVIDER)
    return isAuthenticated
      ? { configured: true, listModels: listChatgptModels }
      : { configured: false };
  const baseUrl = String(config?.base_url || provider.baseUrl || "").trim();
  const apiKey = String(config?.api_key || "").trim();

  const eligible =
    getProviderSelectionBlockers(provider.requirements, {
      isAuthenticated,
      isPaid,
      config: { base_url: baseUrl, api_key: apiKey },
    }).length === 0;

  if (!eligible) {
    return { configured: false };
  }

  let listModelsFunc: () => Promise<ListModelsResult>;

  switch (provider.id) {
    case "openai":
      listModelsFunc = () => listOpenAIModels(baseUrl, apiKey);
      break;
    case "cloudflare_workers_ai":
      listModelsFunc = () => listCloudflareWorkersAIModels(baseUrl, apiKey);
      break;
    case "anthropic":
      listModelsFunc = () => listAnthropicModels(baseUrl, apiKey);
      break;
    case "openrouter":
      listModelsFunc = () => listOpenRouterModels(baseUrl, apiKey);
      break;
    case "google_generative_ai":
      listModelsFunc = () => listGoogleModels(baseUrl, apiKey);
      break;
    case "mistral":
      listModelsFunc = () => listMistralModels(baseUrl, apiKey);
      break;
    case "azure_openai":
      listModelsFunc = () => listAzureOpenAIModels(baseUrl, apiKey);
      break;
    case "azure_ai":
      listModelsFunc = () => listAzureAIModels(baseUrl, apiKey);
      break;
    case "ollama":
      listModelsFunc = () => listOllamaModels(baseUrl, apiKey);
      break;
    case "lmstudio":
      listModelsFunc = () => listLMStudioModels(baseUrl, apiKey);
      break;
    case "custom":
      listModelsFunc = () => listGenericModels(baseUrl, apiKey);
      break;
    default:
      listModelsFunc = () => listGenericModels(baseUrl, apiKey);
  }

  return { configured: true, listModels: listModelsFunc };
}

function useConfiguredMapping(): {
  providers: Record<string, ProviderStatus>;
  isReady: boolean;
} {
  const { providers: configuredProviders, isReady } =
    useAiProvidersState("llm");

  const { current_llm_provider } = useConfigValues([
    "current_llm_provider",
  ] as const);
  const chatgptAccount = useChatgptAccount(
    current_llm_provider === CHATGPT_PROVIDER,
  );
  const mapping = useMemo(() => {
    return Object.fromEntries(
      PROVIDERS.map((provider) => {
        const config = configuredProviders[providerRowId("llm", provider.id)];
        return [
          provider.id,
          getLlmProviderStatus({
            provider,
            config,
            isAuthenticated:
              provider.id !== CHATGPT_PROVIDER || !!chatgptAccount.data,
            isPaid: true,
          }),
        ];
      }),
    ) as Record<string, ProviderStatus>;
  }, [configuredProviders, chatgptAccount.data]);

  return { providers: mapping, isReady };
}
