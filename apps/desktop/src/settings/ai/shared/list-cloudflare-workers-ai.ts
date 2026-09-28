import { fetch as tauriFetch } from "@tauri-apps/plugin-http";
export { CLOUDFLARE_WORKERS_AI_MODELS } from "@hypr/ai-providers/models/list-cloudflare-workers-ai";
export { getCloudflareWorkersAIModelMetadata } from "@hypr/ai-providers/models/list-cloudflare-workers-ai";
export { createStaticCloudflareWorkersAIModelResult } from "@hypr/ai-providers/models/list-cloudflare-workers-ai";
import { listCloudflareWorkersAIModels as sharedlistCloudflareWorkersAIModels } from "@hypr/ai-providers/models/list-cloudflare-workers-ai";
export const listCloudflareWorkersAIModels = (
  baseUrl: string,
  apiKey: string,
) => sharedlistCloudflareWorkersAIModels(baseUrl, apiKey, tauriFetch);
