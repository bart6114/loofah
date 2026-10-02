import { fetch as tauriFetch } from "@tauri-apps/plugin-http";

import { listLMStudioModels as sharedlistLMStudioModels } from "@hypr/ai-providers/models/list-lmstudio";
export const listLMStudioModels = (baseUrl: string, apiKey: string) =>
  sharedlistLMStudioModels(baseUrl, apiKey, tauriFetch);
export { getLMStudioNativeModelsUrl } from "@hypr/ai-providers/models/list-lmstudio";
export { processLMStudioModels } from "@hypr/ai-providers/models/list-lmstudio";
