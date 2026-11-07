import { Trans, useLingui } from "@lingui/react/macro";
import { useInfiniteQuery, useMutation, useQuery } from "@tanstack/react-query";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { useEffect, useMemo, useState, useSyncExternalStore } from "react";

import { Button } from "@hypr/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@hypr/ui/components/ui/dialog";

import { activeJob, JobSubmissions } from "./job-submissions";
import type { DocumentTarget, ProcessingJobSnapshot } from "./protocol.gen";
import { VaultProcessingSettings } from "./vault-processing-settings";
import type { VaultScope } from "./vault-scope";

import type { JobSubmissionReceipt } from "~/types/tauri.gen";

export function VaultProcessing({
  scope,
  sessionId,
  target = { kind: "summary" },
}: {
  scope: VaultScope;
  sessionId: string;
  target?: DocumentTarget;
}) {
  const [host, setHost] = useState(scope);
  useEffect(() => setHost(scope), [scope]);
  if (host !== scope) return null;
  if (!scope.client.features.includes("summary_body_jobs")) return null;
  if (
    target.kind === "template_output" &&
    !scope.client.features.includes("template_output_jobs")
  )
    return null;
  return (
    <SummaryJobs
      key={JSON.stringify([sessionId, target])}
      scope={scope}
      sessionId={sessionId}
      target={target}
    />
  );
}

const sameTarget = (a: DocumentTarget, b: DocumentTarget) =>
  a.kind === b.kind &&
  (a.kind === "summary" ||
    (b.kind === "template_output" && a.document_id === b.document_id));

function refresh(controller: JobSubmissions) {
  const { scope } = controller;
  void scope.queryClient.invalidateQueries({
    queryKey: ["device_job_submissions", scope.client.scope],
  });
  void scope.queryClient.invalidateQueries({
    queryKey: ["vault", scope.client.scope, "processing_jobs"],
  });
}

function useReceipts(
  controller: JobSubmissions,
  sessionId?: string,
  enabled = true,
) {
  return useQuery(
    {
      queryKey: [
        "device_job_submissions",
        controller.scope.client.scope,
        sessionId ?? null,
      ],
      queryFn: () => controller.store.list(sessionId),
      networkMode: "always",
      retry: false,
      staleTime: 0,
      enabled,
    },
    controller.scope.queryClient,
  );
}

