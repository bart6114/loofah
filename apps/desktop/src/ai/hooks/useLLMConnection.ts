import { createAnthropic } from "@ai-sdk/anthropic";
import { createAzure } from "@ai-sdk/azure";
import { createGoogleGenerativeAI } from "@ai-sdk/google";
import { createOpenAI } from "@ai-sdk/openai";
import { createOpenAICompatible } from "@ai-sdk/openai-compatible";
import { createOpenRouter } from "@openrouter/ai-sdk-provider";
import { useQuery } from "@tanstack/react-query";
import { fetch as tauriFetch } from "@tauri-apps/plugin-http";
import { extractReasoningMiddleware, wrapLanguageModel } from "ai";
import { useMemo } from "react";

import type { AIProviderStorage } from "@hypr/store";

import {
  CHATGPT_PROVIDER,
  useChatgptAccount,
  listChatgptModels,
} from "~/ai/chatgpt-account";
import { createChatgptModel } from "~/ai/chatgpt-model";
import { type ProviderId, PROVIDERS } from "~/settings/ai/llm/shared";
import { getProviderSelectionBlockers } from "~/settings/ai/shared/eligibility";
import { useAiProvider } from "~/settings/providers";
import { useConfigValues } from "~/shared/config";

type LanguageModelV3 = Parameters<typeof wrapLanguageModel>[0]["model"];

type LLMConnectionInfo = {
  providerId: ProviderId;
  modelId: string;
  baseUrl: string;
  apiKey: string;
};

export type LLMConnectionStatus =
  | { status: "pending"; reason: "connecting"; providerId: ProviderId }
  | {
      status: "error";
      reason: "chatgpt";
      providerId: ProviderId;
      message: string;
    }
  | { status: "pending"; reason: "missing_provider" }
  | { status: "pending"; reason: "missing_model"; providerId: ProviderId }
  | { status: "error"; reason: "provider_not_found"; providerId: string }
  | {
      status: "error";
      reason: "missing_config";
      providerId: ProviderId;
      missing: Array<"base_url" | "api_key">;
    }
  | { status: "success"; providerId: ProviderId };

type LLMConnectionResult = {
  conn: LLMConnectionInfo | null;
  status: LLMConnectionStatus;
};

export const useLanguageModel = (): LanguageModelV3 | null => {
  const { conn } = useLLMConnection();

  return useMemo(() => {
    if (!conn) return null;

    return createLanguageModel(conn);
  }, [conn]);
};

export const useLLMConnection = (): LLMConnectionResult => {
  const { current_llm_provider, current_llm_model } = useConfigValues([
    "current_llm_provider",
    "current_llm_model",
  ] as const);
  const providerConfig = useAiProvider("llm", current_llm_provider) as
    | AIProviderStorage
    | undefined;

  const account = useChatgptAccount(current_llm_provider === CHATGPT_PROVIDER);
  const models = useQuery({
    queryKey: ["models", CHATGPT_PROVIDER],
    queryFn: listChatgptModels,
    enabled: current_llm_provider === CHATGPT_PROVIDER && !!account.data,
    staleTime: 30_000,
    retry: false,
  });

  return useMemo<LLMConnectionResult>(() => {
    if (current_llm_provider === CHATGPT_PROVIDER) {
      if (account.isPending || (account.data && models.isPending))
        return {
          conn: null,
          status: {
            status: "pending",
            reason: "connecting",
            providerId: CHATGPT_PROVIDER,
          },
        };
      const message =
        account.error?.message ??
        (!account.data
          ? "Sign in to ChatGPT in Intelligence settings."
          : (models.error?.message ??
            (!models.data?.models.includes(current_llm_model ?? "")
              ? "Select an available ChatGPT model in Intelligence settings."
              : null)));
      if (message)
        return {
          conn: null,
          status: {
            status: "error",
            reason: "chatgpt",
            providerId: CHATGPT_PROVIDER,
            message,
          },
        };
    }
    return resolveLLMConnection({
      providerId: current_llm_provider,
      modelId: current_llm_model,
      providerConfig,
    });
  }, [
    current_llm_model,
    current_llm_provider,
    providerConfig,
    account.data,
    account.error,
    account.isPending,
    models.data,
    models.error,
    models.isPending,
  ]);
};

export const useLLMConnectionStatus = (): LLMConnectionStatus => {
  const { status } = useLLMConnection();
  return status;
};

