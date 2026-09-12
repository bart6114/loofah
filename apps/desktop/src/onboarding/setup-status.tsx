import { Trans } from "@lingui/react/macro";
import { useQuery } from "@tanstack/react-query";
import { CheckIcon, CircleIcon, Loader2Icon } from "lucide-react";

import type { LocalModel } from "@hypr/plugin-local-stt";

import { sttModelQueries } from "~/settings/ai/stt/shared";
import { useConfigValues } from "~/shared/config";
import { isFmtrLocalSttModel } from "~/stt/capabilities";

export function TranscriptionSetupStatus() {
  const { current_stt_provider, current_stt_model } = useConfigValues([
    "current_stt_provider",
    "current_stt_model",
  ] as const);
  const selected = isFmtrLocalSttModel(current_stt_provider, current_stt_model);
  const downloaded = useQuery({
    ...sttModelQueries.isDownloaded(current_stt_model as LocalModel),
    enabled: selected,
  });
  const downloading = useQuery({
    ...sttModelQueries.isDownloading(current_stt_model as LocalModel),
    enabled: selected && downloaded.data !== true,
  });
  const pending =
    selected &&
    (downloaded.isPending ||
      (downloaded.data !== true && downloading.isPending));

  return (
    <span role="status" className="flex items-center gap-2">
      {downloaded.data === true ? (
        <>
          <CheckIcon className="text-brand size-4" />
          <Trans>Transcription model downloaded</Trans>
        </>
      ) : pending || downloading.data === true ? (
        <>
          <Loader2Icon className="size-4 animate-spin" />
          {pending ? (
            <Trans>Checking transcription download…</Trans>
          ) : (
            <Trans>Transcription model downloading</Trans>
          )}
        </>
      ) : (
        <>
          <CircleIcon className="size-4" />
          <Trans>Transcription not set up</Trans>
        </>
      )}
    </span>
  );
}

export function SummarySetupStatus() {
  const { current_llm_provider, current_llm_model } = useConfigValues([
    "current_llm_provider",
    "current_llm_model",
  ] as const);
  const selected = !!(current_llm_provider && current_llm_model);

  return (
    <span role="status" className="flex items-center gap-2">
      {selected ? (
        <>
          <CheckIcon className="text-brand size-4" />
          <Trans>Summary service selected</Trans>
        </>
      ) : (
        <>
          <CircleIcon className="size-4" />
          <Trans>Summaries not enabled</Trans>
        </>
      )}
    </span>
  );
}
