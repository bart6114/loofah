import { describe, expect, test } from "vitest";

import { getBatchProvider } from "./batch-provider";

describe("getBatchProvider", () => {
  test.each([
    "soniqo-parakeet-batch",
    "onnx-parakeet-streaming",
    "onnx-parakeet-batch",
  ])("routes %s through the native Parakeet batch engine", (model) => {
    expect(getBatchProvider("fmtr", model)).toBe("soniqo");
  });

  test("rejects retired models", () => {
    expect(getBatchProvider("fmtr", "am-parakeet-v3")).toBeNull();
  });

  test.each([
    "whisper-large-v3",
    "QuantizedLargeTurbo",
    "QuantizedSmall",
    "QuantizedSmallEn",
    "QuantizedBase",
    "QuantizedBaseEn",
    "QuantizedTiny",
    "QuantizedTinyEn",
  ])("routes %s through the Whisper batch engine", (model) => {
    expect(getBatchProvider("fmtr", model)).toBe("whispercpp");
  });

  test("returns null for any non-on-device provider — STT is on-device only", () => {
    expect(getBatchProvider("deepgram", "nova-3-general")).toBeNull();
    expect(getBatchProvider("custom", "whisper-large-v3")).toBeNull();
  });
});
