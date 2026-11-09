import { Trans, useLingui } from "@lingui/react/macro";
import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useState, useSyncExternalStore } from "react";

import { Button } from "@hypr/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@hypr/ui/components/ui/dialog";

import { ApplicationRequestError } from "./errors";
import type { VaultScope } from "./vault-scope";

import type {
  LocalCaptureControlSnapshot,
  LocalCaptureResolution,
  LocalCaptureStatus,
} from "~/types/tauri.gen";

export function VaultRecordingProcessing({
  scope,
  sessionId,
}: {
  scope: VaultScope;
  sessionId: string;
}) {
  const { t } = useLingui();
  const api = scope.client.recordingProcessing!;
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  const connected = connection.state === "connected";
  const denied = [
    "closed",
    "access_denied",
    "login_required",
    "incompatible",
  ].includes(connection.state);
  const lifetime = useMemo(
    () => ({ active: false, abort: new AbortController() }),
    [scope, sessionId],
  );
  useEffect(() => {
    lifetime.active = true;
    lifetime.abort = new AbortController();
    return () => {
      lifetime.active = false;
      lifetime.abort.abort();
    };
  }, [lifetime]);
  const [review, setReview] = useState<{
    scope: VaultScope;
    sessionId: string;
    snapshot: LocalCaptureControlSnapshot;
  } | null>(null);
  useEffect(() => setReview(null), [lifetime]);
  const key = ["recording_processing", scope.client.scope, sessionId];
  const query = useQuery(
    {
      queryKey: key,
      queryFn: ({ signal }) =>
        scope.run(() => api.controls(sessionId, { signal })),
      enabled: connected,
      staleTime: 0,
      refetchInterval: 15_000,
    },
    scope.queryClient,
  );
  const recovery = useMutation(
    {
      retry: false,
      networkMode: "always",
      mutationFn: (requestId: string) => {
        if (
          !lifetime.active ||
          !scope.client.canEdit ||
          scope.getSnapshot().state !== "connected"
        )
          throw new Error("This recording is no longer available for recovery");
        return scope.run(() =>
          api.recover(requestId, { signal: lifetime.abort.signal }),
        );
      },
      onSettled: () => scope.queryClient.invalidateQueries({ queryKey: key }),
    },
    scope.queryClient,
  );
  const cancellation = useMutation(
    {
      retry: false,
      networkMode: "always",
      mutationFn: (requestId: string) => {
        if (
          !lifetime.active ||
          !scope.client.canEdit ||
          scope.getSnapshot().state !== "connected"
        )
          throw new Error(
            t`This recording is no longer available for cancellation`,
          );
        return scope.run(() =>
          api.cancel(requestId, { signal: lifetime.abort.signal }),
        );
      },
      onSettled: () => scope.queryClient.invalidateQueries({ queryKey: key }),
    },
    scope.queryClient,
  );
  if (denied) return null;
  const rows =
    query.data?.filter(
      (value) => value.status.declaration.session_id === sessionId,
    ) ?? [];
  if (!rows.length && !query.error) return null;
  return (
    <section
      className="flex flex-col gap-2 border-t p-3"
      aria-label={t`Recording processing`}
    >
      {rows.map((snapshot) => {
        const { status: value, actions } = snapshot;
        return (
          <div
            key={value.declaration.request_id}
            className="flex flex-col gap-1 text-sm"
          >
            <p role="status">{recordingStatus(value)}</p>
            {value.message && <p>{value.message}</p>}
            {actions.includes("cancel") && (
              <Button
                size="sm"
                variant="outline"
                disabled={
                  !connected ||
                  !scope.client.canEdit ||
                  recovery.isPending ||
                  cancellation.isPending
                }
                onClick={() =>
                  cancellation.mutate(value.declaration.request_id)
                }
              >
                <Trans>Cancel processing</Trans>
              </Button>
            )}
            {value.state === "cancelling" && (
              <p>
                <Trans>
                  Waiting for processing resources. Saved recording and content
                  will be kept.
                </Trans>
              </p>
            )}
            {actions.includes("keep_current_content") && (
              <Button
                size="sm"
                variant="outline"
                disabled={
                  !connected ||
                  !scope.client.canEdit ||
                  recovery.isPending ||
                  cancellation.isPending
                }
                onClick={() =>
                  setReview({
                    scope,
                    sessionId,
                    snapshot: structuredClone(snapshot),
                  })
                }
              >
                <Trans>Review processing conflict</Trans>
              </Button>
            )}
            {actions.includes("recover") && (
              <Button
                size="sm"
                variant="outline"
                disabled={
                  !connected ||
                  !scope.client.canEdit ||
                  recovery.isPending ||
                  cancellation.isPending
                }
                onClick={() => recovery.mutate(value.declaration.request_id)}
              >
                <Trans>Recover recording processing</Trans>
              </Button>
            )}
          </div>
        );
      })}
      {review?.scope === scope && review.sessionId === sessionId && (
        <RecordingResolutionReview
          key={`${scope.client.scope}:${review.snapshot.status.declaration.request_id}:${review.snapshot.revision}`}
          scope={scope}
          snapshot={review.snapshot}
          onClose={() => setReview(null)}
          onUpdated={() => {
            void scope.queryClient.invalidateQueries({ queryKey: key });
            setReview(null);
          }}
        />
      )}
      {(query.error || recovery.error || cancellation.error) && (
        <p role="alert">
          {(cancellation.error ?? recovery.error ?? query.error)?.message}
        </p>
      )}
      {query.isError && (
        <Button
          size="sm"
          variant="outline"
          disabled={!connected}
          onClick={() => void query.refetch()}
        >
          <Trans>Retry</Trans>
        </Button>
      )}
    </section>
  );
}
function recordingStatus(value: LocalCaptureStatus) {
  switch (value.state) {
    case "capturing":
      return <Trans>Recording</Trans>;
    case "waiting_for_persistence":
      return <Trans>Saving recording</Trans>;
    case "transcribing":
      return <Trans>Transcribing recording</Trans>;
    case "summarizing":
      return <Trans>Creating summary</Trans>;
    case "cancelling":
      return <Trans>Cancelling recording processing</Trans>;
    case "cancelled":
      return <Trans>Recording processing cancelled</Trans>;
    case "closed":
      return <Trans>Recording processing closed by review</Trans>;
    case "succeeded":
      return <Trans>Recording processing complete</Trans>;
    case "skipped":
      return <Trans>Recording processing skipped</Trans>;
    case "not_started":
      return <Trans>Recording did not start</Trans>;
    case "review":
      return <Trans>Recording processing needs review</Trans>;
    case "conflict":
      return (
        <Trans>
          Recording processing stopped because the saved content changed
        </Trans>
      );
  }
}

