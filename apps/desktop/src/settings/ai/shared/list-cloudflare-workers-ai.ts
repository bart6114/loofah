import imagePolicy from "../../../../../../crates/loofah-core/assets/local-image-capabilities.json";
import type { ListModelsResult, ModelMetadata } from "./list-common";
import { DEFAULT_RESULT } from "./list-common";

export const CLOUDFLARE_WORKERS_AI_MODELS =
  imagePolicy.cloudflare_workers_ai_models.map(({ model }) => model);

export function getCloudflareWorkersAIModelMetadata(
  model: string | undefined,
): ModelMetadata | undefined {
  const metadata = imagePolicy.cloudflare_workers_ai_models.find(
    (entry) => entry.model === model,
  );
  if (!metadata) return undefined;
  return {
    input_modalities: metadata.input_modalities.filter(
      (modality): modality is "text" | "image" =>
        modality === "text" || modality === "image",
    ),
  };
}

export function createStaticCloudflareWorkersAIModelResult(): ListModelsResult {
  const metadata: Record<string, ModelMetadata> = {};

  for (const model of CLOUDFLARE_WORKERS_AI_MODELS) {
    metadata[model] = getCloudflareWorkersAIModelMetadata(model)!;
  }

  return {
    models: [...CLOUDFLARE_WORKERS_AI_MODELS],
    ignored: [],
    metadata,
  };
}

export async function listCloudflareWorkersAIModels(
  baseUrl: string,
  _apiKey: string,
): Promise<ListModelsResult> {
  if (!baseUrl) {
    return DEFAULT_RESULT;
  }

  return createStaticCloudflareWorkersAIModelResult();
}
