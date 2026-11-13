import { Trans } from "@lingui/react/macro";
import { useForm } from "@tanstack/react-form";
import { useMutation } from "@tanstack/react-query";

import { type StoredSettingValues, setSettingValues } from "~/settings/queries";
import { resolveConfigValues } from "~/shared/config";

export function DictationSettings({
  storedSettings,
}: {
  storedSettings: StoredSettingValues;
}) {
  const values = resolveConfigValues(
    ["dictation_shortcut", "dictation_trigger_mode"] as const,
    storedSettings,
  );
  const save = useMutation({
    mutationFn: (value: typeof values) => setSettingValues(value),
  });
  const form = useForm({
    defaultValues: values,
    listeners: {
      onChange: ({ formApi }) => {
        void formApi.handleSubmit();
      },
    },
    onSubmit: ({ value }) => save.mutate(value),
  });

  return (
    <section className="space-y-3">
      <h3 className="text-sm font-medium">
        <Trans>Dictation</Trans>
      </h3>
      <p className="text-muted-foreground text-xs">
        <Trans>
          Speak into a focused text field. Loofah inserts the transcription when
          you stop.
        </Trans>
      </p>
      <form.Field name="dictation_shortcut">
        {(field) => (
          <label className="flex items-center gap-3 text-sm">
            <Trans>Shortcut</Trans>
            <input
              readOnly
              value={field.state.value}
              className="rounded border px-2 py-1"
              onKeyDown={(event) => {
                event.preventDefault();
                const key =
                  event.code === "Space"
                    ? "Space"
                    : event.code.replace(/^Key/, "").replace(/^Digit/, "");
                if (!/^(Space|[A-Z0-9])$/.test(key)) return;
                const modifiers = [
                  event.ctrlKey && "Ctrl",
                  event.altKey &&
                    (navigator.platform.includes("Mac") ? "Option" : "Alt"),
                  event.shiftKey && "Shift",
                  event.metaKey && "Cmd",
                ].filter(Boolean);
                if (modifiers.length < 2) return;
                field.handleChange([...modifiers, key].join("+"));
              }}
            />
          </label>
        )}
      </form.Field>
      <form.Field name="dictation_trigger_mode">
        {(field) => (
          <label className="flex items-center gap-3 text-sm">
            <Trans>Trigger</Trans>
            <select
              value={field.state.value}
              onChange={(event) => {
                field.handleChange(event.target.value);
              }}
              className="rounded border px-2 py-1"
            >
              <option value="hold">
                <Trans>Hold to speak</Trans>
              </option>
              <option value="toggle">
                <Trans>Press to start, press again to stop</Trans>
              </option>
            </select>
          </label>
        )}
      </form.Field>
      {save.isError && (
        <p role="alert" className="text-destructive text-xs">
          <Trans>Could not save dictation settings.</Trans>
        </p>
      )}
    </section>
  );
}
