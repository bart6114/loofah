import { describe, expect, it } from "vitest";

import { _PROVIDERS } from "./shared";

describe("llm providers", () => {
  it("keeps every BYO-key/local provider and drops the hosted provider", () => {
    expect(_PROVIDERS.map((p) => p.id)).toEqual([
      "lmstudio",
      "ollama",
      "openrouter",
      "openai",
      "chatgpt_subscription",
      "cloudflare_workers_ai",
      "anthropic",
      "mistral",
      "azure_openai",
      "azure_ai",
      "google_generative_ai",
      "custom",
    ]);
  });
});
