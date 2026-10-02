import { listAnthropicModels } from "./models/list-anthropic";
import { listAzureAIModels } from "./models/list-azure-ai";
import { listAzureOpenAIModels } from "./models/list-azure-openai";
import { listCloudflareWorkersAIModels } from "./models/list-cloudflare-workers-ai";
import { DEFAULT_RESULT, type ListModelsResult } from "./models/list-common";
import { listGoogleModels } from "./models/list-google";
import { listLMStudioModels } from "./models/list-lmstudio";
import { listMistralModels } from "./models/list-mistral";
import { listOllamaModels } from "./models/list-ollama";
import { listOpenAIModels } from "./models/list-openai";
import { listGenericModels } from "./models/list-openai";
import { listOpenRouterModels } from "./models/list-openrouter";
import type { ProviderId } from "./providers";
export * from "./providers";
export * from "./model";
export type {
  ListModelsResult,
  ModelMetadata,
  IgnoredModel,
  InputModality,
} from "./models/list-common";
export const SUMMARY_MAX_OUTPUT_TOKENS = 8192;
export const AI_GENERATION_MAX_RETRIES = 4;

export async function listModels({
  providerId,
  baseUrl,
  apiKey,
  fetch: fetcher,
}: {
  providerId: ProviderId;
  baseUrl: string;
  apiKey: string;
  fetch: typeof fetch;
}): Promise<ListModelsResult> {
  switch (providerId) {
    case "anthropic":
      return listAnthropicModels(baseUrl, apiKey, fetcher);
    case "azure_ai":
      return listAzureAIModels(baseUrl, apiKey, fetcher);
    case "azure_openai":
      return listAzureOpenAIModels(baseUrl, apiKey, fetcher);
    case "cloudflare_workers_ai":
      return listCloudflareWorkersAIModels(baseUrl, apiKey, fetcher);
    case "google_generative_ai":
      return listGoogleModels(baseUrl, apiKey, fetcher);
    case "lmstudio":
      return listLMStudioModels(baseUrl, apiKey, fetcher);
    case "mistral":
      return listMistralModels(baseUrl, apiKey, fetcher);
    case "ollama":
      return listOllamaModels(baseUrl, apiKey, fetcher);
    case "openai":
      return listOpenAIModels(baseUrl, apiKey, fetcher);
    case "openrouter":
      return listOpenRouterModels(baseUrl, apiKey, fetcher);
    case "custom":
      return listGenericModels(baseUrl, apiKey, fetcher);
    case "chatgpt_subscription":
      return DEFAULT_RESULT;
  }
}

export { resolveLLMConnection, type ConnectionStatus } from "./connection";
