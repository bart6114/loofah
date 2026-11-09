import { describe, expect, test } from "vitest";

import {
  CLOUDFLARE_WORKERS_AI_MODELS,
  createStaticCloudflareWorkersAIModelResult,
} from "./list-cloudflare-workers-ai";

describe("createStaticCloudflareWorkersAIModelResult", () => {
  test("keeps a selectable fallback for Workers AI chat models", () => {
    const result = createStaticCloudflareWorkersAIModelResult();

    expect(result.models).toEqual([...CLOUDFLARE_WORKERS_AI_MODELS]);
    expect(result.models[0]).toBe("@cf/moonshotai/kimi-k2.6");
    expect(result.metadata["@cf/moonshotai/kimi-k2.6"]).toEqual({
      input_modalities: ["text", "image"],
    });
    expect(result.metadata["@cf/openai/gpt-oss-120b"]).toEqual({
      input_modalities: ["text"],
    });
  });
});

test("the shared catalog preserves unique model order and explicit modalities", () => {
  const result = createStaticCloudflareWorkersAIModelResult();
  expect(result.models).toHaveLength(10);
  expect(new Set(result.models).size).toBe(result.models.length);
  expect(result.models[9]).toBe("@cf/meta/llama-3.3-70b-instruct-fp8-fast");
  expect(
    result.metadata["@cf/meta/llama-4-scout-17b-16e-instruct"].input_modalities,
  ).toEqual(["text", "image"]);
  expect(
    result.metadata["@cf/qwen/qwen3-30b-a3b-fp8"].input_modalities,
  ).toEqual(["text"]);
});
