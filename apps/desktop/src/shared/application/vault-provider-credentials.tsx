import { Trans } from "@lingui/react/macro";
import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useMemo } from "react";

import { Button } from "@hypr/ui/components/ui/button";

import type {
  ProcessingPolicy,
  SaveProviderCredentialRequest,
} from "./protocol.gen";
import type { VaultScope } from "./vault-scope";

export function VaultProviderCredentials({
  scope,
  policy,
  connected,
}: {
  scope: VaultScope;
  policy: ProcessingPolicy;
  connected: boolean;
}) {
  const instance = useMemo(() => crypto.randomUUID(), [scope]);
  if (!scope.client.providerCredentials) return null;
  return (
    <section className="flex flex-col gap-4" aria-label="Provider credentials">
      <h3 className="font-medium">
        <Trans>Provider credentials</Trans>
      </h3>
      <p className="text-muted-foreground text-sm">
        <Trans>
          Keys are stored on the server for this vault and are never shown
          again. Clearing a vault key restores the server's configured key, when
          available.
        </Trans>
      </p>
      {policy.providers.map((provider) => (
        <ProviderCredentialForm
          key={`${instance}:${provider.id}`}
          scope={scope}
          providerId={provider.id}
          connected={connected}
        />
      ))}
    </section>
  );
}

function ProviderCredentialForm({
  scope,
  providerId,
  connected,
}: {
  scope: VaultScope;
  providerId: string;
  connected: boolean;
}) {
  const api = scope.client.providerCredentials!;
  const queryKey = [
    "vault",
    scope.client.scope,
    "provider_credential",
    providerId,
  ];
  const status = useQuery(
    {
      queryKey,
      queryFn: ({ signal }) =>
        scope.run(() => api.status(providerId, { signal })),
      enabled: connected,
      retry: false,
      refetchOnMount: "always",
    },
    scope.queryClient,
  );
  const save = useMutation(
    {
      mutationFn: (request: {
        owner: VaultScope;
        api: NonNullable<VaultScope["client"]["providerCredentials"]>;
        providerId: string;
        queryKey: readonly unknown[];
        input: SaveProviderCredentialRequest;
      }) =>
        request.owner.run(() =>
          request.api.save(
            request.providerId,
            { ...request.input },
            {
              requestId: crypto.randomUUID(),
            },
          ),
        ),
      networkMode: "always",
      retry: false,
      gcTime: 0,
      onSettled: async (_result, _error, request) => {
        request.input.api_key = null;
        await request.owner.queryClient.invalidateQueries({
          queryKey: request.queryKey,
          exact: true,
        });
      },
    },
    scope.queryClient,
  );
  const form = useForm({
    defaultValues: { apiKey: "" },
    onSubmit: async ({ value }) => {
      if (!connected || save.isPending || !status.data || !value.apiKey) return;
      const input = {
        expected_revision: status.data.revision,
        api_key: value.apiKey,
      };
      form.reset();
      await save.mutateAsync({
        owner: scope,
        api,
        providerId,
        queryKey,
        input,
      });
    },
  });
  const disabled =
    !connected || save.isPending || !status.isSuccess || status.isFetching;
  return (
    <form
      className="flex flex-col gap-2 rounded border p-3"
      aria-label={`${providerId} credential`}
      onSubmit={(event) => {
        event.preventDefault();
        void form.handleSubmit().catch(() => {});
      }}
    >
      <span className="text-sm font-medium">{providerId}</span>
      {status.data && (
        <p role="status" className="text-sm">
          {!status.data.endpoint_matches ? (
            <Trans>
              The provider endpoint changed. Its previous vault key is inactive.
            </Trans>
          ) : status.data.source === "vault" ? (
            <Trans>Using a vault key.</Trans>
          ) : status.data.source === "operator" ? (
            <Trans>Using the server's configured key.</Trans>
          ) : (
            <Trans>No provider key is configured.</Trans>
          )}
        </p>
      )}
      <fieldset disabled={disabled} className="flex flex-col gap-2">
        <form.Field name="apiKey">
          {(field) => (
            <label className="flex flex-col gap-1 text-sm">
              <Trans>New API key</Trans>
              <input
                type="password"
                autoComplete="new-password"
                spellCheck={false}
                maxLength={16384}
                value={field.state.value}
                onBlur={field.handleBlur}
                onChange={(event) => field.handleChange(event.target.value)}
                className="rounded border p-2"
              />
            </label>
          )}
        </form.Field>
        <div className="flex gap-2">
          <Button type="submit" size="sm" variant="outline">
            <Trans>Save vault key</Trans>
          </Button>
          <Button
            type="button"
            size="sm"
            variant="outline"
            onClick={() => {
              form.reset();
              if (status.data)
                save.mutate({
                  owner: scope,
                  api,
                  providerId,
                  queryKey,
                  input: {
                    expected_revision: status.data.revision,
                    api_key: null,
                  },
                });
            }}
          >
            <Trans>Clear vault key</Trans>
          </Button>
        </div>
      </fieldset>
      {save.data?.kind === "conflict" && (
        <p role="alert">
          <Trans>
            The credential changed. Review its status before saving another key.
          </Trans>
        </p>
      )}
      {save.data?.kind === "saved" && !status.isFetching && !status.error && (
        <p role="status">
          <Trans>Credential update confirmed.</Trans>
        </p>
      )}
      {(status.error || save.error) && (
        <div role="alert">
          <p>{(status.error ?? save.error)?.message}</p>
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={!connected || status.isFetching || save.isPending}
            onClick={() => void status.refetch()}
          >
            <Trans>Refresh credential status</Trans>
          </Button>
        </div>
      )}
    </form>
  );
}
