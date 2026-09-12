import { Trans } from "@lingui/react/macro";
import { useMutation } from "@tanstack/react-query";
import { useId } from "react";

import { cn } from "@hypr/utils";

import { setSettingValues } from "~/settings/queries";
import { useConfigValues } from "~/shared/config";
import {
  getOnDeviceTranscriptionMode,
  getTranscriptionLanguages,
  isFmtrLocalSttModel,
  isRealtimeLocalModel,
} from "~/stt/capabilities";

export function TranscriptionTiming() {
  const config = useConfigValues([
    "current_stt_provider",
    "current_stt_model",
    "transcription_timing",
    "ai_language",
    "spoken_languages",
    "meeting_languages",
  ] as const);
  const save = useMutation({ mutationFn: setSettingValues });
  const groupId = useId();
  const languages = getTranscriptionLanguages(
    config.ai_language,
    config.spoken_languages,
    config.meeting_languages,
  );
  const configured = isFmtrLocalSttModel(
    config.current_stt_provider,
    config.current_stt_model,
  );
  const liveModel =
    configured && isRealtimeLocalModel(config.current_stt_model);
  const canStream =
    configured &&
    getOnDeviceTranscriptionMode(config.current_stt_model, languages) ===
      "live";
  const timing =
    canStream && config.transcription_timing !== "batch" ? "live" : "batch";

  return (
    <fieldset className="flex flex-col gap-3" disabled={save.isPending}>
      <legend className="mb-3 text-sm font-medium">
        <Trans>Transcription timing</Trans>
      </legend>
      {(
        [
          {
            value: "live",
            title: <Trans>While recording</Trans>,
            description: <Trans>Show the transcript as you speak.</Trans>,
            disabled: !canStream,
          },
          {
            value: "batch",
            title: <Trans>After recording</Trans>,
            description: <Trans>Transcribe when the recording ends.</Trans>,
            disabled: false,
          },
        ] as const
      ).map((option) => (
        <label
          key={option.value}
          className={cn([
            "flex items-start gap-3 rounded-lg border p-3",
            timing === option.value && "border-primary",
            option.disabled
              ? "cursor-not-allowed opacity-50"
              : "cursor-pointer",
          ])}
        >
          <input
            type="radio"
            name={groupId}
            value={option.value}
            checked={timing === option.value}
            disabled={option.disabled}
            aria-describedby={`${groupId}-${option.value}${option.disabled ? ` ${groupId}-unavailable` : ""}`}
            onChange={() => save.mutate({ transcription_timing: option.value })}
            onClick={() => {
              // A fallback already checks batch; clicking it must still save that preference.
              if (
                option.value === timing &&
                config.transcription_timing !== timing
              ) {
                save.mutate({ transcription_timing: option.value });
              }
            }}
            className="accent-primary mt-1"
          />
          <span className="flex flex-col gap-1">
            <span className="text-sm font-medium">{option.title}</span>
            <span
              id={`${groupId}-${option.value}`}
              className="text-muted-foreground text-xs"
            >
              {option.description}
            </span>
          </span>
        </label>
      ))}
      {!canStream && (
        <p
          id={`${groupId}-unavailable`}
          className="text-muted-foreground text-xs"
        >
          {!configured ? (
            <Trans>
              Select a transcription model to check live availability.
            </Trans>
          ) : !liveModel ? (
            <Trans>This model transcribes after recording.</Trans>
          ) : (
            <Trans>
              This model transcribes after recording for the selected meeting
              languages. Live transcription supports English only.
            </Trans>
          )}
        </p>
      )}
      {save.error && (
        <p role="alert" className="text-destructive text-sm">
          {save.error.message}
        </p>
      )}
    </fieldset>
  );
}
