import { useQuery } from "@tanstack/react-query";
import { fetch as tauriFetch } from "@tauri-apps/plugin-http";
import { useMemo } from "react";

import {
  createLanguageModel as createSharedLanguageModel,
  resolveLLMConnection,
  type ConnectionStatus,
  type LLMConnectionInfo,
  type LanguageModelV3,
} from "@hypr/ai-providers";
import type { AIProviderStorage } from "@hypr/store";

import {
  CHATGPT_PROVIDER,
  useChatgptAccount,
  listChatgptModels,
} from "~/ai/chatgpt-account";
import { createChatgptModel } from "~/ai/chatgpt-model";
import { useAiProvider } from "~/settings/providers";
import { useConfigValues } from "~/shared/config";

export type LLMConnectionStatus = ConnectionStatus;

type LLMConnectionResult = {
  conn: LLMConnectionInfo | null;
  status: LLMConnectionStatus;
};

export const useLanguageModel = (): LanguageModelV3 | null => {
  const { conn } = useLLMConnection();

  return useMemo(() => {
    if (!conn) return null;

    return conn.providerId === CHATGPT_PROVIDER
      ? createChatgptModel(conn.modelId)
      : createSharedLanguageModel(conn, { fetch: tauriFetch });
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
          ? "Sign in to ChatGPT in summary settings."
          : (models.error?.message ??
            (!models.data?.models.includes(current_llm_model ?? "")
              ? "Select an available ChatGPT model in summary settings."
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
