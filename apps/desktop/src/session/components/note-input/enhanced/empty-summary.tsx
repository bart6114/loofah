import { Trans } from "@lingui/react/macro";

import { useSummarySource } from "~/session/hooks/useSummarySource";
import { useEnhancedDocumentTarget } from "~/session/queries";
import { VaultProcessing } from "~/shared/application/vault-processing";
import { useVaultHost } from "~/shared/application/vault-provider";

export function EmptySummary({
  sessionId,
  enhancedNoteId,
}: {
  sessionId: string;
  sessionTitle: string;
  enhancedNoteId?: string;
  titleTrailerElement?: HTMLElement;
}) {
  const { scope } = useVaultHost();
  const hasSource = useSummarySource(sessionId);
  const selected = useEnhancedDocumentTarget(sessionId, enhancedNoteId ?? "");
  const target = enhancedNoteId ? selected : { kind: "summary" as const };
  if (!scope || target === undefined)
    return (
      <p role="status">
        <Trans>Loading content…</Trans>
      </p>
    );
  if (!target)
    return (
      <p role="alert">
        <Trans>This document is not available for generation.</Trans>
      </p>
    );
  if (
    !scope.client.features.includes("summary_body_jobs") ||
    (target.kind === "template_output" &&
      !scope.client.features.includes("template_output_jobs"))
  )
    return (
      <p role="status">
        <Trans>Summary generation is unavailable for this connection.</Trans>
      </p>
    );
  return (
    <div className="flex min-h-[300px] flex-col gap-4 px-6 py-6">
      <p className="text-muted-foreground text-sm">
        {hasSource ? (
          <Trans>Generate a summary from your notes or transcript.</Trans>
        ) : (
          <Trans>Add a note or transcript to generate a summary.</Trans>
        )}
      </p>
      <VaultProcessing
        scope={scope}
        sessionId={sessionId}
        target={target}
        defaultTitlePolicy="generate_if_empty"
        sourceAvailable={hasSource}
      />
    </div>
  );
}
