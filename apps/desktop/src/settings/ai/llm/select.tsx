import { Trans, useLingui } from "@lingui/react/macro";
import { useForm } from "@tanstack/react-form";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Check, Loader2 } from "lucide-react";
import { useMemo } from "react";

import { Button } from "@hypr/ui/components/ui/button";
import { cn } from "@hypr/utils";

import { useLlmSettings } from "./context";
import { useConnectionHealth } from "./health";
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
import { useAiProvidersState } from "~/settings/providers";
import { setSettingValues, useSettingsReady } from "~/settings/queries";
import { useConfigValues } from "~/shared/config";

export function SelectProviderAndModel({
  showAlerts = true,
}: {
  showAlerts?: boolean;
} = {}) {
  const { providers, isReady } = useConfiguredMapping();
  const settingsReady = useSettingsReady();
  const queryClient = useQueryClient();
  const { setEditingConnection, setConnectionProvider, setAccordionValue } =
    useLlmSettings();
  const { current_llm_provider, current_llm_model } = useConfigValues([
    "current_llm_provider",
    "current_llm_model",
  ] as const);
  const health = useConnectionHealth();
  const provider = PROVIDERS.find(({ id }) => id === current_llm_provider);
  const hasSelection = !!current_llm_provider;
  const selection = useMutation({
    mutationFn: setSettingValues,
  });
  const checkConnection = useMutation({
    mutationFn: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["llm-health-check"] }),
        queryClient.invalidateQueries({ queryKey: ["chatgpt-account"] }),
        queryClient.invalidateQueries({
          queryKey: ["models", current_llm_provider],
        }),
      ]);
    },
  });
  const connectionChecking =
    checkConnection.isPending || health.status === "pending";
  const editConnection = () => {
    setConnectionProvider(current_llm_provider ?? "");
    setAccordionValue(current_llm_provider ?? "");
    setEditingConnection(true);
  };

  if (!settingsReady || !isReady) {
    return (
      <p role="status" className="text-muted-foreground text-sm">
        <Trans>Loading connection…</Trans>
      </p>
    );
  }

  return (
    <section className="flex flex-col gap-4 rounded-xl border p-4">
      <div className="flex items-start justify-between gap-4">
        <div className="flex flex-col gap-1">
          <h3 className="text-sm font-semibold">
            {hasSelection ? (
              <Trans>Current connection</Trans>
            ) : (
              <Trans>Summaries are off</Trans>
            )}
          </h3>
          {!hasSelection && (
            <p className="text-muted-foreground text-sm">
              <Trans>
                Recording and transcription still work. Connect an AI service or
                a local AI app when you want summaries.
              </Trans>
            </p>
          )}
          {hasSelection && (
            <div className="flex items-center gap-2 text-sm">
              {provider && <ProviderIconSlot>{provider.icon}</ProviderIconSlot>}
              <span>{provider?.displayName ?? current_llm_provider}</span>
              <span className="text-muted-foreground text-xs">
                <Trans>Selected</Trans>
              </span>
            </div>
          )}
        </div>
        <Button variant="outline" size="sm" onClick={editConnection}>
          {hasSelection ? (
            <Trans>Change connection</Trans>
          ) : (
            <Trans>Set up summaries</Trans>
          )}
        </Button>
      </div>
      {hasSelection && (
        <>
          <p className="text-muted-foreground text-xs">
            {current_llm_provider === "ollama" ||
            current_llm_provider === "lmstudio" ? (
              <Trans>
                Uses your AI app's configured server. Choose an on-device model
                in that app to keep summaries local.
              </Trans>
            ) : current_llm_provider === "custom" ? (
              <Trans>
                Text for summaries is sent to your configured server.
              </Trans>
            ) : (
              <Trans>Text for summaries is sent to this provider.</Trans>
            )}
          </p>
          <div className="flex flex-col gap-2">
            <span className="text-muted-foreground text-xs">
              <Trans>Model</Trans>
            </span>
            <ModelCombobox
              providerId={current_llm_provider}
              value={current_llm_model ?? ""}
              onChange={(model) =>
                selection.mutate({ current_llm_model: model })
              }
              disabled={selection.isPending}
              listModels={providers[current_llm_provider]?.listModels}
            />
          </div>
          <div
            role="status"
            className={cn([
              "flex items-center gap-2 text-sm",
              health.status === "error" && !connectionChecking
                ? "text-destructive"
                : "text-muted-foreground",
            ])}
          >
            {health.status === "success" && !connectionChecking ? (
              <>
                <Check className="text-brand size-4" />
                <Trans>Ready for summaries</Trans>
              </>
            ) : connectionChecking ? (
              <>
                <Loader2 className="size-4 animate-spin" />
                <Trans>Checking connection…</Trans>
              </>
            ) : health.status === "error" ? (
              <Trans>Connection unavailable</Trans>
            ) : (
              <Trans>Choose a model to finish setup</Trans>
            )}
          </div>
          {showAlerts && !connectionChecking && health.status === "error" && (
            <div className="flex flex-col gap-2">
              <p className="text-muted-foreground text-sm">
                <Trans>
                  Check this connection's sign-in, API key, or local AI app,
                  then try again.
                </Trans>
              </p>
              {health.message && (
                <details className="text-muted-foreground text-xs">
                  <summary className="cursor-pointer">
                    <Trans>Connection details</Trans>
                  </summary>
                  <p className="mt-2 break-words">{health.message}</p>
                </details>
              )}
              <Button
                variant="outline"
                size="sm"
                className="self-start"
                disabled={checkConnection.isPending}
                onClick={() => checkConnection.mutate()}
              >
                <Trans>Check connection</Trans>
              </Button>
            </div>
          )}
          <Button
            variant="ghost"
            size="sm"
            className="text-muted-foreground self-start"
            disabled={selection.isPending}
            onClick={() =>
              selection.mutate({
                current_llm_provider: "",
                current_llm_model: "",
              })
            }
          >
            <Trans>Turn off summaries</Trans>
          </Button>
        </>
      )}
      {selection.error && (
        <p role="alert" className="text-destructive text-sm">
          {selection.error.message}
        </p>
      )}
    </section>
  );
}

