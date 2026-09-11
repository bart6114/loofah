import { useQuery } from "@tanstack/react-query";
import { generateText } from "ai";
import { useEffect } from "react";

import { Spinner } from "@hypr/ui/components/ui/spinner";

import { CHATGPT_PROVIDER } from "~/ai/chatgpt-account";
import { useLanguageModel, useLLMConnectionStatus } from "~/ai/hooks";
import { useConfigValues } from "~/shared/config";

export type LlmHealthStatus = {
  status: "pending" | "error" | "success" | null;
  message?: string;
};

export function HealthStatusIndicator() {
  const health = useConnectionHealth();

  if (health.status === "pending") {
    return <Spinner size={14} className="text-muted-foreground shrink-0" />;
  }

  return null;
}

export function useConnectionHealth(): LlmHealthStatus {
  const model = useLanguageModel();
  const connection = useLLMConnectionStatus();
  const { current_llm_provider } = useConfigValues([
    "current_llm_provider",
  ] as const);
  const isChatgpt = current_llm_provider === CHATGPT_PROVIDER;

  const text = useQuery({
    enabled: !!model && !isChatgpt,
    queryKey: ["llm-health-check", model],
    staleTime: 0,
    retry: 5,
    retryDelay: 200,
    queryFn: async () => {
      const result = await generateText({
        model: model!,
        system: "If user says hi, respond with hello, without any other text.",
        prompt: "Hi",
      });
      return result;
    },
  });

  const { refetch } = text;
  useEffect(() => {
    if (model && !isChatgpt) {
      void refetch();
    }
  }, [model, refetch, isChatgpt]);

  if (isChatgpt) {
    return {
      status: connection.status,
      message:
        connection.status === "error" && connection.reason === "chatgpt"
          ? connection.message
          : undefined,
    };
  }

  if (!model) {
    return { status: null };
  }

  if (text.status === "error") {
    const error = text.error as Error;
    const message = error.message || "Unknown error";
    return {
      status: "error",
      message: `Connection failed: ${message}`,
    };
  }

  return { status: text.status };
}
