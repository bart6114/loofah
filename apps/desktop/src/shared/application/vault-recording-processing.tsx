import { Trans, useLingui } from "@lingui/react/macro";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useSyncExternalStore } from "react";

import { Button } from "@hypr/ui/components/ui/button";

import type { VaultScope } from "./vault-scope";

import type { LocalCaptureStatus } from "~/types/tauri.gen";

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
      {rows.map(({ status: value, actions }) => (
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
              onClick={() => cancellation.mutate(value.declaration.request_id)}
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
      ))}
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