export function SetupModelSelection({ providerId }: { providerId: string }) {
  const { t } = useLingui();
  const { providers } = useConfiguredMapping();
  const { setEditingConnection } = useLlmSettings();
  const { current_llm_provider, current_llm_model } = useConfigValues([
    "current_llm_provider",
    "current_llm_model",
  ] as const);
  const selection = useMutation({
    mutationFn: (model: string) =>
      setSettingValues({
        current_llm_provider: providerId,
        current_llm_model: model,
      }),
    onSuccess: () => setEditingConnection(false),
  });
  const form = useForm({
    defaultValues: {
      model:
        current_llm_provider === providerId ? (current_llm_model ?? "") : "",
    },
    onSubmit: ({ value }) => selection.mutate(value.model),
  });
  const status = providers[providerId];
  if (!status?.configured) return null;
  return (
    <form
      className="flex flex-col gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        event.stopPropagation();
        void form.handleSubmit();
      }}
    >
      <form.Field name="model">
        {(field) => (
          <>
            <span className="text-sm font-medium">
              <Trans>Choose a model</Trans>
            </span>
            <ModelCombobox
              providerId={providerId}
              value={field.state.value}
              onChange={field.handleChange}
              listModels={status.listModels}
              placeholder={t`Select a model`}
            />
            <Button
              type="submit"
              className="self-start"
              disabled={!field.state.value || selection.isPending}
            >
              <Trans>Use this connection</Trans>
            </Button>
          </>
        )}
      </form.Field>
      {selection.error && (
        <p role="alert" className="text-destructive text-sm">
          {selection.error.message}
        </p>
      )}
    </form>
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
