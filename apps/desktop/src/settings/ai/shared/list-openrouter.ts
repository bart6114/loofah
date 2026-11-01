import { fetch as tauriFetch } from "@tauri-apps/plugin-http";

import { listOpenRouterModels as sharedlistOpenRouterModels } from "@hypr/ai-providers/models/list-openrouter";
export const listOpenRouterModels = (baseUrl: string, apiKey: string) =>
  sharedlistOpenRouterModels(baseUrl, apiKey, tauriFetch);
export { processOpenRouterModels } from "@hypr/ai-providers/models/list-openrouter";
