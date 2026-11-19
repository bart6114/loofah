import { ask } from "@tauri-apps/plugin-dialog";

import type { LLMConnectionInfo } from "~/ai/hooks/useLLMConnection";
import { PROVIDERS } from "~/settings/ai/llm/shared";
import { commands } from "~/types/tauri.gen";

const approved = new Set<string>();

export function captureAuthorization(connection: LLMConnectionInfo | null) {
  if (!connection) return;
  const provider = PROVIDERS.find(({ id }) => id === connection.providerId);
  const fixedEndpoint =
    connection.providerId === "anthropic" ||
    connection.providerId === "openrouter";
  const endpoint = fixedEndpoint ? provider?.baseUrl : connection.baseUrl;
  const url = URL.canParse(endpoint ?? "") ? new URL(endpoint!) : null;
  if (
    ["ollama", "lmstudio", "custom"].includes(connection.providerId) &&
    url &&
    ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname)
  ) {
    return;
  }
  const name = provider?.displayName ?? connection.providerId;
  const recipient = url ? `${name} (${url.host})` : name;

  return async (sessionId: string) => {
    const result = await commands.isIphoneCapture(sessionId);
    if (result.status === "error") throw new Error(result.error);
    if (!result.data) return true;

    const key = JSON.stringify([
      sessionId,
      connection.providerId,
      endpoint,
      connection.modelId,
    ]);
    if (approved.has(key)) return true;
    const onward =
      connection.providerId === "openrouter"
        ? " OpenRouter forwards the request to the model provider it selects."
        : "";
    const allowed = await ask(
      `Loofah will send this iPhone capture’s notes, transcript, available session details, and summary instructions to ${recipient} using ${connection.modelId}. It may also send the generated summary to create a title.${onward}`,
      {
        title: "Share this iPhone capture for a summary?",
        okLabel: "Share and summarize",
        cancelLabel: "Keep on Mac",
      },
    );
    if (allowed) approved.add(key);
    return allowed;
  };
}
