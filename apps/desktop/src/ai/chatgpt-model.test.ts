import { generateText, streamText } from "ai";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { createChatgptModel, toChatgptRequest } from "./chatgpt-model";

import type { ChatgptEvent, ChatgptGeneration } from "~/types/tauri.gen";

const mocks = vi.hoisted(() => ({ generate: vi.fn(), cancel: vi.fn() }));
vi.mock("~/types/tauri.gen", () => ({
  commands: {
    chatgptGenerate: mocks.generate,
    chatgptCancelGeneration: mocks.cancel,
  },
}));
vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    onmessage = (_event: unknown) => {};
  },
}));

const model = createChatgptModel("test-model");
const prompt = [
  {
    role: "user" as const,
    content: [{ type: "text" as const, text: "Summarize the meeting." }],
  },
];

beforeEach(() => {
  vi.clearAllMocks();
  mocks.cancel.mockResolvedValue(undefined);
});

describe("ChatGPT model adapter", () => {
  it("runs the existing AI SDK generation path and preserves usage", async () => {
    mocks.generate.mockImplementation(
      async (
        _request: ChatgptGeneration,
        events: { onmessage: (event: ChatgptEvent) => void },
      ) => {
        events.onmessage({ type: "text", delta: "Launch " });
        events.onmessage({ type: "text", delta: "on Friday." });
        events.onmessage({
          type: "complete",
          input_tokens: 12,
          output_tokens: 4,
        });
        return { status: "ok", data: null };
      },
    );
    const result = await generateText({
      model,
      system: "Use bullet points.",
      prompt: "Summarize the meeting.",
    });
    expect(result.text).toBe("Launch on Friday.");
    expect(result.usage.inputTokens).toBe(12);
    expect(mocks.generate.mock.calls[0][0]).toMatchObject({
      system: "Use bullet points.",
      prompt: "Summarize the meeting.",
      model: "test-model",
    });
  });

  it("does not retry a quota failure", async () => {
    mocks.generate.mockResolvedValue({
      status: "error",
      error: {
        code: "quota",
        message: "Allowance exhausted",
        retryable: false,
      },
    });
    const onError = vi.fn();
    const result = streamText({
      model,
      prompt: "Summarize.",
      maxRetries: 4,
      onError,
    });
    await result.consumeStream();
    expect(onError).toHaveBeenCalledWith(
      expect.objectContaining({
        error: expect.objectContaining({ code: "quota" }),
      }),
    );
    expect(mocks.generate).toHaveBeenCalledTimes(1);
  });

  it("cancels a request even when abort arrives before backend acknowledgement", async () => {
    let acknowledge!: (value: unknown) => void;
    mocks.generate.mockImplementation(
      () =>
        new Promise((resolve) => {
          acknowledge = resolve;
        }),
    );
    const abort = new AbortController();
    const { stream } = await model.doStream({
      prompt,
      abortSignal: abort.signal,
    });
    const reader = stream.getReader();
    abort.abort();
    await expect(reader.read()).rejects.toMatchObject({ name: "AbortError" });
    acknowledge({ status: "ok", data: null });
    await vi.waitFor(() =>
      expect(mocks.cancel).toHaveBeenCalledWith(
        mocks.generate.mock.calls[0][0].requestId,
      ),
    );
  });

  it("cancels the backend when a stream consumer stops reading", async () => {
    mocks.generate.mockResolvedValue({ status: "ok", data: null });
    const { stream } = await model.doStream({ prompt });
    await stream.cancel();
    await vi.waitFor(() => expect(mocks.cancel).toHaveBeenCalled());
  });

  it("passes attached image bytes without granting local file access", () => {
    const request = toChatgptRequest("test-model", {
      prompt: [
        {
          role: "user",
          content: [
            { type: "text", text: "Use this diagram." },
            {
              type: "file",
              mediaType: "image/png",
              data: new Uint8Array([1, 2, 3]),
            },
          ],
        },
      ],
    });
    expect(request.images).toEqual(["data:image/png;base64,AQID"]);
    expect(request.prompt).toBe("Use this diagram.");
  });

  it("rejects unsupported capabilities instead of silently changing the request", () => {
    expect(() =>
      toChatgptRequest("test-model", { prompt, temperature: 0 }),
    ).toThrow("sampling controls");
    expect(() =>
      toChatgptRequest("test-model", {
        prompt,
        responseFormat: { type: "json" },
      }),
    ).toThrow();
  });
});
