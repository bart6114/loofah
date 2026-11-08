import { Trans, useLingui } from "@lingui/react/macro";
import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useEffect, useState, useSyncExternalStore } from "react";

import { Button } from "@hypr/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@hypr/ui/components/ui/dialog";

import type {
  ProcessingPolicy,
  ProcessingSettingsSnapshot,
  SaveProcessingSettingsRequest,
} from "./protocol.gen";
import { VaultProviderCredentials } from "./vault-provider-credentials";
import type { VaultScope } from "./vault-scope";

export function VaultProcessingSettings({
  scope,
  open: controlledOpen,
  onOpenChange,
  hideTrigger = false,
}: {
  scope: VaultScope;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  hideTrigger?: boolean;
}) {
  const [internalOpen, setInternalOpen] = useState(false);
  const open = controlledOpen ?? internalOpen;
  const setOpen = onOpenChange ?? setInternalOpen;
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  const denied = [
    "access_denied",
    "login_required",
    "closed",
    "incompatible",
  ].includes(connection.state);
  useEffect(() => {
    if (denied) setOpen(false);
  }, [denied, setOpen]);
  if (!scope.client.features.includes("processing_settings") || denied)
    return null;
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      {!hideTrigger && (
        <DialogTrigger asChild>
          <Button size="sm" variant="outline">
            <Trans>Processing settings</Trans>
          </Button>
        </DialogTrigger>
      )}
      <DialogContent className="max-h-[85vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>
            <Trans>Processing settings</Trans>
          </DialogTitle>
          <DialogDescription>
            <Trans>
              These settings apply to this vault. Provider credentials are
              managed separately.
            </Trans>
          </DialogDescription>
        </DialogHeader>
        {open && (
          <SettingsLoader
            scope={scope}
            connected={connection.state === "connected"}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}

function SettingsLoader({
  scope,
  connected,
}: {
  scope: VaultScope;
  connected: boolean;
}) {
  const policy = useQuery(
    { ...scope.processingPolicyOptions(), enabled: connected },
    scope.queryClient,
  );
  const current = useQuery(
    { ...scope.processingSettingsOptions(), enabled: connected },
    scope.queryClient,
  );
  if ((!policy.data || !current.data) && (policy.error || current.error))
    return (
      <div role="alert">
        <p>{(policy.error ?? current.error)?.message}</p>
        <Button
          variant="outline"
          disabled={!connected}
          onClick={() => {
            void policy.refetch();
            void current.refetch();
          }}
        >
          <Trans>Retry</Trans>
        </Button>
      </div>
    );
  if (!policy.data || !current.data)
    return (
      <p>
        <Trans>Loading…</Trans>
      </p>
    );
  return (
    <>
      {(policy.error || current.error) && (
        <p role="alert">{(policy.error ?? current.error)?.message}</p>
      )}
      <SettingsForm
        scope={scope}
        connected={connected}
        policy={policy.data}
        current={current.data}
      />
      <VaultProviderCredentials
        scope={scope}
        policy={policy.data}
        connected={connected}
      />
    </>
  );
}

function SettingsForm({
  scope,
  connected,
  policy,
  current,
}: {
  scope: VaultScope;
  connected: boolean;
  policy: ProcessingPolicy;
  current: ProcessingSettingsSnapshot;
}) {
  const { t } = useLingui();
  const [baseline, setBaseline] = useState(current);
  const mutation = useMutation(
    {
      mutationFn: (input: SaveProcessingSettingsRequest) =>
        scope.run(() => scope.client.saveProcessingSettings(input)),
      networkMode: "always",
      retry: false,
      onSuccess: async (outcome) => {
        await scope.confirmProcessingSettings(outcome.snapshot);
      },
    },
    scope.queryClient,
  );
  const form = useForm({
    defaultValues: baseline.settings,
    listeners: { onChange: () => mutation.reset() },
    onSubmit: async ({ value }) => {
      if (!connected || !scope.client.canEdit || mutation.isPending) return;
      const outcome = await mutation.mutateAsync({
        settings: value,
        expected_revision: baseline.revision,
      });
      if (outcome.kind === "saved") {
        setBaseline(outcome.snapshot);
        form.reset(outcome.snapshot.settings);
      }
    },
  });
  const stale = baseline.revision !== current.revision;
  const disabled = !connected || !scope.client.canEdit || mutation.isPending;
  return (
    <form
      className="flex flex-col gap-4"
      onSubmit={(event) => {
        event.preventDefault();
        void form.handleSubmit().catch(() => {});
      }}
    >
      {!connected && (
        <p role="status">
          <Trans>
            Reconnect to save settings. Your changes stay in this form.
          </Trans>
        </p>
      )}
      {!scope.client.canEdit && (
        <p>
          <Trans>You have read-only access to this vault.</Trans>
        </p>
      )}
      <fieldset className="flex min-w-0 flex-col gap-4" disabled={disabled}>
        <form.Field name="summary_model">
          {(field) => {
            const selected = field.state.value;
            const provider = policy.providers.find(
              (entry) => entry.id === selected?.provider_id,
            );
            return (
              <>
                <label className="flex flex-col gap-1">
                  <Trans>Summary provider</Trans>
                  <select
                    className="min-w-0 rounded border p-2"
                    aria-label={t`Summary provider`}
                    value={selected?.provider_id ?? ""}
                    onChange={(event) => {
                      const next = policy.providers.find(
                        (entry) => entry.id === event.target.value,
                      );
                      field.handleChange(
                        next
                          ? {
                              provider_id: next.id,
                              model_id: next.models[0] ?? "",
                              base_url: next.base_url,
                            }
                          : null,
                      );
                      mutation.reset();
                    }}
                  >
                    <option value="">
                      <Trans>No summary model</Trans>
                    </option>
                    {selected && !provider && (
                      <option value={selected.provider_id}>
                        {selected.provider_id}
                      </option>
                    )}
                    {policy.providers.map((entry) => (
                      <option key={entry.id} value={entry.id}>
                        {entry.id}
                      </option>
                    ))}
                  </select>
                </label>
                {selected && provider && (
                  <>
                    <label className="flex flex-col gap-1">
                      <Trans>Summary model</Trans>
                      {provider.allow_custom_model ? (
                        <input
                          className="min-w-0 rounded border p-2"
                          aria-label={t`Summary model`}
                          value={selected.model_id}
                          maxLength={512}
                          required
                          onBlur={field.handleBlur}
                          onChange={(event) =>
                            field.handleChange({
                              ...selected,
                              model_id: event.target.value,
                            })
                          }
                        />
                      ) : (
                        <select
                          className="min-w-0 rounded border p-2"
                          aria-label={t`Summary model`}
                          value={selected.model_id}
                          required
                          onChange={(event) =>
                            field.handleChange({
                              ...selected,
                              model_id: event.target.value,
                            })
                          }
                        >
                          {!provider.models.includes(selected.model_id) && (
                            <option value={selected.model_id} disabled>
                              {selected.model_id}
                            </option>
                          )}
                          {provider.models.map((model) => (
                            <option key={model} value={model}>
                              {model}
                            </option>
                          ))}
                        </select>
                      )}
                    </label>
                    {provider.allow_custom_endpoint ? (
                      <label className="flex flex-col gap-1">
                        <Trans>Provider endpoint</Trans>
                        <input
                          className="min-w-0 rounded border p-2"
                          aria-label={t`Provider endpoint`}
                          value={selected.base_url}
                          maxLength={8192}
                          placeholder={provider.base_url}
                          onChange={(event) =>
                            field.handleChange({
                              ...selected,
                              base_url: event.target.value,
                            })
                          }
                        />
                      </label>
                    ) : (
                      provider.base_url && (
                        <p className="text-xs break-all text-neutral-600">
                          {provider.base_url}
                        </p>
                      )
                    )}
                  </>
                )}
              </>
            );
          }}
        </form.Field>
        <form.Field name="ai_language">
          {(field) => (
            <label className="flex flex-col gap-1">
              <Trans>Summary language</Trans>
              <input
                className="min-w-0 rounded border p-2"
                aria-label={t`Summary language`}
                value={field.state.value}
                maxLength={128}
                onBlur={field.handleBlur}
                onChange={(event) => field.handleChange(event.target.value)}
              />
            </label>
          )}
        </form.Field>
        <form.Field name="auto_apply_high_confidence_tags">
          {(field) => (
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={field.state.value}
                onChange={(event) => field.handleChange(event.target.checked)}
              />
              <Trans>Apply high-confidence tag suggestions</Trans>
            </label>
          )}
        </form.Field>
        <form.Field name="summary_prompt">
          {(field) =>
            policy.allow_custom_templates || field.state.value ? (
              <label className="flex flex-col gap-1">
                <Trans>Custom summary template</Trans>
                <textarea
                  className="min-h-32 min-w-0 rounded border p-2 font-mono text-sm"
                  aria-label={t`Custom summary template`}
                  value={field.state.value}
                  readOnly={!policy.allow_custom_templates}
                  maxLength={65536}
                  onBlur={field.handleBlur}
                  onChange={(event) => field.handleChange(event.target.value)}
                />
                {!policy.allow_custom_templates && (
                  <Button
                    type="button"
                    variant="outline"
                    onClick={() => field.handleChange("")}
                  >
                    <Trans>Clear unsupported template</Trans>
                  </Button>
                )}
              </label>
            ) : null
          }
        </form.Field>
      </fieldset>
      {mutation.error && <p role="alert">{mutation.error.message}</p>}
      {(stale || mutation.data?.kind === "conflict") && (
        <div className="flex flex-col gap-2" role="alert">
          <p>
            <Trans>
              Vault settings changed. Your draft has been kept. Loading current
              settings discards this draft.
            </Trans>
          </p>
          <Button
            type="button"
            variant="outline"
            disabled={!connected || mutation.isPending}
            onClick={() => {
              setBaseline(current);
              form.reset(current.settings);
              mutation.reset();
            }}
          >
            <Trans>Load current settings</Trans>
          </Button>
        </div>
      )}
      {mutation.data?.kind === "saved" && !stale && (
        <p role="status">
          <Trans>Settings saved.</Trans>
        </p>
      )}
      <Button type="submit" disabled={disabled || stale}>
        {mutation.isPending ? (
          <Trans>Saving…</Trans>
        ) : (
          <Trans>Save settings</Trans>
        )}
      </Button>
    </form>
  );
}
