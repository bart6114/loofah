import type { LLMConnectionInfo } from "./model";
import { PROVIDERS, type ProviderId } from "./providers";
export type ConnectionStatus =
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

export const resolveLLMConnection = (params: {
  providerId: string | undefined;
  modelId: string | undefined;
  providerConfig: { base_url?: string; api_key?: string } | undefined;
}): { conn: LLMConnectionInfo | null; status: ConnectionStatus } => {
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

  const missing = providerDefinition.requirements.flatMap((requirement) =>
    requirement.kind === "requires_config"
      ? requirement.fields.filter(
          (field) => !(field === "base_url" ? baseUrl : apiKey),
        )
      : [],
  );
  if (missing.length)
    return {
      conn: null,
      status: {
        status: "error",
        reason: "missing_config",
        providerId,
        missing,
      },
    };

  return {
    conn: { providerId, modelId, baseUrl, apiKey },
    status: { status: "success", providerId },
  };
};
