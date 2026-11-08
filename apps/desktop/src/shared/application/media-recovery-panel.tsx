import { Trans, useLingui } from "@lingui/react/macro";
import { useMutation, useQuery } from "@tanstack/react-query";
import { open as selectFile } from "@tauri-apps/plugin-dialog";
import {
  useEffect,
  useId,
  useMemo,
  useState,
  useSyncExternalStore,
} from "react";

import { Button } from "@hypr/ui/components/ui/button";

import {
  deviceFileName,
  prepareDeviceAttachmentSource,
} from "./device-attachment-source";
import { ApplicationRequestError } from "./errors";
import { mediaPending, type MediaStatus } from "./vault-client";
import type { VaultScope } from "./vault-scope";

const PAGE_SIZE = 10;
const deniedStates = [
  "closed",
  "access_denied",
  "login_required",
  "incompatible",
];

export function MediaRecoveryPanel({ scope }: { scope: VaultScope }) {
  const { t } = useLingui();
  const [expanded, setExpanded] = useState(false);
  const [page, setPage] = useState(0);
  const id = useId();
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  const connected = connection.state === "connected";
  const api = scope.client.media;
  const key = ["vault", scope.client.scope, "media_submissions"];
  const query = useQuery(
    {
      queryKey: key,
      queryFn: ({ signal }) =>
        scope.run(() => api!.pendingSubmissions!({ signal })),
      enabled: !!api?.pendingSubmissions && expanded && connected,
      networkMode: "always",
      retry: false,
      staleTime: 0,
      refetchOnWindowFocus: false,
    },
    scope.queryClient,
  );
  if (!api?.pendingSubmissions || deniedStates.includes(connection.state))
    return null;
  const receipts = query.data ?? [];
  const currentPage = Math.min(
    page,
    Math.max(0, Math.ceil(receipts.length / PAGE_SIZE) - 1),
  );
  return (
    <aside
      className="fixed right-4 bottom-4 z-40 w-80 max-w-[calc(100vw-2rem)] rounded-lg border bg-white shadow-lg"
      aria-label={t`Media recovery`}
    >
      <Button
        variant="ghost"
        className="w-full justify-between"
        aria-expanded={expanded}
        aria-controls={id}
        onClick={() => setExpanded((value) => !value)}
      >
        <Trans>Media recovery</Trans>
        <span aria-hidden>{expanded ? "−" : "+"}</span>
      </Button>
      {expanded && (
        <div
          id={id}
          className="max-h-[65vh] overflow-y-auto border-t p-3 text-sm"
        >
          <p className="mb-3 text-neutral-600">
            <Trans>
              Review saved media requests, including imports that do not have a
              note yet. Recovery starts only when you choose it.
            </Trans>
          </p>
          {!connected && (
            <p role="status">
              <Trans>
                Reconnect to the original workspace to inspect or recover
                requests.
              </Trans>
            </p>
          )}
          {query.isPending && connected && (
            <p role="status">
              <Trans>Loading saved requests…</Trans>
            </p>
          )}
          {query.error && <p role="alert">{query.error.message}</p>}
          {query.isSuccess && !receipts.length && (
            <p role="status">
              <Trans>No saved media requests.</Trans>
            </p>
          )}
          <div className="flex flex-col gap-3">
            {receipts
              .slice(currentPage * PAGE_SIZE, (currentPage + 1) * PAGE_SIZE)
              .map((receipt) => (
                <RecoveryRequest
                  key={receipt.request_id}
                  scope={scope}
                  receipt={receipt}
                  connected={connected}
                />
              ))}
          </div>
          {receipts.length > PAGE_SIZE && (
            <nav
              className="mt-3 flex items-center justify-between gap-2"
              aria-label={t`Saved media request pages`}
            >
              <Button
                size="sm"
                variant="outline"
                disabled={currentPage === 0}
                onClick={() => setPage(currentPage - 1)}
              >
                <Trans>Previous</Trans>
              </Button>
              <span>
                {currentPage + 1} / {Math.ceil(receipts.length / PAGE_SIZE)}
              </span>
              <Button
                size="sm"
                variant="outline"
                disabled={(currentPage + 1) * PAGE_SIZE >= receipts.length}
                onClick={() => setPage(currentPage + 1)}
              >
                <Trans>Next</Trans>
              </Button>
            </nav>
          )}
          <Button
            size="sm"
            variant="outline"
            className="mt-3"
            disabled={!connected || query.isFetching}
            onClick={() => void query.refetch()}
          >
            <Trans>Refresh requests</Trans>
          </Button>
        </div>
      )}
    </aside>
  );
}

