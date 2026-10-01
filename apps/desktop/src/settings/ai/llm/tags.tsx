import { Trans } from "@lingui/react/macro";
import { useForm } from "@tanstack/react-form";
import { useMutation } from "@tanstack/react-query";
import { useEffect, useId } from "react";

import { Switch } from "@hypr/ui/components/ui/switch";

import { setSettingValue } from "~/settings/queries";
import { useConfigValue } from "~/shared/config";

export function TagSettings() {
  const enabled = useConfigValue("auto_apply_high_confidence_tags");
  const titleId = useId();
  const descriptionId = useId();
  const save = useMutation({
    mutationFn: (value: boolean) =>
      setSettingValue("auto_apply_high_confidence_tags", value),
  });
  const form = useForm({
    defaultValues: { enabled },
    onSubmit: async ({ value }) => {
      await save.mutateAsync(value.enabled);
    },
  });

  useEffect(() => {
    form.reset({ enabled });
  }, [enabled, form]);

  return (
    <section className="rounded-2xl border p-4">
      <div className="flex items-center justify-between gap-4">
        <div className="flex-1">
          <h3 id={titleId} className="text-sm font-medium">
            <Trans>Auto-apply tags with high confidence</Trans>
          </h3>
          <p id={descriptionId} className="text-muted-foreground mt-1 text-xs">
            <Trans>
              Automatically attach tags when the summary model's confidence is
              above 85%. Other tags stay as suggestions. Turn this off to accept
              or dismiss every suggestion manually. Applies to future summaries.
            </Trans>
          </p>
        </div>
        <form.Field name="enabled">
          {(field) => (
            <Switch
              checked={field.state.value}
              disabled={save.isPending}
              aria-labelledby={titleId}
              aria-describedby={descriptionId}
              onCheckedChange={(value) => {
                field.handleChange(value);
                void form.handleSubmit().catch(() => form.reset({ enabled }));
              }}
            />
          )}
        </form.Field>
      </div>
      {save.error && (
        <p role="alert" className="text-destructive mt-2 text-xs">
          {save.error.message}
        </p>
      )}
    </section>
  );
}
