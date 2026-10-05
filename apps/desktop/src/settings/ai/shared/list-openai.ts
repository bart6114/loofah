import { fetch as tauriFetch } from "@tauri-apps/plugin-http";

import { listOpenAIModels as sharedlistOpenAIModels } from "@hypr/ai-providers/models/list-openai";
export const listOpenAIModels = (baseUrl: string, apiKey: string) =>
  sharedlistOpenAIModels(baseUrl, apiKey, tauriFetch);
import { listGenericModels as sharedlistGenericModels } from "@hypr/ai-providers/models/list-openai";
export const listGenericModels = (baseUrl: string, apiKey: string) =>
  sharedlistGenericModels(baseUrl, apiKey, tauriFetch);