function RecordingResolutionReview({
  scope,
  snapshot,
  onClose,
  onUpdated,
}: {
  scope: VaultScope;
  snapshot: LocalCaptureControlSnapshot;
  onClose: () => void;
  onUpdated: () => void;
}) {
  const { t } = useLingui();
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  const [original] = useState(() => ({
    scope,
    requestId: snapshot.status.declaration.request_id,
    input: {
      resolution_id: crypto.randomUUID(),
      expected_revision: snapshot.revision,
      decision: "keep_current_content",
    } satisfies LocalCaptureResolution,
  }));
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
    scope === original.scope &&
    scope.client.canEdit &&
    scope.getSnapshot().state === "connected";
  const mutation = useMutation(
    {
      retry: false,
      networkMode: "always",
      mutationFn: async () => {
        if (!current())
          throw new Error(
            t`This processing request is unavailable in its original workspace.`,
          );
        const result = await scope.run(() =>
          scope.client.recordingProcessing!.resolve(
            original.requestId,
            original.input,
            { signal: lifetime.abort.signal },
          ),
        );
        if (
          result.status.declaration.request_id !== original.requestId ||
          result.status.state !== "closed" ||
          result.resolution?.resolution_id !== original.input.resolution_id ||
          result.resolution.expected_revision !==
            original.input.expected_revision ||
          result.resolution.decision !== original.input.decision
        )
          throw new Error(t`The original resolution has not been confirmed.`);
        return result;
      },
      onSuccess: () => {
        if (current()) onUpdated();
      },
    },
    scope.queryClient,
  );
  const stale =
    mutation.error instanceof ApplicationRequestError &&
    mutation.error.detail.code === "conflict" &&
    mutation.error.detail.mutation_outcome === "not_committed" &&
    mutation.error.detail.current_revision !== null;
  const available =
    connection.state === "connected" &&
    scope.client.canEdit &&
    scope === original.scope;
  const form = useForm({
    defaultValues: { confirmed: false },
    onSubmit: async ({ value }) => {
      if (!value.confirmed || !current() || stale || mutation.isPending) return;
      mutation.mutate();
    },
  });
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !mutation.isPending) onClose();
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>
            <Trans>Keep current content and close processing?</Trans>
          </DialogTitle>
          <DialogDescription>
            <Trans>
              This closes only this recording's completed processing conflict.
              Current notes, images, transcript, summary, audio, retained
              results and journals stay unchanged. No transcription or summary
              will run.
            </Trans>
          </DialogDescription>
        </DialogHeader>
        {snapshot.status.message && <p>{snapshot.status.message}</p>}
        <form
          onSubmit={(event) => {
            event.preventDefault();
            event.stopPropagation();
            void form.handleSubmit();
          }}
          className="flex flex-col gap-3"
        >
          <form.Field name="confirmed">
            {(field) => (
              <label className="flex items-start gap-2">
                <input
                  type="checkbox"
                  checked={field.state.value}
                  disabled={mutation.isPending || !available || stale}
                  onChange={(event) => field.handleChange(event.target.checked)}
                />
                <Trans>
                  Keep the current saved content and close this processing
                  request.
                </Trans>
              </label>
            )}
          </form.Field>
          {mutation.error && <p role="alert">{mutation.error.message}</p>}
          {stale ? (
            <p>
              <Trans>
                The processing state changed. Refresh and review it before
                making another decision.
              </Trans>
            </p>
          ) : (
            mutation.isError && (
              <p>
                <Trans>
                  The outcome is not confirmed. Retrying uses the same reviewed
                  decision and request ID.
                </Trans>
              </p>
            )
          )}
          <form.Subscribe selector={(state) => state.values.confirmed}>
            {(confirmed) => (
              <Button
                type="submit"
                disabled={
                  !confirmed || !available || mutation.isPending || stale
                }
              >
                {mutation.isError ? (
                  <Trans>Retry original resolution</Trans>
                ) : (
                  <Trans>Keep current content and close processing</Trans>
                )}
              </Button>
            )}
          </form.Subscribe>
          {stale && (
            <Button
              type="button"
              variant="outline"
              onClick={onUpdated}
              disabled={!available}
            >
              <Trans>Refresh and review again</Trans>
            </Button>
          )}
          <Button
            type="button"
            variant="outline"
            onClick={onClose}
            disabled={mutation.isPending}
          >
            <Trans>Close review</Trans>
          </Button>
        </form>
      </DialogContent>
    </Dialog>
  );
}
