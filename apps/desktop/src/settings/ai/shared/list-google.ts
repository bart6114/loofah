import { fetch as tauriFetch } from "@tauri-apps/plugin-http";

import { listGoogleModels as sharedlistGoogleModels } from "@hypr/ai-providers/models/list-google";
export const listGoogleModels = (baseUrl: string, apiKey: string) =>
  sharedlistGoogleModels(baseUrl, apiKey, tauriFetch);
