import { type QueryClient, useQuery } from "@tanstack/react-query";

import type { ListModelsResult } from "~/settings/ai/shared/list-common";
import {
  commands,
  type ChatgptError,
  type ChatgptModel,
} from "~/types/tauri.gen";

export const CHATGPT_PROVIDER = "chatgpt_subscription";
export const CHATGPT_ACCOUNT_KEY = ["chatgpt-account"];
const metadata = new Map<string, ChatgptModel>();

export function unwrapChatgpt<T>(
  result: { status: "ok"; data: T } | { status: "error"; error: ChatgptError },
): T {
  if (result.status === "error") {
    throw Object.assign(new Error(result.error.message), result.error);
  }
  return result.data;
}

export function useChatgptAccount(enabled: boolean) {
  return useQuery({
    queryKey: CHATGPT_ACCOUNT_KEY,
    queryFn: async () => unwrapChatgpt(await commands.chatgptAccount()),
    enabled,
    retry: false,
    staleTime: (query) => (query.state.data ? 30_000 : 0),
  });
}

export async function refreshChatgptConnection(queryClient: QueryClient) {
  await queryClient.cancelQueries({ queryKey: CHATGPT_ACCOUNT_KEY });
  await queryClient.cancelQueries({ queryKey: ["models", CHATGPT_PROVIDER] });
  await queryClient.fetchQuery({
    queryKey: CHATGPT_ACCOUNT_KEY,
    queryFn: async () => unwrapChatgpt(await commands.chatgptAccount()),
    staleTime: 0,
  });
  await queryClient.invalidateQueries({
    queryKey: ["models", CHATGPT_PROVIDER],
    refetchType: "none",
  });
  return queryClient.fetchQuery({
    queryKey: ["models", CHATGPT_PROVIDER],
    queryFn: listChatgptModels,
    staleTime: 0,
  });
}

export async function listChatgptModels(): Promise<ListModelsResult> {
  const models = unwrapChatgpt(await commands.chatgptModels());
  metadata.clear();
  for (const model of models) metadata.set(model.model, model);
  return {
    models: models.map((model) => model.model),
    ignored: [],
    metadata: Object.fromEntries(
      models.map((model) => [
        model.model,
        {
          input_modalities: (model.inputModalities ?? []).filter(
            (m): m is "image" | "text" => m === "image" || m === "text",
          ),
        },
      ]),
    ),
  };
}

export function chatgptModelSupportsImages(model: string) {
  return metadata.get(model)?.inputModalities?.includes("image") ?? false;
}
