import { Trans } from "@lingui/react/macro";
import { useForm } from "@tanstack/react-form";
import { useMutation } from "@tanstack/react-query";

import { CORE_TRANSCRIPTION_LANGUAGE_CODES } from "./language";
import { MainLanguageView } from "./main-language";
import { SpokenLanguagesView } from "./spoken-languages";

import { setSettingValues } from "~/settings/queries";
import { useConfigValues } from "~/shared/config";
import { getTranscriptionLanguages } from "~/stt/capabilities";

function useLanguageSettings() {
  const config = useConfigValues([
    "ai_language",
    "spoken_languages",
    "meeting_languages",
  ] as const);
  const meetingLanguages = getTranscriptionLanguages(
    config.ai_language,
    config.spoken_languages,
    config.meeting_languages,
  );
  const save = useMutation({ mutationFn: setSettingValues });
  return { config, meetingLanguages, save };
}

export function SummaryLanguageSettings() {
  const { config, meetingLanguages, save } = useLanguageSettings();
  const form = useForm({
    defaultValues: { language: config.ai_language },
    onSubmit: async ({ value }) => {
      await save.mutateAsync({
        ai_language: value.language,
        meeting_languages: JSON.stringify(meetingLanguages),
      });
    },
  });
  return (
    <div className="flex flex-col gap-2">
      <form.Field name="language">
        {(field) => (
          <MainLanguageView
            value={field.state.value}
            onChange={(value) => {
              field.handleChange(value);
              void form.handleSubmit().catch(() => {});
            }}
            supportedLanguages={CORE_TRANSCRIPTION_LANGUAGE_CODES}
          />
        )}
      </form.Field>
      {save.error && (
        <p role="alert" className="text-destructive text-sm">
          {save.error.message}
        </p>
      )}
    </div>
  );
}

export function MeetingLanguageSettings() {
  const { meetingLanguages, save } = useLanguageSettings();
  const form = useForm({
    defaultValues: { languages: meetingLanguages },
    onSubmit: async ({ value }) => {
      if (value.languages.length === 0) return;
      await save.mutateAsync({
        meeting_languages: JSON.stringify(value.languages),
      });
    },
  });
  return (
    <div className="flex flex-col gap-2">
      <form.Field name="languages">
        {(field) => (
          <SpokenLanguagesView
            value={field.state.value}
            onChange={(value) => {
              field.handleChange(value);
              void form.handleSubmit().catch(() => {});
            }}
            supportedLanguages={CORE_TRANSCRIPTION_LANGUAGE_CODES}
          />
        )}
      </form.Field>
      {save.error && (
        <p role="alert" className="text-destructive text-sm">
          {save.error.message}
        </p>
      )}
      {meetingLanguages.length === 0 && (
        <p className="text-muted-foreground text-xs">
          <Trans>
            Add a meeting language to help transcription choose the right words.
          </Trans>
        </p>
      )}
    </div>
  );
}
