import { Trans, useLingui } from "@lingui/react/macro";
import { useForm } from "@tanstack/react-form";
import { useMutation } from "@tanstack/react-query";
import { useEffect, useMemo, useState, useSyncExternalStore } from "react";

import { Button } from "@hypr/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@hypr/ui/components/ui/dialog";

import type {
  MediaRetirementRequest,
  MediaSourceDisposition,
} from "./protocol.gen";
import type { VaultScope } from "./vault-scope";

import type { MediaRetirementView } from "~/types/tauri.gen";

export function MediaRetirementReview(props: {
  scope: VaultScope;
  requestId: string;
  view: MediaRetirementView;
  onUpdated: (view: MediaRetirementView) => void;
  onClose: () => void;
}) {
  const { t } = useLingui();
  const [original] = useState(() => ({
    scope: props.scope,
    requestId: props.requestId,
    view: structuredClone(props.view),
    id: props.view.pending?.retirement_id ?? crypto.randomUUID(),
  }));
  const { scope, view, requestId } = original;
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  const lifetime = useMemo(
    () => ({ active: false, abort: new AbortController() }),
    [],
  );
  useEffect(() => {
    lifetime.active = true;
    lifetime.abort = new AbortController();
    return () => {
      lifetime.active = false;
      lifetime.abort.abort();
    };
  }, [lifetime]);
  const current = () =>
    lifetime.active &&
    !lifetime.abort.signal.aborted &&
    props.scope === scope &&
    props.requestId === requestId &&
    scope.getSnapshot().state === "connected" &&
    scope.client.canEdit;
  const mutation = useMutation(
    {
      networkMode: "always",
      retry: false,
      mutationFn: async (input: MediaRetirementRequest) => {
        if (!current() || !scope.client.media?.retire)
          throw new Error(
            t`Reconnect to the original workspace before removing retained media.`,
          );
        return scope.run(() =>
          scope.client.media!.retire!(requestId, input, {
            signal: lifetime.abort.signal,
          }),
        );
      },
      onSuccess: (result) => {
        if (!current()) return;
        props.onUpdated(result);
        if (result.status.state === "retired") props.onClose();
      },
      onSettled: () => {
        if (lifetime.active) {
          void scope.queryClient.invalidateQueries({
            queryKey: ["vault", scope.client.scope, "media_submissions"],
          });
          void scope.queryClient.invalidateQueries({
            queryKey: [
              "vault",
              scope.client.scope,
              "media_retirement",
              requestId,
            ],
          });
        }
      },
    },
    scope.queryClient,
  );
  const [decision, setDecision] = useState<MediaRetirementRequest | null>(
    view.pending,
  );
  const form = useForm({
    defaultValues: {
      disposition:
        view.pending?.disposition ?? ("" as MediaSourceDisposition | ""),
      confirmed: false,
    },
    onSubmit: async ({ value }) => {
      if (!value.confirmed || !value.disposition || !current()) return;
      const input = decision ?? {
        retirement_id: original.id,
        expected_revision: view.status.revision,
        expected_declaration_sha256: view.status.declaration_sha256,
        expected_source: view.status.source,
        disposition: value.disposition,
      };
      setDecision(input);
      mutation.mutate(input);
    },
  });
  const choices = view.pending
    ? [view.pending.disposition]
    : view.status.allowed_dispositions;
  const available =
    connection.state === "connected" &&
    props.scope === scope &&
    props.requestId === requestId &&
    scope.client.canEdit &&
    !!scope.client.media?.retire;
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !mutation.isPending) props.onClose();
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>
            <Trans>Review retained upload and request quota</Trans>
          </DialogTitle>
          <DialogDescription>
            <Trans>
              This removes only this request's retained upload, if any, and
              releases its upload and request quota. Canonical notes,
              recordings, transcripts, processing copies and retained results
              stay unchanged.
            </Trans>
          </DialogDescription>
        </DialogHeader>
        {view.status.source && (
          <p>
            {view.status.source.size_bytes.toLocaleString()}{" "}
            <Trans>bytes in the original source</Trans>
          </p>
        )}
        {view.pending && (
          <p role="status">
            <Trans>
              This continues the saved cleanup decision with the same request
              ID.
            </Trans>
          </p>
        )}
        <form
          onSubmit={(event) => {
            event.preventDefault();
            event.stopPropagation();
            void form.handleSubmit();
          }}
          className="flex flex-col gap-3"
        >
          <form.Field name="disposition">
            {(field) => (
              <label>
                <Trans>Original source</Trans>
                <select
                  aria-label={t`Original source`}
                  value={field.state.value}
                  disabled={!!decision || mutation.isPending || !available}
                  onChange={(event) =>
                    field.handleChange(
                      event.target.value as MediaSourceDisposition | "",
                    )
                  }
                >
                  <option value="">
                    <Trans>Choose how to handle the source</Trans>
                  </option>
                  {choices.includes("exported") && (
                    <option value="exported" disabled={!view.verified_export}>
                      <Trans>I have kept the verified exported copy</Trans>
                    </option>
                  )}
                  {choices.includes("discard") && (
                    <option value="discard">
                      <Trans>Discard the retained original upload</Trans>
                    </option>
                  )}
                  {choices.includes("no_upload") && (
                    <option value="no_upload">
                      <Trans>No uploaded source; release request quota</Trans>
                    </option>
                  )}
                </select>
              </label>
            )}
          </form.Field>
          {choices.includes("exported") && !view.verified_export && (
            <p>
              <Trans>
                Use Save original file first. Only an export verified by this
                app can be selected as an exported copy.
              </Trans>
            </p>
          )}
          {view.verified_export && (
            <p>
              <Trans>
                The app verified the copy when it was saved. Confirm that you
                have kept it; the app cannot guarantee it still exists.
              </Trans>
            </p>
          )}
          <form.Field name="confirmed">
            {(field) => (
              <label className="flex gap-2">
                <input
                  type="checkbox"
                  checked={field.state.value}
                  disabled={!available || mutation.isPending}
                  onChange={(event) => field.handleChange(event.target.checked)}
                />
                {view.status.source ? (
                  <Trans>
                    I understand that removing this retained upload may delete
                    its only original copy.
                  </Trans>
                ) : (
                  <Trans>
                    I confirm releasing this request's active quota.
                  </Trans>
                )}
              </label>
            )}
          </form.Field>
          {mutation.error && <p role="alert">{mutation.error.message}</p>}
          {!available && (
            <p role="status">
              <Trans>
                The original workspace is unavailable. Nothing will be removed
                automatically.
              </Trans>
            </p>
          )}
          <form.Subscribe
            selector={(state) => [
              state.values.confirmed,
              state.values.disposition,
            ]}
          >
            {([confirmed, disposition]) => (
              <Button
                type="submit"
                disabled={
                  !available ||
                  !confirmed ||
                  !disposition ||
                  mutation.isPending ||
                  (disposition === "exported" && !view.verified_export)
                }
              >
                {decision ? (
                  <Trans>Resume original cleanup</Trans>
                ) : view.status.source ? (
                  <Trans>Remove retained upload</Trans>
                ) : (
                  <Trans>Release request quota</Trans>
                )}
              </Button>
            )}
          </form.Subscribe>
        </form>
      </DialogContent>
    </Dialog>
  );
}