const resolveLLMConnection = (params: {
  providerId: string | undefined;
  modelId: string | undefined;
  providerConfig: AIProviderStorage | undefined;
}): LLMConnectionResult => {
  const { providerId: rawProviderId, modelId, providerConfig } = params;

  if (!rawProviderId) {
    return {
      conn: null,
      status: { status: "pending", reason: "missing_provider" },
    };
  }

  const providerId = rawProviderId as ProviderId;

  if (!modelId) {
    return {
      conn: null,
      status: { status: "pending", reason: "missing_model", providerId },
    };
  }

  const providerDefinition = PROVIDERS.find((p) => p.id === rawProviderId);

  if (!providerDefinition) {
    return {
      conn: null,
      status: {
        status: "error",
        reason: "provider_not_found",
        providerId: rawProviderId,
      },
    };
  }

  const baseUrl =
    providerConfig?.base_url?.trim() ||
    providerDefinition.baseUrl?.trim() ||
    "";
  const apiKey = providerConfig?.api_key?.trim() || "";

  const blockers = getProviderSelectionBlockers(
    providerDefinition.requirements,
    {
      isAuthenticated: false,
      isPaid: true,
      config: { base_url: baseUrl, api_key: apiKey },
    },
  );

  if (blockers.length > 0) {
    const blocker = blockers[0];
    if (blocker.code === "missing_config") {
      return {
        conn: null,
        status: {
          status: "error",
          reason: "missing_config",
          providerId,
          missing: blocker.fields,
        },
      };
    }
  }

  return {
    conn: { providerId, modelId, baseUrl, apiKey },
    status: { status: "success", providerId },
  };
};

const wrapWithThinkingMiddleware = (
  model: LanguageModelV3,
): LanguageModelV3 => {
  return wrapLanguageModel({
    model,
    middleware: [
      extractReasoningMiddleware({ tagName: "think" }),
      extractReasoningMiddleware({ tagName: "thinking" }),
    ],
  });
};

const createLanguageModel = (conn: LLMConnectionInfo): LanguageModelV3 => {
  switch (conn.providerId) {
    case CHATGPT_PROVIDER:
      return createChatgptModel(conn.modelId);
    case "anthropic": {
      const provider = createAnthropic({
        fetch: tauriFetch,
        apiKey: conn.apiKey,
        headers: {
          "anthropic-version": "2023-06-01",
          "anthropic-dangerous-direct-browser-access": "true",
        },
      });
      return wrapWithThinkingMiddleware(provider(conn.modelId));
    }

    case "google_generative_ai": {
      const provider = createGoogleGenerativeAI({
        fetch: tauriFetch,
        baseURL: conn.baseUrl,
        apiKey: conn.apiKey,
      });
      return wrapWithThinkingMiddleware(provider(conn.modelId));
    }

    case "openrouter": {
      const provider = createOpenRouter({
        fetch: tauriFetch,
        apiKey: conn.apiKey,
      });
      return wrapWithThinkingMiddleware(provider.chat(conn.modelId));
    }

    case "openai": {
      const provider = createOpenAI({
        fetch: tauriFetch,
        baseURL: conn.baseUrl,
        apiKey: conn.apiKey,
      });
      return wrapWithThinkingMiddleware(provider(conn.modelId));
    }

    case "azure_openai": {
      const provider = createAzure({
        fetch: tauriFetch,
        baseURL: conn.baseUrl,
        apiKey: conn.apiKey,
      });
      return wrapWithThinkingMiddleware(provider(conn.modelId));
    }

    case "azure_ai": {
      const provider = createOpenAICompatible({
        fetch: tauriFetch,
        name: "azure_ai",
        baseURL: conn.baseUrl,
        apiKey: conn.apiKey,
        headers: { "api-key": conn.apiKey },
      });
      return wrapWithThinkingMiddleware(provider.chatModel(conn.modelId));
    }

    case "ollama": {
      const ollamaOrigin = new URL(conn.baseUrl.replace(/\/v1\/?$/, "")).origin;
      const ollamaFetch: typeof fetch = async (input, init) => {
        const headers = new Headers(init?.headers);
        headers.set("Origin", ollamaOrigin);
        return tauriFetch(input as RequestInfo | URL, {
          ...init,
          headers,
        });
      };
      const provider = createOpenAICompatible({
        fetch: ollamaFetch,
        name: conn.providerId,
        baseURL: conn.baseUrl,
      });
      return wrapWithThinkingMiddleware(provider.chatModel(conn.modelId));
    }

    default: {
      const config: Parameters<typeof createOpenAICompatible>[0] = {
        fetch: tauriFetch,
        name: conn.providerId,
        baseURL: conn.baseUrl,
      };
      if (conn.apiKey) {
        config.apiKey = conn.apiKey;
      }
      const provider = createOpenAICompatible(config);
      return wrapWithThinkingMiddleware(provider.chatModel(conn.modelId));
    }
  }
};
