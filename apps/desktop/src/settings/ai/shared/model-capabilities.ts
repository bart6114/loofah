import imagePolicy from "../../../../../../crates/loofah-core/assets/local-image-capabilities.json";
import { getCloudflareWorkersAIModelMetadata } from "./list-cloudflare-workers-ai";

import { chatgptModelSupportsImages } from "~/ai/chatgpt-account";

const TEXT_ONLY_MODEL_RE = new RegExp(imagePolicy.text_only_model_pattern, "i");

const IMAGE_INPUT_MODEL_RE = new RegExp(
  imagePolicy.image_input_model_pattern,
  "i",
);

export function modelSupportsImageInput(
  providerId: string | undefined,
  modelId: string | undefined,
): boolean {
  if (!providerId || !modelId) {
    return false;
  }

  if (providerId === "chatgpt_subscription")
    return chatgptModelSupportsImages(modelId);

  if (providerId === "cloudflare_workers_ai") {
    const metadata = getCloudflareWorkersAIModelMetadata(modelId);
    if (metadata?.input_modalities) {
      return metadata.input_modalities.includes("image");
    }
  }

  if (TEXT_ONLY_MODEL_RE.test(modelId)) {
    return false;
  }

  if (IMAGE_INPUT_MODEL_RE.test(modelId)) {
    return true;
  }

  return false;
}
