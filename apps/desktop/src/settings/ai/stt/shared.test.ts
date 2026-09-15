import { describe, expect, test } from "vitest";

import { displayModelLabel } from "./shared";

describe("STT model display labels", () => {
  test.each([
    ["onnx-parakeet-streaming", "Parakeet Streaming"],
    ["onnx-parakeet-batch", "Parakeet Batch"],
    ["soniqo-parakeet-streaming", "Soniqo Parakeet Streaming"],
    ["soniqo-parakeet-batch", "Soniqo Parakeet Batch"],
  ])("preserves the display name for %s", (model, displayName) => {
    expect(displayModelLabel(model, displayName)).toBe(displayName);
  });

  test("falls back to the raw model id when there is no display name", () => {
    expect(displayModelLabel("some-unknown-model")).toBe("some-unknown-model");
    expect(displayModelLabel("onnx-parakeet-streaming")).toBe(
      "onnx-parakeet-streaming",
    );
  });
});