function SummaryJobs({
  scope,
  sessionId,
  target,
}: {
  scope: VaultScope;
  sessionId: string;
  target: DocumentTarget;
}) {
  const { t } = useLingui();
  const controller = useMemo(() => new JobSubmissions(scope), [scope]);
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  const connected = connection.state === "connected";
  const denied = [
    "access_denied",
    "login_required",
    "closed",
    "incompatible",
  ].includes(connection.state);
  const receipts = useReceipts(controller, sessionId, !denied);
  const active = useQuery(
    {
      ...scope.processingJobsOptions({
        session_id: sessionId,
        active_only: true,
        after: null,
        limit: 200,
      }),
      enabled: connected,
      refetchInterval: (query) =>
        query.state.data?.items.length ? 15_000 : false,
    },
    scope.queryClient,
  );
  const history = useInfiniteQuery(
    {
      queryKey: [
        "vault",
        scope.client.scope,
        "processing_jobs",
        sessionId,
        "history",
        target,
      ],
      initialPageParam: null as string | null,
      queryFn: ({ pageParam, signal }) =>
        scope.run(() =>
          scope.client.processingJobs(
            {
              session_id: sessionId,
              active_only: false,
              limit: 20,
              after: pageParam,
            },
            { signal },
          ),
        ),
      getNextPageParam: (page) => page.next_cursor ?? undefined,
      refetchInterval: (query) =>
        query.state.data?.pages.some((page) => page.items.some(activeJob))
          ? 15_000
          : false,
      enabled: connected,
    },
    scope.queryClient,
  );
  const generate = useMutation(
    {
      mutationFn: () => controller.generateDocument(sessionId, target),
      networkMode: "always",
      retry: false,
      onSettled: () => refresh(controller),
    },
    scope.queryClient,
  );
  const rows = useMemo(() => {
    const merged = new Map<string, ProcessingJobSnapshot>();
    for (const job of [
      ...(history.data?.pages.flatMap((page) => page.items) ?? []),
      ...(active.data?.items ?? []),
    ]) {
      if (!sameTarget(job.target, target)) continue;
      const previous = merged.get(job.id);
      if (!previous || activeJob(previous)) merged.set(job.id, job);
    }
    return Array.from(merged.values());
  }, [active.data, history.data, target]);
  const known = new Map(rows.map((job) => [job.id, job]));
  const targetReceipts = receipts.data?.filter((receipt) =>
    sameTarget(receipt.input.target, target),
  );
  const pending = targetReceipts?.some(
    (receipt) =>
      !receipt.job || activeJob(known.get(receipt.job.id) ?? receipt.job),
  );
  if (denied) return null;
  return (
    <section
      className="flex flex-col gap-2 rounded border p-3"
      aria-label={t`Summary generation`}
    >
      <div className="flex flex-wrap items-center gap-2">
        <VaultProcessingSettings scope={scope} />
        {(target.kind === "summary" || scope.client.canEdit) && (
          <Button
            size="sm"
            disabled={
              !connected ||
              !scope.client.canEdit ||
              generate.isPending ||
              pending ||
              receipts.isPending ||
              receipts.isError ||
              active.isPending ||
              active.isError ||
              active.data?.items.some(
                (job) =>
                  sameTarget(job.target, target) &&
                  activeJob(known.get(job.id) ?? job),
              )
            }
            onClick={() => generate.mutate()}
          >
            {generate.isPending ? (
              <Trans>Starting summary…</Trans>
            ) : target.kind === "template_output" ? (
              <Trans>Regenerate</Trans>
            ) : (
              <Trans>Summarize saved notes</Trans>
            )}
          </Button>
        )}
        <AllDeviceRequests
          controller={controller}
          connected={connected}
          onRecovered={generate.reset}
        />
        <Button
          size="sm"
          variant="ghost"
          disabled={receipts.isFetching}
          onClick={() => refresh(controller)}
        >
          <Trans>Refresh requests</Trans>
        </Button>
      </div>
      <p className="text-muted-foreground text-xs">
        <Trans>
          Saves pending edits before using the saved notes and transcript.
        </Trans>
      </p>
      {(generate.error || active.error || history.error || receipts.error) && (
        <p role="alert" className="text-destructive text-xs">
          {
            (generate.error ?? active.error ?? history.error ?? receipts.error)
              ?.message
          }
        </p>
      )}
      <ReceiptList
        controller={controller}
        connected={connected}
        onRecovered={generate.reset}
        receipts={(targetReceipts ?? []).filter((receipt) => !receipt.job)}
      />
      <div className="flex max-h-52 flex-col gap-2 overflow-y-auto">
        {rows.map((job) => (
          <JobRow
            key={job.id}
            controller={controller}
            job={job}
            connected={connected}
          />
        ))}
        {history.hasNextPage && (
          <Button
            size="sm"
            variant="ghost"
            disabled={!connected || history.isFetchingNextPage}
            onClick={() => void history.fetchNextPage()}
          >
            <Trans>Older summary requests</Trans>
          </Button>
        )}
      </div>
    </section>
  );
}

