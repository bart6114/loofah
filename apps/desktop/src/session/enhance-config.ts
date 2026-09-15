import type { LLMConnectionStatus } from "~/ai/hooks";

export function shouldShowEmptySummaryConfigError(status: LLMConnectionStatus) {
  if (status.status === "pending") {
    return (
      status.reason === "missing_provider" || status.reason === "missing_model"
    );
  }

  if (status.status !== "error") {
    return false;
  }

  return status.reason === "missing_config" || status.reason === "chatgpt";
}
