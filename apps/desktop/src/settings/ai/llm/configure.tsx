import { Trans, useLingui } from "@lingui/react/macro";

import { Accordion } from "@hypr/ui/components/ui/accordion";
import { Button } from "@hypr/ui/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@hypr/ui/components/ui/select";
import { cn } from "@hypr/utils";

import { ChatgptSettings } from "./chatgpt";
import { useLlmSettings } from "./context";
import { SetupModelSelection } from "./select";
import { type ProviderId, PROVIDERS } from "./shared";

import { NonHyprProviderCard, StyledStreamdown } from "~/settings/ai/shared";
import { useConfigValues } from "~/shared/config";

export function getConnectionRoute(provider: string) {
  if (!provider) return null;
  if (provider === "chatgpt_subscription") return "chatgpt";
  if (provider === "ollama" || provider === "lmstudio") return "local";
  return "api";
}

export function ConfigureProviders() {
  const { t } = useLingui();
  const {
    accordionValue,
    setAccordionValue,
    editingConnection,
    setEditingConnection,
    connectionProvider,
    setConnectionProvider,
  } = useLlmSettings();
  const { current_llm_provider, current_llm_model } = useConfigValues([
    "current_llm_provider",
    "current_llm_model",
  ] as const);
  if (!editingConnection) return null;
  const route = getConnectionRoute(connectionProvider);
  const provider = PROVIDERS.find(({ id }) => id === connectionProvider);
  const routeProviders = PROVIDERS.filter(
    ({ id }) => getConnectionRoute(id) === route,
  );
  const chooseProvider = (id: string) => {
    setConnectionProvider(id);
    setAccordionValue(id);
  };
  return (
    <section className="flex flex-col gap-4 rounded-xl border p-4">
      <div className="flex items-center justify-between gap-4">
        <h3 className="text-sm font-semibold">
          <Trans>Choose a connection</Trans>
        </h3>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => setEditingConnection(false)}
        >
          <Trans>Close setup</Trans>
        </Button>
      </div>
      <div className="grid grid-cols-1 gap-2 sm:grid-cols-3">
        {[
          {
            id: "chatgpt",
            provider: "chatgpt_subscription",
            label: t`ChatGPT subscription`,
            description: t`Sign in · Beta`,
          },
          {
            id: "api",
            provider: "openai",
            label: t`API key`,
            description: t`Connect an AI provider`,
          },
          {
            id: "local",
            provider: "lmstudio",
            label: t`Local AI app`,
            description: t`LM Studio or Ollama`,
          },
        ].map((option) => (
          <button
            key={option.id}
            type="button"
            aria-pressed={route === option.id}
            onClick={() => {
              if (route !== option.id) chooseProvider(option.provider);
            }}
            className={cn([
              "flex flex-col gap-1 rounded-lg border p-3 text-left text-sm transition-colors",
              route === option.id
                ? "border-brand bg-accent"
                : "hover:bg-accent",
            ])}
          >
            <span className="font-medium">{option.label}</span>
            <span className="text-muted-foreground text-xs">
              {option.description}
            </span>
          </button>
        ))}
      </div>
      {route === "local" && (
        <p className="text-muted-foreground text-sm">
          <Trans>
            Requires a separate app. Install LM Studio or Ollama, download a
            model there, and keep its local server running while using
            summaries.
          </Trans>
        </p>
      )}
      {route === "api" && (
        <p className="text-muted-foreground text-sm">
          <Trans>
            The selected provider processes the text you send for summaries. API
            usage may have separate charges.
          </Trans>
        </p>
      )}
      {route && route !== "chatgpt" && (
        <Select value={connectionProvider} onValueChange={chooseProvider}>
          <SelectTrigger
            aria-label={route === "local" ? t`Local AI app` : t`AI provider`}
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {routeProviders.map((item) => (
              <SelectItem key={item.id} value={item.id}>
                {item.displayName}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
      {provider && (
        <>
          <Accordion
            type="single"
            collapsible
            value={accordionValue}
            onValueChange={setAccordionValue}
          >
            {provider.id === "chatgpt_subscription" ? (
              <ChatgptSettings activateOnLogin={false} />
            ) : (
              <NonHyprProviderCard
                config={provider}
                providerType="llm"
                isActive={
                  current_llm_provider === provider.id && !!current_llm_model
                }
                providers={PROVIDERS}
                providerContext={<ProviderContext providerId={provider.id} />}
              />
            )}
          </Accordion>
          <SetupModelSelection key={provider.id} providerId={provider.id} />
        </>
      )}
    </section>
  );
}

function ProviderContext({ providerId }: { providerId: ProviderId }) {
  const content =
    providerId === "lmstudio"
      ? "- Ensure LM Studio server is **running.** (Default port is 1234)\n- Enable **CORS** in LM Studio config."
      : providerId === "ollama"
        ? "- Ensure Ollama is **running** (`ollama serve`)\n- Pull a model first (`ollama pull llama3.2`)"
        : providerId === "custom"
          ? "We only support **OpenAI-compatible** endpoints for now."
          : providerId === "openrouter"
            ? "We filter out models from the combobox based on heuristics like **input modalities** and **tool support**."
            : providerId === "azure_openai"
              ? "Enter your **Azure OpenAI endpoint** (e.g. `https://your-resource.openai.azure.com`) as the Base URL and your **API key**. [Report issues](https://github.com/fastrepl/char/issues/3928)"
              : providerId === "azure_ai"
                ? "Enter your **Azure AI Foundry endpoint** as the Base URL and your **API key**. Supports Claude and other models deployed via Azure AI Foundry. [Report issues](https://github.com/fastrepl/char/issues/3928)"
                : providerId === "google_generative_ai"
                  ? "Visit [AI Studio](https://aistudio.google.com/api-keys) to create an API key."
                  : providerId === "cloudflare_workers_ai"
                    ? "Enter the Workers AI **OpenAI-compatible base URL** as `https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/v1` and use a Cloudflare API token with Workers AI access."
                    : "";

  if (!content) {
    return null;
  }

  return <StyledStreamdown className="mb-3">{content}</StyledStreamdown>;
}
