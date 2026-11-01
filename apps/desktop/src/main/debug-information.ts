import type { DeviceInfo } from "@hypr/plugin-misc";
import type { DegradedError } from "@hypr/plugin-transcription";

import type { StoredSettingValues } from "~/settings/queries";

export function releaseChannel(identifier: string) {
  if (identifier.endsWith(".dev")) return "dev";
  if (identifier.endsWith(".staging")) return "staging";
  return identifier ? "stable" : "unknown";
}

export function formatDebugInformation({
  device,
  settings,
  modelDisplayName,
  languages,
  timing,
  availability,
  serverStatus,
  serverModel,
  liveMode,
  batchProvider,
  batchSupported,
  activity,
}: {
  device: DeviceInfo & { identifier: string };
  settings: StoredSettingValues;
  modelDisplayName?: string;
  languages: string[];
  timing: string;
  availability: string;
  serverStatus: string;
  serverModel?: string | null;
  liveMode?: string;
  batchProvider: string | null;
  batchSupported?: boolean;
  activity: {
    status: string;
    starting: boolean;
    liveActive: boolean | null;
    requestedLive: boolean | null;
    stalled: boolean;
    degraded: DegradedError | null;
    batchCount: number;
  };
}) {
  const { values, hasValues } = settings;
  const saved = (key: keyof typeof values) => {
    if (!hasValues.has(key)) return "Not set";
    const value = values[key];
    return value === "" ? "Empty" : String(value);
  };
  const model = values.current_stt_model;
  const provider = values.current_stt_provider;
  const modelLabel =
    modelDisplayName && model
      ? `${modelDisplayName} [${model}]`
      : saved("current_stt_model");
  const engine =
    batchProvider === "whispercpp"
      ? "Whisper (whisper.cpp)"
      : batchProvider === "soniqo"
        ? "Soniqo"
        : "None / unsupported";
  const target = `${provider || "None"} / ${model || "None"}`;
  const ready = availability === "Downloaded" && serverStatus === "ready";
  const reason =
    !provider || !model
      ? "No transcription provider/model selected"
      : !batchProvider
        ? "Selected provider/model is unsupported"
        : !ready
          ? `Local model ${availability.toLowerCase()}; server ${serverStatus}`
          : timing === "batch"
            ? "Configured to transcribe after recording"
            : liveMode === "batch"
              ? "Selected model/languages do not support live transcription; transcribe after recording"
              : liveMode === "live"
                ? "None"
                : "Checking live support";
  const liveStatus = activity.starting
    ? "Starting capture"
    : activity.status === "finalizing"
      ? "Finalizing capture"
      : activity.status !== "active"
        ? "No live transcription active"
        : activity.liveActive === true
          ? "Live transcription active"
          : activity.liveActive === false
            ? "Recording only; no live transcription active"
            : "Live transcription status unknown";
  const fallbackReason = activity.degraded
    ? {
        authentication_failed: "Authentication failed",
        upstream_unavailable: "Transcription service unavailable",
        connection_timeout: "Connection timed out",
        stream_error: "Transcription stream failed",
      }[activity.degraded.type]
    : activity.requestedLive && activity.liveActive === false
      ? "Live transcription unavailable; recording only (reason unavailable)"
      : "None";

  return [
    "Loofah debug information",
    "",
    `App version: ${device.appVersion}`,
    `Build hash: ${device.buildHash || "Unknown"}`,
    `Release channel: ${releaseChannel(device.identifier)}`,
    `macOS: ${device.osVersion || "Unknown"}`,
    `Hardware architecture: ${device.arch}`,
    "",
    "Saved settings",
    `Transcription provider: ${saved("current_stt_provider")}`,
    `Transcription model: ${modelLabel}`,
    `Transcription timing: ${saved("transcription_timing")}`,
    `Meeting languages: ${saved("meeting_languages")}`,
    `Spoken languages: ${saved("spoken_languages")}`,
    `Summary language: ${saved("ai_language")}`,
    `Intelligence provider: ${saved("current_llm_provider")}`,
    `Intelligence model: ${saved("current_llm_model")}`,
    "",
    "Resolved from current settings",
    `Transcription engine: ${engine}`,
    `Transcription target: ${target}`,
    `Transcription timing: ${timing}`,
    `Transcription languages: ${languages.join(", ") || "Automatic detection"}`,
    `Local model availability: ${availability}`,
    `Local server: ${serverStatus}`,
    `Local server model: ${serverModel || "None"}`,
    `Live target: ${ready && liveMode === "live" ? target : "None"}`,
    `Batch target: ${ready && batchProvider && batchSupported === true ? `${batchProvider} / ${model}` : "None"}`,
    `Batch language support: ${batchSupported === undefined ? "Unknown" : batchSupported ? "Supported" : "Unsupported"}`,
    `Fallback / mode reason: ${reason}`,
    `Intelligence: ${values.current_llm_provider && values.current_llm_model ? `${values.current_llm_provider} / ${values.current_llm_model}` : "Disabled / setup incomplete"}`,
    "",
    "Current activity",
    `Live: ${liveStatus}`,
    `Batch: ${activity.batchCount > 0 ? `${activity.batchCount} active transcription(s)` : "No batch transcription active"}`,
    `Live fallback: ${fallbackReason}`,
    `Live transcription stalled: ${activity.stalled ? "Yes; batch repair needed" : "No"}`,
  ].join("\n");
}
