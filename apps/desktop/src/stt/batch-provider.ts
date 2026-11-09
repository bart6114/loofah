import type { TranscriptionParams } from "@hypr/plugin-transcription";

import { isSupportedLocalSttModel } from "./capabilities";

export function getBatchProvider(
  provider: string,
  model: string,
): TranscriptionParams["provider"] | null {
  if (provider !== "fmtr" || !isSupportedLocalSttModel(model)) {
    return null;
  }

  if (model.startsWith("soniqo-") || model.startsWith("onnx-")) return "soniqo";
  if (model.startsWith("whisper-") || model.startsWith("Quantized")) {
    return "whispercpp";
  }
  return null;
}
