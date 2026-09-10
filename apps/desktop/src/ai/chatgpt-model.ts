import type {
  LanguageModelV3,
  LanguageModelV3CallOptions,
  LanguageModelV3StreamPart,
  LanguageModelV3Usage,
  SharedV3Warning,
} from "@ai-sdk/provider";
import { Channel } from "@tauri-apps/api/core";

import { CHATGPT_PROVIDER, unwrapChatgpt } from "./chatgpt-account";

import {
  commands,
  type ChatgptEvent,
  type ChatgptGeneration,
} from "~/types/tauri.gen";

const emptyUsage = (): LanguageModelV3Usage => ({
  inputTokens: {
    total: undefined,
    noCache: undefined,
    cacheRead: undefined,
    cacheWrite: undefined,
  },
  outputTokens: { total: undefined, text: undefined, reasoning: undefined },
});

export function createChatgptModel(modelId: string): LanguageModelV3 {
  const model: LanguageModelV3 = {
    specificationVersion: "v3",
    provider: CHATGPT_PROVIDER,
    modelId,
    supportedUrls: {},
    async doGenerate(options) {
      const { stream } = await model.doStream(options);
      const reader = stream.getReader();
      let text = "";
      let usage = emptyUsage();
      const warnings: SharedV3Warning[] = [];
      try {
        while (true) {
          const { value, done } = await reader.read();
          if (done) break;
          if (value.type === "text-delta") text += value.delta;
          if (value.type === "stream-start") warnings.push(...value.warnings);
          if (value.type === "finish") usage = value.usage;
          if (value.type === "error") throw value.error;
        }
      } finally {
        reader.releaseLock();
      }
      return {
        content: [{ type: "text", text }],
        usage,
        warnings,
        finishReason: { unified: "stop", raw: "completed" },
      };
    },
    async doStream(options) {
      options.abortSignal?.throwIfAborted();
      const request = toChatgptRequest(modelId, options);
      const warnings: SharedV3Warning[] = [];
      if (options.maxOutputTokens !== undefined)
        warnings.push({
          type: "unsupported",
          feature: "maxOutputTokens",
          details:
            "ChatGPT uses this as prompt guidance, not a hard token limit.",
        });
      const id = request.requestId;
      let settled = false;
      let started = false;
      let controller: ReadableStreamDefaultController<LanguageModelV3StreamPart>;
      const cancel = async () => {
        await commands.chatgptCancelGeneration(id);
      };
      const cleanup = () =>
        options.abortSignal?.removeEventListener("abort", abort);
      const fail = (error: unknown) => {
        if (settled) return;
        settled = true;
        cleanup();
        if (error instanceof DOMException && error.name === "AbortError") {
          controller.error(error);
        } else {
          controller.enqueue({ type: "error", error });
          controller.close();
        }
      };
      const abort = () => {
        fail(new DOMException("Generation cancelled", "AbortError"));
        if (started) void cancel().catch(() => {});
      };
      const channel = new Channel<ChatgptEvent>();
      channel.onmessage = (event) => {
        if (settled) return;
        if (event.type === "text")
          controller.enqueue({ type: "text-delta", id, delta: event.delta });
        if (event.type === "error") {
          fail(
            event.error.code === "cancelled"
              ? new DOMException(event.error.message, "AbortError")
              : Object.assign(new Error(event.error.message), event.error),
          );
        }
        if (event.type === "complete") {
          const usage = emptyUsage();
          usage.inputTokens.total = event.input_tokens ?? undefined;
          usage.outputTokens.total = event.output_tokens ?? undefined;
          controller.enqueue({ type: "text-end", id });
          controller.enqueue({
            type: "finish",
            finishReason: { unified: "stop", raw: "completed" },
            usage,
          });
          settled = true;
          cleanup();
          controller.close();
        }
      };
      const stream = new ReadableStream<LanguageModelV3StreamPart>({
        async start(c) {
          controller = c;
          c.enqueue({ type: "stream-start", warnings });
          c.enqueue({ type: "text-start", id });
          options.abortSignal?.addEventListener("abort", abort, { once: true });
          if (options.abortSignal?.aborted) {
            abort();
            return;
          }
          try {
            unwrapChatgpt(await commands.chatgptGenerate(request, channel));
            started = true;
            if (settled || options.abortSignal?.aborted) await cancel();
          } catch (error) {
            fail(error);
          }
        },
        async cancel() {
          settled = true;
          cleanup();
          if (started) await cancel();
        },
      });
      return { stream };
    },
  };
  return model;
}

export function toChatgptRequest(
  model: string,
  options: LanguageModelV3CallOptions,
): ChatgptGeneration {
  if (
    options.tools?.length ||
    options.responseFormat?.type === "json" ||
    options.stopSequences?.length ||
    options.temperature !== undefined ||
    options.topP !== undefined ||
    options.topK !== undefined ||
    options.frequencyPenalty !== undefined ||
    options.presencePenalty !== undefined ||
    options.seed !== undefined
  ) {
    throw new Error(
      "This ChatGPT provider supports text and image summaries without tools or sampling controls.",
    );
  }
  const system: string[] = [];
  const prompt: string[] = [];
  const images: string[] = [];
  for (const message of options.prompt) {
    if (message.role === "system") {
      system.push(message.content);
      continue;
    }
    if (message.role !== "user")
      throw new Error(
        "This ChatGPT provider only supports single-request generation.",
      );
    for (const part of message.content) {
      if (part.type === "text") prompt.push(part.text);
      else if (
        part.type === "file" &&
        part.mediaType.startsWith("image/") &&
        !(part.data instanceof URL)
      ) {
        const base64 =
          typeof part.data === "string"
            ? part.data
            : btoa(
                Array.from(part.data, (byte) => String.fromCharCode(byte)).join(
                  "",
                ),
              );
        images.push(`data:${part.mediaType};base64,${base64}`);
      } else {
        throw new Error(
          "This ChatGPT provider only supports text and attached images.",
        );
      }
    }
  }
  return {
    requestId: crypto.randomUUID(),
    model,
    system: system.join("\n\n"),
    prompt: prompt.join("\n\n"),
    images,
    maxOutputTokens: options.maxOutputTokens ?? null,
  };
}
