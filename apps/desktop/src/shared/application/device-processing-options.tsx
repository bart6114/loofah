import { Trans } from "@lingui/react/macro";
import { useForm } from "@tanstack/react-form";

import type { CaptureProcessingOptions } from "~/types/tauri.gen";

export function useDeviceProcessingOptions() {
  return useForm({
    defaultValues: { transcribe: false, summary: false },
  });
}

export function captureProcessingOptions(values: {
  transcribe: boolean;
  summary: boolean;
}): CaptureProcessingOptions | null {
  if (!values.transcribe) return null;
  return {
    model: null,
    languages: [],
    keywords: [],
    summary: values.summary
      ? {
          target: { kind: "summary" },
          title_policy: "generate_if_empty",
        }
      : null,
  };
}

export function DeviceProcessingOptions({
  form,
  disabled,
}: {
  form: ReturnType<typeof useDeviceProcessingOptions>;
  disabled: boolean;
}) {
  return (
    <fieldset disabled={disabled} className="flex flex-col gap-1 text-xs">
      <form.Field name="transcribe">
        {(field) => (
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={field.state.value}
              onChange={(event) => field.handleChange(event.target.checked)}
              onBlur={field.handleBlur}
            />
            <Trans>Transcribe on the server</Trans>
          </label>
        )}
      </form.Field>
      <form.Subscribe selector={(state) => state.values.transcribe}>
        {(transcribe) => (
          <form.Field name="summary">
            {(field) => (
              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  disabled={!transcribe}
                  checked={field.state.value}
                  onChange={(event) => field.handleChange(event.target.checked)}
                  onBlur={field.handleBlur}
                />
                <Trans>Summarize when transcription finishes</Trans>
              </label>
            )}
          </form.Field>
        )}
      </form.Subscribe>
    </fieldset>
  );
}