function RecoveryRequest({
  scope,
  receipt,
  connected,
}: {
  scope: VaultScope;
  receipt: {
    request_id: string;
    filename: string | null;
    session_id: string | null;
  };
  connected: boolean;
}) {
  const { t } = useLingui();
  const api = scope.client.media!;
  const lifetime = useMemo(
    () => ({ active: false, abort: new AbortController() }),
    [scope, receipt.request_id],
  );
  useEffect(() => {
    const abort = new AbortController();
    lifetime.active = true;
    lifetime.abort = abort;
    return () => {
      lifetime.active = false;
      abort.abort();
    };
  }, [lifetime]);
  const key = [
    "vault",
    scope.client.scope,
    "media_recovery_status",
    receipt.request_id,
  ];
  const query = useQuery(
    {
      queryKey: key,
      queryFn: async ({ signal }) => {
        try {
          const status = await scope.run(() =>
            api.status(receipt.request_id, { signal }),
          );
          if (status.request_id !== receipt.request_id)
            throw new Error(
              t`The server returned a different media request. Keep the original request for review.`,
            );
          return status;
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
      networkMode: "always",
      retry: false,
      staleTime: 0,
      refetchOnWindowFocus: false,
      refetchInterval: (query) =>
        connected && query.state.data && mediaPending(query.state.data)
          ? 5000
          : false,
    },
    scope.queryClient,
  );
  const action = useMutation(
    {
      retry: false,
      networkMode: "always",
      mutationFn: async (kind: "recover" | "resume" | "cancel") => {
        const signal = lifetime.abort.signal;
        const current = () =>
          lifetime.active &&
          !signal.aborted &&
          scope.getSnapshot().state === "connected" &&
          scope.client.canEdit;
        if (!current())
          throw new Error(
            t`This saved request is no longer available in its original workspace.`,
          );
        const options = { signal };
        if (kind === "recover")
          return scope.run(() => api.recover(receipt.request_id, options));
        if (kind === "cancel")
          return scope.run(() => api.cancel(receipt.request_id, options));
        if (!api.resumeSource)
          throw new Error(t`This workspace cannot resume media uploads.`);
        const path = await selectFile({ multiple: false, directory: false });
        if (!path || !current()) return;
        const prepared = await prepareDeviceAttachmentSource(
          {
            kind: "path",
            path,
            name: deviceFileName(path),
          },
          options,
        );
        try {
          if (!current()) return;
          return await scope.run(() =>
            api.resumeSource!(receipt.request_id, prepared, options),
          );
        } finally {
          await prepared.close();
        }
      },
      onSuccess: (status) => {
        if (status && lifetime.active)
          scope.queryClient.setQueryData(key, status);
      },
      onSettled: () => {
        if (lifetime.active) {
          void scope.queryClient.invalidateQueries({ queryKey: key });
          void scope.queryClient.invalidateQueries({
            queryKey: ["vault", scope.client.scope, "media_submissions"],
          });
        }
      },
    },
    scope.queryClient,
  );
  const job = query.data;
  const disabled = !connected || !scope.client.canEdit || action.isPending;
  const review =
    query.isError ||
    (query.isSuccess &&
      (job === null ||
        (!!job && ["review", "failed", "cancelled"].includes(job.state))));
  return (
    <article className="rounded border p-2">
      <p className="font-medium break-words">
        {receipt.filename ?? t`Transcription request`}
      </p>
      {query.isPending && (
        <p role="status">
          <Trans>Checking the saved request…</Trans>
        </p>
      )}
      {((query.isSuccess && job === null) || query.isError) && (
        <p role="status">
          <Trans>
            The server outcome is not confirmed. Recover this saved request to
            check its original operation.
          </Trans>
        </p>
      )}
      {job && <p role="status">{statusLabel(job)}</p>}
      {!receipt.session_id && !job?.session_id && (
        <p className="text-neutral-600">
          <Trans>No note is linked to this saved request yet.</Trans>
        </p>
      )}
      {job?.error && <p>{job.error.message}</p>}
      {(query.error || action.error) && (
        <p role="alert">{(action.error ?? query.error)?.message}</p>
      )}
      <div className="mt-2 flex flex-wrap gap-2">
        {review && (
          <Button
            size="sm"
            variant="outline"
            disabled={disabled}
            onClick={() => action.mutate("recover")}
          >
            <Trans>Recover saved request</Trans>
          </Button>
        )}
        {job?.state === "receiving" && api.resumeSource && (
          <Button
            size="sm"
            variant="outline"
            disabled={disabled}
            onClick={() => action.mutate("resume")}
          >
            <Trans>Choose original file to resume upload</Trans>
          </Button>
        )}
        {job && mediaPending(job) && (
          <Button
            size="sm"
            variant="outline"
            disabled={disabled}
            onClick={() => action.mutate("cancel")}
          >
            <Trans>Cancel processing</Trans>
          </Button>
        )}
        {query.isError && (
          <Button
            size="sm"
            variant="outline"
            disabled={!connected || query.isFetching}
            onClick={() => void query.refetch()}
          >
            <Trans>Retry status</Trans>
          </Button>
        )}
      </div>
    </article>
  );
}
function statusLabel(job: MediaStatus) {
  switch (job.state) {
    case "receiving":
      return <Trans>Waiting for the original media file</Trans>;
    case "ready":
      return <Trans>Media queued</Trans>;
    case "running":
      return <Trans>Processing media</Trans>;
    case "succeeded":
      return <Trans>Media saved</Trans>;
    case "review":
      return <Trans>Media processing needs review</Trans>;
    case "failed":
      return <Trans>Media processing failed</Trans>;
    case "cancelled":
      return <Trans>Media processing cancelled</Trans>;
  }
}
