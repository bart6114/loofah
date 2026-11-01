import { fetch as tauriFetch } from "@tauri-apps/plugin-http";

import { listOllamaModels as sharedlistOllamaModels } from "@hypr/ai-providers/models/list-ollama";
export const listOllamaModels = (baseUrl: string, apiKey: string) =>
  sharedlistOllamaModels(baseUrl, apiKey, tauriFetch);
