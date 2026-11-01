import { fetch as tauriFetch } from "@tauri-apps/plugin-http";

import { listAnthropicModels as sharedlistAnthropicModels } from "@hypr/ai-providers/models/list-anthropic";
export const listAnthropicModels = (baseUrl: string, apiKey: string) =>
  sharedlistAnthropicModels(baseUrl, apiKey, tauriFetch);
