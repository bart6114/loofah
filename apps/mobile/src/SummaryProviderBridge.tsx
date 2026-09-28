import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { fetch as tauriFetch } from "@tauri-apps/plugin-http";
import { generateText } from "ai";
import { useEffect } from "react";

import {
  AI_GENERATION_MAX_RETRIES,
  createLanguageModel,
  type LLMConnectionInfo,
  SUMMARY_MAX_OUTPUT_TOKENS,
} from "@hypr/ai-providers";

let readiness = Promise.resolve();

function updateReadiness(ready: boolean, enabled = () => true) {
  readiness = readiness
    .catch(() => {})
    .then(async () => {
      if (enabled()) await invoke("mobile_summary_bridge_ready", { ready });
    });
  return readiness;
}

export function SummaryProviderBridge() {
  useEffect(() => {
    let mounted = true;
    const unlisteners: (() => void)[] = [];
    let active: { id: string; controller: AbortController } | null = null;

    async function generate(request: {
      id: string;
      instructions: string;
      input: string;
      maxOutputTokens?: number | null;
      connection: LLMConnectionInfo;
    }) {
      if (!mounted) return;
      active?.controller.abort();
      const controller = new AbortController();
      active = { id: request.id, controller };
      let text: string | null = null;
      let error: string | null = null;
      try {
        const result = await generateText({
          model: createLanguageModel(request.connection, { fetch: tauriFetch }),
          system: request.instructions,
          prompt: request.input,
          maxOutputTokens: request.maxOutputTokens ?? SUMMARY_MAX_OUTPUT_TOKENS,
          maxRetries: AI_GENERATION_MAX_RETRIES,
          abortSignal: controller.signal,
        });
        if (result.finishReason === "length") {
          throw new Error(
            "The model reached its output limit before completing the summary. Choose another model and retry.",
          );
        }
        if (!result.text.trim()) {
          throw new Error("The summary provider returned no text. Try again.");
        }
        text = result.text;
      } catch (failure) {
        error =
          failure instanceof Error
            ? failure.message
            : typeof failure === "string"
              ? failure
              : "The summary provider could not generate a summary. Try again.";
        if (request.connection.apiKey) {
          for (const secret of [
            request.connection.apiKey,
            encodeURIComponent(request.connection.apiKey),
          ]) {
            error = error.split(secret).join("[redacted]");
          }
        }
        error = error.slice(0, 1500);
      }
      if (
        !mounted ||
        controller.signal.aborted ||
        active?.controller !== controller
      ) {
        return;
      }
      active = null;
      await invoke("mobile_summary_complete", {
        requestId: request.id,
        text,
        error,
      }).catch(() => {});
    }

    async function install() {
      try {
        const requestListener = await listen<Parameters<typeof generate>[0]>(
          "mobile-summary-request",
          ({ payload }) => void generate(payload),
        );
        if (!mounted) {
          requestListener();
          return;
        }
        unlisteners.push(requestListener);
        const abortListener = await listen<string>(
          "mobile-summary-abort",
          ({ payload }) => {
            if (mounted && active?.id === payload) {
              active.controller.abort();
              active = null;
            }
          },
        );
        if (!mounted) {
          abortListener();
          return;
        }
        unlisteners.push(abortListener);
        await updateReadiness(true, () => mounted);
      } catch {
        for (const unlisten of unlisteners.splice(0)) unlisten();
        active?.controller.abort();
        active = null;
        if (mounted) await updateReadiness(false).catch(() => {});
      }
    }
    void install();
    return () => {
      mounted = false;
      for (const unlisten of unlisteners.splice(0)) unlisten();
      active?.controller.abort();
      active = null;
      void updateReadiness(false).catch(() => {});
    };
  }, []);
  return null;
}
