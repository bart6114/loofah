import { Trans } from "@lingui/react/macro";
import { useMutation, useQuery } from "@tanstack/react-query";
import { CheckIcon, CopyIcon } from "lucide-react";
import { useEffect } from "react";

import type { DeviceInfo } from "@hypr/plugin-misc";
import { Button } from "@hypr/ui/components/ui/button";

import { formatDebugInformation } from "./debug-information";

import { useStoredSettingValuesQuery } from "~/settings/queries";
import { useConfigValues } from "~/shared/config";
import {
  getLiveTranscriptionConfig,
  getTranscriptionLanguages,
  isFmtrLocalSttModel,
  isSupportedLanguagesBatch,
} from "~/stt/capabilities";
import { useListener } from "~/stt/contexts";
import { localSttQueries } from "~/stt/useLocalSttModel";
import { getBatchProvider } from "~/stt/useRunBatch";

export function DebugSection({
  device,
  deviceError,
}: {
  device: (DeviceInfo & { identifier: string }) | undefined;
  deviceError: boolean;
}) {
  const stored = useStoredSettingValuesQuery();
  const settings = useConfigValues([
    "current_stt_provider",
    "current_stt_model",
    "transcription_timing",
    "meeting_languages",
  ] as const);
  const { current_stt_provider: provider, current_stt_model: model } = settings;
  const localModel = isFmtrLocalSttModel(provider, model) ? model : null;
  const supportedModels = useQuery({
    ...localSttQueries.supportedModels(),
    networkMode: "always",
    retry: false,
  });
  const modelInfo = supportedModels.data?.find((entry) => entry.key === model);
  const downloaded = useQuery({
    ...localSttQueries.isDownloaded(localModel!),
    enabled: !!localModel,
    networkMode: "always",
    retry: false,
  });
  const downloading = useQuery({
    ...localSttQueries.isDownloading(localModel!),
    enabled: !!localModel,
    refetchInterval: 1000,
    networkMode: "always",
    retry: false,
  });
  const server = useQuery({
    ...localSttQueries.server(localModel),
    networkMode: "always",
    retry: false,
  });
  const languages = getTranscriptionLanguages(settings.meeting_languages);
  const batchProvider =
    provider && model ? getBatchProvider(provider, model) : null;
  const support = useQuery({
    queryKey: [
      "about",
      "transcription-support",
      provider,
      model,
      languages,
      settings.transcription_timing,
    ],
    queryFn: async () => {
      const [live, batch] = await Promise.all([
        getLiveTranscriptionConfig({
          provider,
          model,
          languages,
          timing: settings.transcription_timing,
        }),
        batchProvider
          ? isSupportedLanguagesBatch(batchProvider, model, languages)
          : Promise.resolve(false),
      ]);
      return { liveMode: live.transcriptionMode, batchSupported: batch };
    },
    networkMode: "always",
    retry: false,
  });
  const activity = useListener((state) => ({
    status: state.live.status,
    starting: state.live.status === "inactive" && state.live.loading,
    liveActive: state.live.liveTranscriptionActive,
    requestedLive: state.live.requestedLiveTranscription,
    stalled: state.live.transcriptionStalled,
    degraded: state.live.degraded,
    batchCount: Object.values(state.batch).filter(
      (batch) => !batch.isComplete && !batch.terminalReason,
    ).length,
  }));
  const text =
    device && stored.data
      ? formatDebugInformation({
          device,
          settings: stored.data,
          modelDisplayName: modelInfo?.display_name,
          languages,
          timing: settings.transcription_timing,
          availability: !localModel
            ? "Not applicable"
            : downloaded.isError || downloading.isError
              ? "Unknown"
              : downloaded.data
                ? "Downloaded"
                : downloading.data
                  ? "Downloading"
                  : downloaded.data === false
                    ? "Not downloaded"
                    : "Checking",
          serverStatus: !localModel
            ? "Not configured"
            : server.isError
              ? "Unknown"
              : (server.data?.status ?? "Checking"),
          serverModel: server.data?.model,
          liveMode: support.data?.liveMode,
          batchProvider,
          batchSupported: support.data?.batchSupported,
          activity,
        })
      : null;
  const copy = useMutation({
    mutationFn: async () => {
      if (text) await navigator.clipboard.writeText(text);
    },
    networkMode: "always",
  });
  const { reset } = copy;
  useEffect(() => {
    if (!copy.isSuccess) return;
    const timer = setTimeout(reset, 1500);
    return () => clearTimeout(timer);
  }, [copy.isSuccess, reset]);

  return (
    <div className="flex flex-col gap-3 border-t px-6 py-5">
      <p className="text-muted-foreground text-xs">
        <Trans>
          Preview the settings and diagnostics to include in a bug report. API
          keys, tokens, notes, and transcripts are excluded.
        </Trans>
      </p>
      {text ? (
        <pre
          aria-label="Debug information"
          className="bg-muted max-h-72 overflow-auto rounded-lg p-3 font-mono text-[11px] leading-relaxed break-words whitespace-pre-wrap select-text"
        >
          {text}
        </pre>
      ) : (
        <p className="text-muted-foreground text-xs" role="status">
          {stored.error || deviceError ? (
            <Trans>Couldn’t load debug information.</Trans>
          ) : (
            <Trans>Loading debug information…</Trans>
          )}
        </p>
      )}
      <Button
        variant="outline"
        size="sm"
        disabled={!text || copy.isPending}
        onClick={() => copy.mutate()}
      >
        {copy.isSuccess ? (
          <CheckIcon className="size-3.5" />
        ) : (
          <CopyIcon className="size-3.5" />
        )}
        {copy.isSuccess ? (
          <Trans>Copied</Trans>
        ) : (
          <Trans>Copy debug information</Trans>
        )}
      </Button>
      {copy.isError && (
        <p role="alert" className="text-destructive text-xs">
          <Trans>Couldn’t copy debug information. Try again.</Trans>
        </p>
      )}
    </div>
  );
}
