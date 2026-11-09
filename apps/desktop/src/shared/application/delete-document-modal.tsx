import { Trans, useLingui } from "@lingui/react/macro";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";

import { Button } from "@hypr/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@hypr/ui/components/ui/dialog";

import { documentEditorBaseline } from "./document-editor-draft";
import { ApplicationRequestError } from "./errors";
import type { DocumentTarget } from "./protocol.gen";
import { captureSessionGenerationGuard } from "./session-generation-guard";
import { useVaultReadScope } from "./use-vault-read-scope";

export function DeleteDocumentModal(props: {
  sessionId: string;
  target: DocumentTarget;
  onClose: () => void;
  onDeleted: () => void;
}) {
  const { t } = useLingui();
  const owner = useVaultReadScope();
  const { scope, current } = owner;
  const [sessionId] = useState(props.sessionId);
  const [target] = useState(() => Object.freeze(structuredClone(props.target)));
  const originalView =
    props.sessionId === sessionId &&
    JSON.stringify(props.target) === JSON.stringify(target);
  const [requestId] = useState(() => crypto.randomUUID());
  const lifetime = useMemo(
    () => ({
      active: false,
      abort: new AbortController(),
      guard: null as ReturnType<typeof captureSessionGenerationGuard> | null,
      sent: false,
    }),
    [scope, sessionId],
  );
  useEffect(() => {
    lifetime.active = true;
    lifetime.abort = new AbortController();
    return () => {
      lifetime.active = false;
      lifetime.abort.abort();
      lifetime.guard?.dispose();
    };
  }, [lifetime]);
  useEffect(() => {
    if (!current() || !originalView) lifetime.abort.abort();
  }, [owner.connection, current, lifetime, originalView]);
  const available =
    current() &&
    originalView &&
    !lifetime.abort.signal.aborted &&
    owner.connection.state === "connected" &&
    scope.client.canEdit &&
    !!scope.client.deleteDocument &&
    target.kind === "template_output";
  const preview = useQuery(
    {
      queryKey: ["document_delete_preview", scope.client.scope, requestId],
      queryFn: async ({ signal }) => {
        const guard = captureSessionGenerationGuard(
          scope,
          sessionId,
          target,
          signal,
        );
        lifetime.guard?.dispose();
        lifetime.guard = guard;
        try {
          await guard.prepare();
        } catch {
          throw new Error(
            t`Save or recover your edits before removing this output. Your draft stays on this device.`,
          );
        }
        const snapshot = await scope.run(() =>
          scope.client.readDocumentEditor(sessionId, target, { signal }),
        );
        documentEditorBaseline(sessionId, target, snapshot);
        guard.assertReady();
        return structuredClone(snapshot);
      },
      enabled: available,
      retry: false,
      networkMode: "always",
      staleTime: "static",
      refetchOnWindowFocus: false,
      refetchOnReconnect: false,
      refetchOnMount: false,
    },
    scope.queryClient,
  );
  const mutation = useMutation(
    {
      retry: false,
      networkMode: "always",
      mutationFn: async () => {
        const remove = scope.client.deleteDocument;
        if (
          !lifetime.active ||
          !current() ||
          !originalView ||
          scope.getSnapshot().state !== "connected" ||
          !scope.client.canEdit ||
          !remove ||
          !preview.data ||
          !preview.data.document.document ||
          lifetime.abort.signal.aborted
        )
          throw new Error(
            t`This output is unavailable in its original workspace.`,
          );
        if (!lifetime.sent) {
          try {
            lifetime.guard?.assertReady();
          } catch {
            throw new Error(
              t`Save or recover your edits before removing this output. Your draft stays on this device.`,
            );
          }
        }
        lifetime.sent = true;
        try {
          return await scope.run(() =>
            remove(
              sessionId,
              target,
              {
                expected_revision: preview.data.document.revision,
                expected_session_created_at:
                  preview.data.metadata.meta.created_at,
              },
              { requestId, signal: lifetime.abort.signal },
            ),
          );
        } catch (cause) {
          if (
            cause instanceof ApplicationRequestError &&
            cause.detail.mutation_outcome === "not_committed"
          )
            lifetime.sent = false;
          throw cause;
        }
      },
      onSuccess: (result) => {
        if (
          !lifetime.active ||
          !current() ||
          !originalView ||
          lifetime.abort.signal.aborted
        )
          return;
        if (result.kind === "deleted") props.onDeleted();
      },
    },
    scope.queryClient,
  );
  const outcome = mutation.data;
  const document = preview.data?.document.document;
  return (
    <Dialog open onOpenChange={(open) => !open && props.onClose()}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>
            <Trans>Remove saved output</Trans>
          </DialogTitle>
          <DialogDescription>
            <Trans>
              Review the saved output below. Removing it moves this file to the
              vault trash. Your note and tasks stay saved.
            </Trans>
          </DialogDescription>
        </DialogHeader>
        {preview.isPending && available && (
          <p role="status">
            <Trans>Loading saved output…</Trans>
          </p>
        )}
        {document && (
          <section aria-label={t`Saved output to remove`}>
            <p className="mb-2 font-medium">{document.title}</p>
            <pre className="max-h-64 overflow-auto rounded border p-3 font-sans text-sm whitespace-pre-wrap">
              {document.markdown}
            </pre>
          </section>
        )}
        {!available && (
          <p role="status">
            <Trans>This output is unavailable in its original workspace.</Trans>
          </p>
        )}
        {preview.error && <p role="alert">{preview.error.message}</p>}
        {outcome?.kind === "conflict" && (
          <p role="alert">
            <Trans>
              The saved output changed. Close this dialog and review it before
              removing it.
            </Trans>
          </p>
        )}
        {outcome?.kind === "not_found" && (
          <p role="status">
            <Trans>This saved output no longer exists.</Trans>
          </p>
        )}
        {mutation.error && (
          <div role="alert" className="flex flex-col gap-2">
            <p>{mutation.error.message}</p>
            <p>
              <Trans>
                Removal could not be confirmed. Retry checks the same request
                and saved version.
              </Trans>
            </p>
            <p className="text-xs break-all">
              <Trans>Request</Trans>: {requestId}
            </p>
          </div>
        )}
        <div className="flex justify-end gap-2">
          <Button variant="outline" onClick={props.onClose}>
            <Trans>Close</Trans>
          </Button>
          <Button
            variant="destructive"
            disabled={
              !available ||
              !document ||
              !!preview.error ||
              mutation.isPending ||
              !!outcome
            }
            onClick={() => mutation.mutate()}
          >
            {mutation.error ? (
              <Trans>Retry original removal</Trans>
            ) : (
              <Trans>Remove output</Trans>
            )}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
