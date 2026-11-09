import { Trans, useLingui } from "@lingui/react/macro";
import { useMutation, useQuery } from "@tanstack/react-query";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { open as selectFile } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useSyncExternalStore } from "react";

import { Button } from "@hypr/ui/components/ui/button";

import { deviceFileName } from "./device-attachment-source";
import { ApplicationRequestError } from "./errors";
import type { ProcessingJobState } from "./protocol.gen";
import { mediaPending, type MediaStatus } from "./vault-client";
import type { VaultScope } from "./vault-scope";

export function VaultMediaProcessing({
  scope,
  sessionId,
}: {
  scope: VaultScope;
  sessionId: string;
}) {
  const { t } = useLingui();
  const api = scope.client.media!;
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
    return () => {
      lifetime.active = false;
      lifetime.abort.abort();
    };
  }, [lifetime]);
  const key = ["vault", scope.client.scope, "media_jobs", sessionId];
  const query = useQuery(
    {
      queryKey: key,
      queryFn: ({ signal }) => scope.run(() => api.list(sessionId, { signal })),
      enabled: connected,
      staleTime: 0,
      refetchInterval: (query) =>
        query.state.data?.some((job) => mediaPending(job)) ? 1000 : 15000,
    },
    scope.queryClient,
  );
  const action = useMutation(
    {
      retry: false,
      networkMode: "always",
      mutationFn: async ({
        kind,
        id,
      }: {
        kind: "cancel" | "recover" | "copy" | "resume";
        id: string;
      }) => {
        if (
          !lifetime.active ||
          scope.getSnapshot().state !== "connected" ||
          (kind !== "copy" && !scope.client.canEdit)
        )
          throw new Error(t`This media request is no longer available.`);
        const options = { signal: lifetime.abort.signal };
        if (kind === "cancel") return scope.run(() => api.cancel(id, options));
        if (kind === "recover")
          return scope.run(() => api.recover(id, options));
        if (kind === "resume" && api.resumeSource) {
          const path = await selectFile({ multiple: false, directory: false });
          if (!path) return;
          if (!lifetime.active || scope.getSnapshot().state !== "connected")
            return;
          const prepared = await scope.prepareAttachmentSource({
            kind: "path",
            path,
            name: deviceFileName(path),
          });
          try {
            if (!lifetime.active || scope.getSnapshot().state !== "connected")
              return;
            return await scope.run(() =>
              api.resumeSource!(id, prepared, options),
            );
          } finally {
            await prepared.close();
          }
        }
        const result = await scope.run(() => api.result(id, options));
        if (
          !lifetime.active ||
          lifetime.abort.signal.aborted ||
          scope.getSnapshot().state !== "connected"
        )
          return;
        if (!result)
          throw new Error(
            t`No retained transcript is available for this request.`,
          );
        await writeText(
          (result.words ?? []).map((word) => word.text).join(" "),
        );
      },
      onSettled: () => scope.queryClient.invalidateQueries({ queryKey: key }),
    },
    scope.queryClient,
  );
  if (denied) return null;
  const rows =
    query.data?.filter((value) => value.session_id === sessionId) ?? [];
  if (!rows.length && !query.error) return null;
  const disabled = !connected || action.isPending;
  return (
    <section
      className="flex flex-col gap-2 border-t p-3"
      aria-label={t`Media processing`}
    >
      {rows.map((job) => (
        <div key={job.request_id} className="flex flex-col gap-1 text-sm">
          <p role="status">{mediaStatus(job)}</p>
          {job.error && <p>{job.error.message}</p>}
          <div className="flex flex-wrap gap-2">
            {job.state === "receiving" && api.resumeSource && (
              <Button
                size="sm"
                variant="outline"
                disabled={disabled || !scope.client.canEdit}
                onClick={() =>
                  action.mutate({ kind: "resume", id: job.request_id })
                }
              >
                <Trans>Choose original file to resume upload</Trans>
              </Button>
            )}
            {mediaPending(job) && (
              <Button
                size="sm"
                variant="outline"
                disabled={disabled || !scope.client.canEdit}
                onClick={() =>
                  action.mutate({ kind: "cancel", id: job.request_id })
                }
              >
                <Trans>Cancel processing</Trans>
              </Button>
            )}
            {["review", "failed", "cancelled"].includes(job.state) && (
              <>
                <Button
                  size="sm"
                  variant="outline"
                  disabled={disabled || !scope.client.canEdit}
                  onClick={() =>
                    action.mutate({ kind: "recover", id: job.request_id })
                  }
                >
                  <Trans>Recover saved request</Trans>
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  disabled={disabled}
                  onClick={() =>
                    action.mutate({ kind: "copy", id: job.request_id })
                  }
                >
                  <Trans>Copy retained transcript</Trans>
                </Button>
              </>
            )}
          </div>
          {!mediaPending(job) && (
            <MediaSummary
              scope={scope}
              requestId={job.request_id}
              connected={connected}
            />
          )}
        </div>
      ))}
      {action.isSuccess && action.variables.kind === "copy" && (
        <p role="status">
          <Trans>Copied</Trans>
        </p>
      )}
      {(query.error || action.error) && (
        <p role="alert">{(action.error ?? query.error)?.message}</p>
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
function MediaSummary({
  scope,
  requestId,
  connected,
}: {
  scope: VaultScope;
  requestId: string;
  connected: boolean;
}) {
  const query = useQuery(
    {
      queryKey: ["vault", scope.client.scope, "media_summary", requestId],
      queryFn: async ({ signal }) => {
        try {
          return await scope.run(() =>
            scope.client.media!.summaryStatus(requestId, { signal }),
          );
        } catch (cause) {
          if (
            cause instanceof ApplicationRequestError &&
            cause.detail.code === "not_found"
          )
            return null;
          throw cause;
        }
      },
      enabled: connected,
      retry: false,
      staleTime: 0,
      refetchInterval: (query) =>
        query.state.data &&
        (query.state.data.state === "pending_media" ||
          query.state.data.state === "source_bound" ||
          (query.state.data.job &&
            ["queued", "running", "cancelling"].includes(
              query.state.data.job.state,
            )))
          ? 15000
          : false,
    },
    scope.queryClient,
  );
  const status = query.data;
  return (
    <>
      {status?.job && <p role="status">{summaryStatus(status.job.state)}</p>}
      {status?.reason && <p>{status.reason}</p>}
      {query.error && <p role="alert">{query.error.message}</p>}
    </>
  );
}
function mediaStatus(job: MediaStatus) {
  switch (job.state) {
    case "receiving":
      return <Trans>Waiting for the original media upload</Trans>;
    case "ready":
      return <Trans>Media queued</Trans>;
    case "running":
      return <Trans>Processing media</Trans>;
    case "succeeded":
      return <Trans>Media saved</Trans>;
    case "failed":
      return <Trans>Media processing failed</Trans>;
    case "review":
      return <Trans>Media processing needs review</Trans>;
    case "cancelled":
      return <Trans>Media processing cancelled</Trans>;
  }
}

function summaryStatus(state: ProcessingJobState) {
  switch (state) {
    case "queued":
      return <Trans>Summary queued</Trans>;
    case "running":
      return <Trans>Creating summary</Trans>;
    case "cancelling":
      return <Trans>Cancelling summary</Trans>;
    case "cancelled":
      return <Trans>Summary cancelled</Trans>;
    case "succeeded":
      return <Trans>Summary saved</Trans>;
    case "failed":
      return <Trans>Summary generation failed</Trans>;
    case "interrupted":
      return <Trans>Summary generation was interrupted</Trans>;
    case "conflict":
      return (
        <Trans>The summary changed. Generated content was not applied.</Trans>
      );
  }
}
