import { Trans, useLingui } from "@lingui/react/macro";
import { useQuery } from "@tanstack/react-query";
import { AlertTriangle } from "lucide-react";

import { cn } from "@hypr/utils";

import { sttModelQueries } from "./shared";

import {
  getBaseLanguageCode,
  getBaseLanguageDisplayName,
} from "~/settings/general/language";
import { useConfigValues } from "~/shared/config";
import {
  getTranscriptionLanguages,
  isFmtrLocalSttModel,
} from "~/stt/capabilities";

export function TranscriptionModelLanguageSupport() {
  const { i18n } = useLingui();
  const config = useConfigValues([
    "current_stt_provider",
    "current_stt_model",
    "meeting_languages",
  ] as const);
  const models = useQuery(sttModelQueries.supportedModels());
  if (
    !isFmtrLocalSttModel(config.current_stt_provider, config.current_stt_model)
  ) {
    return null;
  }
  const selected = models.data?.find(
    (model) => model.key === config.current_stt_model,
  );
  if (!selected?.supported_languages) return null;

  const supported = new Set(
    selected.supported_languages.map(getBaseLanguageCode),
  );
  const unsupported = getTranscriptionLanguages(config.meeting_languages)
    .filter((language) => !supported.has(getBaseLanguageCode(language)))
    .map((language) => getBaseLanguageDisplayName(language, i18n.locale))
    .join(", ");
  const languages = [...supported]
    .map((language) => getBaseLanguageDisplayName(language, i18n.locale))
    .join(", ");
  const model = selected.display_name;

  return (
    <div
      role={unsupported ? "alert" : undefined}
      className={cn([
        "flex items-start gap-2 rounded-lg p-3 text-xs",
        unsupported
          ? "bg-amber-50 text-amber-900 dark:bg-amber-950 dark:text-amber-200"
          : "bg-muted text-muted-foreground",
      ])}
    >
      {unsupported && (
        <AlertTriangle className="size-4 shrink-0" aria-hidden="true" />
      )}
      <div className="flex flex-col gap-1">
        {unsupported && (
          <p>
            <Trans>
              {model} can't transcribe {unsupported}. Choose another model or
              change your spoken languages.
            </Trans>
          </p>
        )}
        <p>
          <Trans>
            Supported languages for {model}: {languages}.
          </Trans>
        </p>
      </div>
    </div>
  );
}
