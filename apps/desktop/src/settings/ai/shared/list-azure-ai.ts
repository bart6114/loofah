import { fetch as tauriFetch } from "@tauri-apps/plugin-http";

import { listAzureAIModels as sharedlistAzureAIModels } from "@hypr/ai-providers/models/list-azure-ai";
export const listAzureAIModels = (baseUrl: string, apiKey: string) =>
  sharedlistAzureAIModels(baseUrl, apiKey, tauriFetch);