function AllDeviceRequests({
  controller,
  connected,
  onRecovered,
}: {
  controller: JobSubmissions;
  connected: boolean;
  onRecovered: () => void;
}) {
  const [open, setOpen] = useState(false);
  const receipts = useReceipts(controller, undefined, open);
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button size="sm" variant="ghost">
          <Trans>Device summary requests</Trans>
        </Button>
      </DialogTrigger>
      <DialogContent className="flex max-h-[80vh] flex-col overflow-hidden">
        <DialogHeader>
          <DialogTitle>
            <Trans>Device summary requests</Trans>
          </DialogTitle>
          <DialogDescription>
            <Trans>
              Requests saved for this account and vault, including notes that
              are no longer open. Loading this list does not start processing.
            </Trans>
          </DialogDescription>
        </DialogHeader>
        {receipts.error && <p role="alert">{receipts.error.message}</p>}
        {receipts.isPending ? (
          <p>
            <Trans>Loading…</Trans>
          </p>
        ) : (
          <div className="flex min-h-0 flex-col gap-2 overflow-y-auto">
            {!receipts.data?.length && (
              <p>
                <Trans>No saved summary requests.</Trans>
              </p>
            )}
            <ReceiptList
              controller={controller}
              connected={connected}
              receipts={receipts.data ?? []}
              showSession
              onRecovered={onRecovered}
            />
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}

function ReceiptList({
  controller,
  connected,
  receipts,
  showSession = false,
  onRecovered,
}: {
  controller: JobSubmissions;
  connected: boolean;
  receipts: JobSubmissionReceipt[];
  showSession?: boolean;
  onRecovered: () => void;
}) {
  const resume = useMutation(
    {
      mutationFn: (id: string) => controller.resume(id),
      onSuccess: onRecovered,
      networkMode: "always",
      retry: false,
      onSettled: () => refresh(controller),
    },
    controller.scope.queryClient,
  );
  const forget = useMutation(
    {
      mutationFn: (id: string) => controller.store.forget(id),
      networkMode: "always",
      retry: false,
      onSettled: () => refresh(controller),
    },
    controller.scope.queryClient,
  );
  const [removing, setRemoving] = useState<JobSubmissionReceipt | null>(null);
  return (
    <>
      {receipts.length > 0 && (resume.error || forget.error) && (
        <p role="alert" className="text-destructive text-xs">
          {(resume.error ?? forget.error)?.message}
        </p>
      )}
      {receipts.map((receipt) => (
        <div
          className="flex flex-wrap items-center gap-2 text-xs"
          key={receipt.request_id}
        >
          {showSession && (
            <span className="w-full break-all">{receipt.input.session_id}</span>
          )}
          <span>{new Date(receipt.created_at_ms).toLocaleString()}</span>
          {receipt.job ? (
            <JobStatus job={receipt.job} />
          ) : (
            <span>
              <Trans>
                Processing may already have started. Resume uses the original
                request.
              </Trans>
            </span>
          )}
          <Button
            size="sm"
            variant="outline"
            disabled={!connected || resume.isPending || forget.isPending}
            onClick={() => resume.mutate(receipt.request_id)}
          >
            {resume.isPending && resume.variables === receipt.request_id ? (
              <Trans>Resuming…</Trans>
            ) : receipt.job ? (
              <Trans>Refresh status</Trans>
            ) : (
              <Trans>Resume request</Trans>
            )}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            disabled={forget.isPending}
            onClick={() => setRemoving(receipt)}
          >
            <Trans>Forget device request</Trans>
          </Button>
        </div>
      ))}
      <Dialog
        open={removing !== null}
        onOpenChange={(open) => {
          if (!open && !forget.isPending) setRemoving(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              <Trans>Forget this device request?</Trans>
            </DialogTitle>
            <DialogDescription>
              <Trans>
                This removes the saved request from this device. It does not
                stop processing. A new summary request may run again.
              </Trans>
            </DialogDescription>
          </DialogHeader>
          {forget.error && <p role="alert">{forget.error.message}</p>}
          <div className="flex gap-2">
            <Button
              variant="outline"
              disabled={forget.isPending}
              onClick={() => setRemoving(null)}
            >
              <Trans>Keep request</Trans>
            </Button>
            <Button
              disabled={!removing || forget.isPending}
              onClick={() => {
                if (removing)
                  forget.mutate(removing.request_id, {
                    onSuccess: () => setRemoving(null),
                  });
              }}
            >
              <Trans>Forget device request</Trans>
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}

function JobRow({
  controller,
  job,
  connected,
}: {
  controller: JobSubmissions;
  job: ProcessingJobSnapshot;
  connected: boolean;
}) {
  const cancel = useMutation(
    {
      mutationFn: () => controller.cancel(job.id),
      networkMode: "always",
      retry: false,
      onSettled: () => refresh(controller),
    },
    controller.scope.queryClient,
  );
  return (
    <div className="flex flex-wrap items-center gap-2 rounded border px-2 py-1 text-xs">
      <time dateTime={job.created_at} className="text-muted-foreground">
        {new Date(job.created_at).toLocaleString()}
      </time>
      <JobStatus job={job} />
      {activeJob(job) ? (
        <Button
          size="sm"
          variant="ghost"
          disabled={
            !connected || cancel.isPending || job.state === "cancelling"
          }
          onClick={() => cancel.mutate()}
        >
          <Trans>Cancel summary</Trans>
        </Button>
      ) : (
        <GeneratedText
          scope={controller.scope}
          jobId={job.id}
          connected={connected}
        />
      )}
      {job.state === "conflict" && (
        <p className="w-full">
          <Trans>
            The summary changed during generation. View the generated text to
            recover it.
          </Trans>
        </p>
      )}
      {job.failure && (
        <p className="w-full">
          <JobFailure failure={job.failure} />
        </p>
      )}
      {cancel.error && activeJob(job) && (
        <p className="text-destructive w-full" role="alert">
          {cancel.error.message}
        </p>
      )}
    </div>
  );
}

function GeneratedText({
  scope,
  jobId,
  connected,
}: {
  scope: VaultScope;
  jobId: string;
  connected: boolean;
}) {
  const [open, setOpen] = useState(false);
  const result = useQuery(
    { ...scope.processingJobResultOptions(jobId), enabled: open && connected },
    scope.queryClient,
  );
  const copy = useMutation(
    {
      mutationFn: (markdown: string) => writeText(markdown),
      networkMode: "always",
    },
    scope.queryClient,
  );
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button size="sm" variant="ghost" disabled={!connected}>
          <Trans>Generated text</Trans>
        </Button>
      </DialogTrigger>
      <DialogContent className="flex max-h-[85vh] max-w-2xl flex-col overflow-hidden">
        <DialogHeader>
          <DialogTitle>
            <Trans>Generated summary text</Trans>
          </DialogTitle>
          <DialogDescription>
            <Trans>
              Copy this text to recover a result that could not be applied.
              Editing the current summary still checks for newer changes.
            </Trans>
          </DialogDescription>
        </DialogHeader>
        {result.error && <p role="alert">{result.error.message}</p>}
        {result.isPending ? (
          <p>
            <Trans>Loading…</Trans>
          </p>
        ) : result.data ? (
          <>
            <pre className="min-h-0 overflow-y-auto text-xs break-words whitespace-pre-wrap">
              {result.data.markdown.slice(0, 64 * 1024)}
            </pre>
            {result.data.markdown.length > 64 * 1024 && (
              <p className="text-xs">
                <Trans>
                  The preview is shortened. Copy to recover the complete text.
                </Trans>
              </p>
            )}
            <Button
              size="sm"
              disabled={copy.isPending}
              onClick={() => copy.mutate(result.data!.markdown)}
            >
              <Trans>Copy generated text</Trans>
            </Button>
          </>
        ) : (
          <p>
            <Trans>
              No confirmed generated text is available for this request.
            </Trans>
          </p>
        )}
        {copy.error && <p role="alert">{copy.error.message}</p>}
      </DialogContent>
    </Dialog>
  );
}

function JobStatus({ job }: { job: ProcessingJobSnapshot }) {
  const { t } = useLingui();
  const label = {
    queued: t`Queued`,
    running:
      job.phase === "applying"
        ? t`Saving summary`
        : job.phase === "generating"
          ? t`Generating summary`
          : t`Preparing summary`,
    cancelling: t`Cancelling`,
    cancelled: t`Cancelled`,
    succeeded: t`Summary saved`,
    failed: t`Summary failed`,
    interrupted: t`Summary interrupted`,
    conflict: t`Summary changed`,
  }[job.state];
  return <span role="status">{label}</span>;
}

function JobFailure({
  failure,
}: {
  failure: NonNullable<ProcessingJobSnapshot["failure"]>;
}) {
  const { t } = useLingui();
  return {
    access_denied: t`Access changed before the summary finished.`,
    source_unavailable: t`Saved notes or transcript could not be read.`,
    resource_limit: t`A processing capacity or size limit was reached.`,
    provider_unavailable: t`The selected summary provider is unavailable.`,
    provider_failed: t`The summary provider failed.`,
    invalid_output: t`The provider returned an unusable summary.`,
    storage: t`The summary could not be saved safely.`,
    interrupted: t`Processing stopped before completion.`,
  }[failure];
}
