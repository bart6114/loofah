import { Trans, useLingui } from "@lingui/react/macro";
import { useForm } from "@tanstack/react-form";
import { useMutation } from "@tanstack/react-query";
import { useStore } from "@tanstack/react-store";
import { useEffect, useMemo } from "react";

import { commands as openerCommands } from "@hypr/plugin-opener2";
import { Button } from "@hypr/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@hypr/ui/components/ui/dialog";

import { useEnhancedDocumentTarget } from "~/session/queries";
import { useVaultReadScope } from "~/shared/application/use-vault-read-scope";
import type { EditorView } from "~/store/zustand/tabs/schema";
import type { SessionExportInput } from "~/types/tauri.gen";

export function ExportModal(props: {
  sessionId: string;
  currentView: EditorView;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  return props.open ? (
    <ActiveExportModal
      key={JSON.stringify([props.sessionId, props.currentView])}
      {...props}
    />
  ) : null;
}

function ActiveExportModal({
  sessionId,
  currentView,
  onOpenChange,
}: {
  sessionId: string;
  currentView: EditorView;
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useLingui();
  const owner = useVaultReadScope();
  const { scope, current } = owner;
  const selectedTarget = useEnhancedDocumentTarget(
    sessionId,
    currentView.type === "enhanced" ? currentView.id : "",
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
  useEffect(() => {
    if (!current() || scope.getSnapshot().state !== "connected")
      lifetime.abort.abort();
  }, [owner.connection, current, scope, lifetime]);
  const form = useForm({
    defaultValues: {
      format: "pdf" as SessionExportInput["format"],
      includeNote: true,
      includeSummary: true,
      includeTranscript: false,
    },
  });
  const values = useStore(form.store, (state) => state.values);
  const connected = current() && owner.connection.state === "connected";
  const blockedDocument =
    values.includeSummary && currentView.type === "enhanced" && !selectedTarget;
  const mutation = useMutation(
    {
      retry: false,
      networkMode: "always",
      mutationFn: async (input: SessionExportInput) => {
        const exporter = scope.client.exportSession;
        if (
          !lifetime.active ||
          !current() ||
          scope.getSnapshot().state !== "connected" ||
          !exporter
        )
          throw new Error(t`Export is unavailable for this connection.`);
        const abort = new AbortController();
        lifetime.abort = abort;
        const result = await scope.run(() =>
          exporter(input, {
            requestId: crypto.randomUUID(),
            signal: abort.signal,
          }),
        );
        if (!lifetime.active || !current() || abort.signal.aborted) return;
        const reveal = await openerCommands.revealItemInDir(result.path);
        if (reveal.status === "error") throw new Error(reveal.error);
        if (lifetime.active && current()) onOpenChange(false);
      },
    },
    scope.queryClient,
  );
  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogContent className="max-w-sm">
        <DialogHeader>
          <DialogTitle>
            <Trans>Export</Trans>
          </DialogTitle>
          <DialogDescription>
            <Trans>
              Choose a file format and which saved content to include.
            </Trans>
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-4">
          <form.Field name="format">
            {(field) => (
              <fieldset
                className="flex flex-col gap-2"
                disabled={mutation.isPending}
              >
                <legend className="text-sm font-medium">
                  <Trans>File format</Trans>
                </legend>
                <div className="flex gap-4">
                  {(["pdf", "txt", "md", "org"] as const).map((format) => (
                    <label
                      key={format}
                      className="flex items-center gap-1 text-sm"
                    >
                      <input
                        type="radio"
                        name="export-format"
                        value={format}
                        checked={field.state.value === format}
                        onChange={() => field.handleChange(format)}
                      />
                      {format.toUpperCase()}
                    </label>
                  ))}
                </div>
              </fieldset>
            )}
          </form.Field>
          <div className="flex flex-col gap-2">
            <form.Field name="includeNote">
              {(field) => (
                <label className="flex items-center gap-2 text-sm">
                  <input
                    type="checkbox"
                    checked={field.state.value}
                    disabled={mutation.isPending}
                    onChange={(event) =>
                      field.handleChange(event.target.checked)
                    }
                  />
                  <Trans>Note</Trans>
                </label>
              )}
            </form.Field>
            <form.Field name="includeSummary">
              {(field) => (
                <label className="flex items-center gap-2 text-sm">
                  <input
                    type="checkbox"
                    checked={field.state.value}
                    disabled={mutation.isPending}
                    onChange={(event) =>
                      field.handleChange(event.target.checked)
                    }
                  />
                  <Trans>Summary</Trans>
                </label>
              )}
            </form.Field>
            <form.Field name="includeTranscript">
              {(field) => (
                <label className="flex items-center gap-2 text-sm">
                  <input
                    type="checkbox"
                    checked={field.state.value}
                    disabled={mutation.isPending}
                    onChange={(event) =>
                      field.handleChange(event.target.checked)
                    }
                  />
                  <Trans>Transcript</Trans>
                </label>
              )}
            </form.Field>
          </div>
          {blockedDocument && (
            <p role="alert">
              <Trans>
                This document cannot be identified safely. Export is
                unavailable.
              </Trans>
            </p>
          )}
          {mutation.error && <p role="alert">{mutation.error.message}</p>}
          <Button
            disabled={
              !connected ||
              !scope.client.exportSession ||
              mutation.isPending ||
              blockedDocument ||
              !(
                values.includeNote ||
                values.includeSummary ||
                values.includeTranscript
              )
            }
            onClick={() =>
              mutation.mutate({
                session_id: sessionId,
                target:
                  currentView.type === "enhanced"
                    ? (selectedTarget ?? { kind: "summary" })
                    : { kind: "summary" },
                format: values.format,
                include_note: values.includeNote,
                include_summary: values.includeSummary,
                include_transcript: values.includeTranscript,
                labels: {
                  untitled: t`Untitled`,
                  created: t`Created`,
                  participants: t`Participants`,
                  duration: t`Duration`,
                  metadata: t`Metadata`,
                  note: t`Note`,
                  summary: t`Summary`,
                  transcript: t`Transcript`,
                },
              })
            }
          >
            {mutation.isPending ? (
              <Trans>Exporting...</Trans>
            ) : (
              <Trans>Export</Trans>
            )}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
