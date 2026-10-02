import { Icon } from "@iconify-icon/react";
import {
  Anthropic,
  Azure,
  AzureAI,
  LmStudio,
  Mistral,
  Ollama,
  OpenAI,
  OpenRouter,
} from "@lobehub/icons";
import type { ReactNode } from "react";

import {
  PROVIDERS as SHARED_PROVIDERS,
  type ProviderDefinition,
  type ProviderId,
} from "@hypr/ai-providers";
export type { ProviderId } from "@hypr/ai-providers";
import {
  checkLMStudioAvailability,
  checkOllamaAvailability,
} from "~/settings/ai/shared/local-provider-availability";
import { sortProviders } from "~/settings/ai/shared/sort-providers";

export type Provider = ProviderDefinition & {
  id: ProviderId;
  icon: ReactNode;
  checkAvailability?: (baseUrl: string, apiKey: string) => Promise<boolean>;
};

const icons: Record<ProviderId, ReactNode> = {
  lmstudio: <LmStudio size={16} />,
  ollama: <Ollama size={16} />,
  openrouter: <OpenRouter size={16} />,
  openai: <OpenAI size={16} />,
  chatgpt_subscription: <OpenAI size={16} />,
  cloudflare_workers_ai: <Icon icon="simple-icons:cloudflare" width={16} />,
  anthropic: <Anthropic size={16} />,
  mistral: <Mistral size={16} />,
  azure_openai: <Azure size={14} style={{ height: 14, width: 14 }} />,
  azure_ai: <AzureAI size={14} style={{ height: 14, width: 14 }} />,
  google_generative_ai: <Icon icon="simple-icons:googlegemini" width={16} />,
  custom: <Icon icon="mingcute:random-fill" />,
};

export const _PROVIDERS: readonly Provider[] = SHARED_PROVIDERS.map(
  (provider) => ({
    ...provider,
    icon: icons[provider.id],
    checkAvailability:
      provider.id === "lmstudio"
        ? checkLMStudioAvailability
        : provider.id === "ollama"
          ? checkOllamaAvailability
          : undefined,
  }),
);
export const PROVIDERS = sortProviders(_PROVIDERS);
