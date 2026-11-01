import { fetch as tauriFetch } from "@tauri-apps/plugin-http";

import { listAzureOpenAIModels as sharedlistAzureOpenAIModels } from "@hypr/ai-providers/models/list-azure-openai";
export const listAzureOpenAIModels = (baseUrl: string, apiKey: string) =>
  sharedlistAzureOpenAIModels(baseUrl, apiKey, tauriFetch);
