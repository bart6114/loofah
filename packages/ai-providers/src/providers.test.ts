import { generateText } from "ai";
import { describe, expect, test, vi } from "vitest";

import {
  createLanguageModel,
  listModels,
  resolveLLMConnection,
  PROVIDERS,
  type ProviderId,
} from ".";

describe("shared provider request contracts", () => {
  test.each([
    ["openai", "/responses", "authorization", "Bearer secret"],
    ["anthropic", "/messages", "x-api-key", "secret"],
    [
      "google_generative_ai",
      "/models/model:generateContent",
      "x-goog-api-key",
      "secret",
    ],
    ["openrouter", "/chat/completions", "authorization", "Bearer secret"],
    ["azure_openai", "/responses", "api-key", "secret"],
    ["azure_ai", "/chat/completions", "api-key", "secret"],
    ["mistral", "/chat/completions", "authorization", "Bearer secret"],
    ["custom", "/chat/completions", "authorization", "Bearer secret"],
    [
      "cloudflare_workers_ai",
      "/chat/completions",
      "authorization",
      "Bearer secret",
    ],
  ] as const)(
    "%s uses its existing protocol",
    async (providerId, path, header, value) => {
      const fetcher = vi.fn<typeof fetch>(async () => {
        throw new Error("request captured");
      });
      const model = createLanguageModel(
        {
          providerId,
          modelId: "model",
          baseUrl: "https://provider.example/v1",
          apiKey: "secret",
        },
        { fetch: fetcher },
      );
      await expect(
        generateText({
          model,
          system: "instructions",
          prompt: "source",
          maxRetries: 0,
        }),
      ).rejects.toThrow();
      const [url, init] = fetcher.mock.calls[0];
      expect(new URL(String(url)).origin).toBe("https://provider.example");
      expect(String(url)).toContain(path);
      if (providerId === "anthropic" || providerId === "openrouter") {
        expect(String(url)).toBe(`https://provider.example/v1${path}`);
      }
      expect(new Headers(init?.headers).get(header)).toBe(value);
      const body = JSON.parse(String(init?.body));
      expect(JSON.stringify(body)).toContain("source");
      expect(JSON.stringify(body)).toContain("instructions");
    },
  );

  test("Ollama keeps its server origin and needs no API key", async () => {
    const fetcher = vi.fn<typeof fetch>(async () => {
      throw new Error("captured");
    });
    const model = createLanguageModel(
      {
        providerId: "ollama",
        modelId: "model",
        baseUrl: "http://mac.local:11434/v1",
        apiKey: "",
      },
      { fetch: fetcher },
    );
    await expect(
      generateText({ model, prompt: "source", maxRetries: 0 }),
    ).rejects.toThrow();
    const [url, init] = fetcher.mock.calls[0];
    expect(url).toBe("http://mac.local:11434/v1/chat/completions");
    expect(new Headers(init?.headers).get("Origin")).toBe(
      "http://mac.local:11434",
    );
    expect(new Headers(init?.headers).has("Authorization")).toBe(false);
  });

  test("discovery uses the supplied fetch and preserves model filtering", async () => {
    const fetcher = vi.fn<typeof fetch>(async () =>
      Response.json({ data: [{ id: "gpt-5.5" }, { id: "whisper-1" }] }),
    );
    const result = await listModels({
      providerId: "openai",
      baseUrl: "https://provider.example/v1",
      apiKey: "secret",
      fetch: fetcher,
    });
    expect(fetcher).toHaveBeenCalledWith("https://provider.example/v1/models", {
      method: "GET",
      headers: { Authorization: "Bearer secret" },
    });
    expect(result.models).toEqual(["gpt-5.5"]);
    expect(result.ignored[0].id).toBe("whisper-1");
  });

  test("reasoning tags remain separate from summary text", async () => {
    const fetcher = vi.fn<typeof fetch>(async () =>
      Response.json({
        id: "response",
        object: "chat.completion",
        created: 0,
        model: "model",
        choices: [
          {
            index: 0,
            message: {
              role: "assistant",
              content: "<think>private reasoning</think>Summary",
            },
            finish_reason: "stop",
          },
        ],
        usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
      }),
    );
    const model = createLanguageModel(
      {
        providerId: "custom",
        modelId: "model",
        baseUrl: "https://provider.example/v1",
        apiKey: "secret",
      },
      { fetch: fetcher },
    );
    const result = await generateText({ model, prompt: "source" });
    expect(result.text).toBe("Summary");
    expect(result.reasoningText).toBe("private reasoning");
  });

  test("connection resolution shares defaults and required fields", () => {
    expect(
      resolveLLMConnection({
        providerId: "openai",
        modelId: "model",
        providerConfig: { api_key: "  secret  " },
      }).conn,
    ).toEqual({
      providerId: "openai",
      modelId: "model",
      baseUrl: "https://api.openai.com/v1",
      apiKey: "secret",
    });
    expect(
      resolveLLMConnection({
        providerId: "azure_ai",
        modelId: "model",
        providerConfig: undefined,
      }).status,
    ).toEqual({
      status: "error",
      reason: "missing_config",
      providerId: "azure_ai",
      missing: ["base_url", "api_key"],
    });
  });

  test("desktop subscription requires its native adapter", () => {
    expect(
      PROVIDERS.find((p) => p.id === "chatgpt_subscription")?.mobileSupported,
    ).toBe(false);
    expect(() =>
      createLanguageModel(
        {
          providerId: "chatgpt_subscription" as ProviderId,
          modelId: "model",
          baseUrl: "",
          apiKey: "",
        },
        { fetch },
      ),
    ).toThrow("desktop runtime");
  });
});
