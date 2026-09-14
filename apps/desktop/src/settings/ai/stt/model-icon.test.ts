import { describe, expect, test } from "vitest";

import { getLocalModelBackendBadge, getLocalModelIcon } from "./model-icon";

describe("local model icons", () => {
  test("prioritizes model family over provider name", () => {
    expect(getLocalModelIcon("soniqo-parakeet-streaming")?.title).toBe(
      "NVIDIA Parakeet",
    );
  });

  test("uses researched logo assets for known model families", () => {
    expect(getLocalModelIcon("soniqo-omnilingual")?.imageSrc).toBe(
      "/assets/model-icons/meta-logo.svg",
    );
    expect(getLocalModelIcon("QuantizedSmall")?.imageSrc).toBe(
      "/assets/model-icons/openai-logo.svg",
    );
    expect(getLocalModelIcon("soniqo-parakeet-batch")?.imageSrc).toBe(
      "/assets/model-icons/nvidia-logo.svg",
    );
  });

  test("returns runtime badges for hardware and model runtimes", () => {
    expect(getLocalModelBackendBadge("whisper-small-apple-npu")?.label).toBe(
      "NPU",
    );
    expect(getLocalModelBackendBadge("whisper-ggml")?.label).toBe("GGML");
    expect(getLocalModelBackendBadge("whisper-nvidia-cuda")?.label).toBe("NV");
  });
});
