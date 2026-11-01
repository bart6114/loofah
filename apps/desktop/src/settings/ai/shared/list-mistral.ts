import { fetch as tauriFetch } from "@tauri-apps/plugin-http";

import { listMistralModels as sharedlistMistralModels } from "@hypr/ai-providers/models/list-mistral";
export const listMistralModels = (baseUrl: string, apiKey: string) =>
  sharedlistMistralModels(baseUrl, apiKey, tauriFetch);
