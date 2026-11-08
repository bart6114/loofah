import { Trans, useLingui } from "@lingui/react/macro";
import { useInfiniteQuery, useMutation, useQuery } from "@tanstack/react-query";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { useEffect, useMemo, useSyncExternalStore } from "react";

import { Button } from "@hypr/ui/components/ui/button";

import type { TranscriptionJobSnapshot } from "./protocol.gen";
import {
  activeTranscription,
  TranscriptionSubmissions,
} from "./transcription-submissions";
import type { VaultScope } from "./vault-scope";

export function VaultTranscription({
  scope,
  sessionId,
  hasTranscript,
}: {
  scope: VaultScope;
  sessionId: string;
  hasTranscript: boolean;
}) {
  const { t } = useLingui();
  const controller = useMemo(
    () => new TranscriptionSubmissions(scope),
    [scope],
  );
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
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  const connected = connection.state === "connected";
  const denied = [
    "closed",
    "access_denied",
    "login_required",
    "incompatible",
  ].includes(connection.state);
  const api = scope.client.transcriptionJobs!;
  const key = ["transcription", scope.client.scope, sessionId];
  const receipts = useQuery(
    {
      queryKey: [...key, "receipts"],
      queryFn: () => controller.store.list(sessionId),
      enabled: !denied,
      networkMode: "always",
      retry: false,
      staleTime: 0,
    },
    scope.queryClient,
  );
  const acknowledged = useQuery(
    {
      queryKey: [
        ...key,
        "acknowledged",
        receipts.data
          ?.filter((receipt) => receipt.job && activeTranscription(receipt.job))
          .map((receipt) => receipt.request_id) ?? [],
      ],
      queryFn: async ({ signal }) =>
        Promise.all(
          (receipts.data ?? [])
            .filter(
              (receipt) => receipt.job && activeTranscription(receipt.job),
            )
            .map(async (receipt) => {
              const job = await scope.run(() =>
                api.get(receipt.job!.id, { signal }),
              );
              await controller.store.observe(receipt.request_id, job);
              return job;
            }),
        ),
      enabled: connected && !!receipts.data,
      staleTime: 0,
      refetchInterval: (query) =>
        query.state.data?.some(activeTranscription) ? 15_000 : false,
    },
    scope.queryClient,
  );
  const recording = useQuery(
    {
      ...scope.audioMetadataOptions(sessionId),
      enabled: connected && !hasTranscript,
    },
    scope.queryClient,
  );
  const policy = useQuery(
    {
      queryKey: ["transcription", scope.client.scope, "policy"],
      queryFn: ({ signal }) => scope.run(() => api.policy({ signal })),
      enabled: connected,
      staleTime: 0,
      refetchInterval: 15_000,
    },
    scope.queryClient,
  );
  const history = useInfiniteQuery(
    {
      queryKey: [...key, "history"],
      initialPageParam: null as string | null,
      queryFn: ({ pageParam, signal }) =>
        scope.run(() => api.list({ limit: 50, after: pageParam }, { signal })),
      getNextPageParam: (page) => page.next_cursor ?? undefined,
      enabled: connected,
      staleTime: 0,
      refetchInterval: (query) =>
        query.state.data?.pages.some((page) =>
          page.jobs.some(activeTranscription),
        )
          ? 15_000
          : false,
    },
    scope.queryClient,
  );
  const refresh = () =>
    scope.queryClient.invalidateQueries({
      queryKey: ["transcription", scope.client.scope],
    });
  const action = useMutation(
    {
      retry: false,
      networkMode: "always",
      mutationFn: async (
        operation:
          | { kind: "start" }
          | { kind: "resume"; requestId: string }
          | { kind: "cancel" | "result" | "copy"; jobId: string },
      ) => {
        if (
          !lifetime.active ||
          !connected ||
          scope.getSnapshot().state !== "connected" ||
          (operation.kind !== "copy" && !scope.client.canEdit)
        )
          throw new Error(
            "This connection is no longer available for transcription",
          );
        const options = { signal: lifetime.abort.signal };
        switch (operation.kind) {
          case "start":
            return controller.transcribe(sessionId, options);
          case "resume":
            return controller.resume(operation.requestId, options);
          case "cancel":
            return scope.run(() => api.cancel(operation.jobId, options));
          case "result":
            return scope.run(() => api.resumeResult(operation.jobId, options));
          case "copy": {
            const result = await scope.run(() =>
              api.result(operation.jobId, options),
            );
            if (
              !lifetime.active ||
              lifetime.abort.signal.aborted ||
              scope.getSnapshot().state !== "connected"
            )
              return;
            if (!result)
              throw new Error("The saved transcription result is unavailable");
            await writeText(
              (result.transcript.words ?? [])
                .map((word) => word.text)
                .join(" "),
            );
            return;
          }
        }
      },
      onSettled: refresh,
    },
    scope.queryClient,
  );
  if (denied) return null;
  const rows = new Map(
    history.data?.pages
      .flatMap((page) => page.jobs)
      .filter((job) => job.session_id === sessionId)
      .map((job) => [job.id, job]),
  );
  for (const job of acknowledged.data ?? []) rows.set(job.id, job);
  for (const receipt of receipts.data ?? []) {
    if (receipt.job && !rows.has(receipt.job.id))
      rows.set(receipt.job.id, receipt.job);
  }
  const blocked = receipts.data?.some(
    (receipt) =>
      !receipt.job ||
      (rows.get(receipt.job.id) ?? receipt.job).state === "uncertain" ||
      activeTranscription(rows.get(receipt.job.id) ?? receipt.job),
  );
  const disabled = !connected || !scope.client.canEdit || action.isPending;
  return (
    <section
      className="flex flex-col gap-2 border-t p-3"
      aria-label={t`Transcription jobs`}
    >
      {!hasTranscript && (recording.isPending || !!recording.data) && (
        <Button
          size="sm"
          disabled={
            disabled ||
            !recording.data ||
            blocked ||
            receipts.isPending ||
            receipts.isError ||
            history.isPending ||
            history.isError ||
            !policy.data?.batch_enabled ||
            policy.data.availability !== "ready" ||
            [...rows.values()].some(activeTranscription)
          }
          onClick={() => action.mutate({ kind: "start" })}
        >
          <Trans>Transcribe saved recording</Trans>
        </Button>
      )}
      {!hasTranscript &&
        recording.data &&
        policy.data &&
        (!policy.data.batch_enabled ||
          policy.data.availability !== "ready") && (
          <p role="status">
            <Trans>
              Transcription is currently unavailable for this connection.
            </Trans>
          </p>
        )}
      {receipts.data
        ?.filter((receipt) => !receipt.job)
        .map((receipt) => (
          <div
            key={receipt.request_id}
            className="flex items-center gap-2 text-sm"
          >
            <p>
              <Trans>
                The previous transcription request was not confirmed. Recover it
                before starting another.
              </Trans>
            </p>
            <Button
              size="sm"
              variant="outline"
              disabled={disabled}
              onClick={() =>
                action.mutate({ kind: "resume", requestId: receipt.request_id })
              }
            >
              <Trans>Recover request</Trans>
            </Button>
          </div>
        ))}
      {[...rows.values()].map((job) => (
        <div key={job.id} className="flex items-center gap-2 text-sm">
          <p role="status">{transcriptionStatus(job)}</p>
          {activeTranscription(job) && (
            <Button
              size="sm"
              variant="outline"
              disabled={disabled || job.state === "cancelling"}
              onClick={() => action.mutate({ kind: "cancel", jobId: job.id })}
            >
              <Trans>Cancel transcription</Trans>
            </Button>
          )}
          {job.result_available &&
            !activeTranscription(job) &&
            !["succeeded", "empty"].includes(job.state) && (
              <Button
                size="sm"
                variant="outline"
                disabled={disabled}
                onClick={() => action.mutate({ kind: "result", jobId: job.id })}
              >
                <Trans>Recover saved result</Trans>
              </Button>
            )}
          {job.result_available && (
            <Button
              size="sm"
              variant="outline"
              disabled={!connected || action.isPending}
              onClick={() => action.mutate({ kind: "copy", jobId: job.id })}
            >
              <Trans>Copy saved transcript</Trans>
            </Button>
          )}
        </div>
      ))}
      {action.isSuccess && action.variables.kind === "copy" && (
        <p role="status">
          <Trans>Copied</Trans>
        </p>
      )}
      {history.hasNextPage && (
        <Button
          size="sm"
          variant="outline"
          disabled={!connected || history.isFetchingNextPage}
          onClick={() => void history.fetchNextPage()}
        >
          <Trans>Load more transcription jobs</Trans>
        </Button>
      )}
      {(receipts.error ||
        acknowledged.error ||
        history.error ||
        policy.error ||
        action.error) && (
        <p role="alert">
          {
            (
              action.error ??
              recording.error ??
              receipts.error ??
              acknowledged.error ??
              history.error ??
              policy.error
            )?.message
          }
        </p>
      )}
      {(recording.isError ||
        receipts.isError ||
        acknowledged.isError ||
        history.isError ||
        policy.isError) && (
        <Button
          size="sm"
          variant="outline"
          disabled={!connected}
          onClick={() => {
            void refresh();
            if (recording.isError) void recording.refetch();
          }}
        >
          <Trans>Retry</Trans>
        </Button>
      )}
    </section>
  );
}

function transcriptionStatus(job: TranscriptionJobSnapshot) {
  switch (job.state) {
    case "queued":
      return <Trans>Transcription queued</Trans>;
    case "running":
      if (job.phase === "preparing") return <Trans>Preparing recording</Trans>;
      if (job.phase === "publishing") return <Trans>Saving transcript</Trans>;
      return <Trans>Transcribing recording</Trans>;
    case "cancelling":
      return <Trans>Cancelling transcription</Trans>;
    case "cancelled":
      return <Trans>Transcription cancelled</Trans>;
    case "succeeded":
      return <Trans>Transcript saved</Trans>;
    case "failed":
      return <Trans>Transcription failed. Your recording is kept.</Trans>;
    case "interrupted":
      return (
        <Trans>Transcription was interrupted. Review the saved request.</Trans>
      );
    case "conflict":
      return (
        <Trans>
          The recording or transcript changed. Generated content did not replace
          your changes.
        </Trans>
      );
    case "empty":
      return <Trans>No speech was found. Your recording is kept.</Trans>;
    case "uncertain":
      return (
        <Trans>
          The transcription outcome needs review. Your recording is kept.
        </Trans>
      );
  }
}
