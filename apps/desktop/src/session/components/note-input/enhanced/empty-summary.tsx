import { Trans } from "@lingui/react/macro";
import { useMutation } from "@tanstack/react-query";

import { Button } from "@hypr/ui/components/ui/button";

import { ConfigError } from "./config-error";

import { useLLMConnectionStatus } from "~/ai/hooks";
import {
  isMainAITaskHostWindow,
  requestMainEnhance,
} from "~/ai/task-window-sync";
import { useLegacySessionActivity } from "~/main/use-legacy-activity";
import { getEnhancerService } from "~/services/enhancer";
import { EMPTY_SUMMARY_SOURCE_MESSAGE } from "~/services/enhancer/source";
import { useSummarySource } from "~/session/hooks/useSummarySource";
import { useEnhancedDocumentTarget } from "~/session/queries";
import { useOpenSummaryPrompt } from "~/settings/use-open-summary-prompt";
import { captureSessionGenerationGuard } from "~/shared/application/session-generation-guard";
import { useVaultHost } from "~/shared/application/vault-provider";
import { flushDatabaseWrites } from "~/shared/write-queue";

export function EmptySummary({
  sessionId,
  sessionTitle,
  enhancedNoteId,
  titleTrailerElement,
}: {
  sessionId: string;
  sessionTitle: string;
  enhancedNoteId?: string;
  titleTrailerElement?: HTMLElement;
}) {
  const { isAvailable } = useLegacySessionActivity(sessionId);
  const hasSource = useSummarySource(sessionId);
  const { scope } = useVaultHost();
  const selected = useEnhancedDocumentTarget(sessionId, enhancedNoteId ?? "");
  const target = enhancedNoteId ? selected : { kind: "summary" as const };
  const allowed =
    !!scope &&
    !!target &&
    scope.client.canEdit &&
    scope.client.features.includes("document_editor_write") &&
    scope.client.features.includes("legacy_desktop_enhance");
  const llmStatus = useLLMConnectionStatus(
    isAvailable("legacy_desktop_enhance"),
  );
  const modelReady = llmStatus.status === "success";
  const connecting =
    llmStatus.status === "pending" && llmStatus.reason === "connecting";
  const openSummaryPrompt = useOpenSummaryPrompt();
  const generate = useMutation({
    mutationFn: async () => {
      if (
        !scope ||
        !target ||
        !allowed ||
        !isAvailable("legacy_desktop_enhance")
      )
        throw new Error("This document is not available for generation.");
      const guard = captureSessionGenerationGuard(scope, sessionId, target);
      try {
        await guard.prepare();
        await flushDatabaseWrites([`session:${sessionId}:note`]);
        const opts = {
          targetNoteId: enhancedNoteId,
        };
        guard.assertReady();
        if (!isAvailable("legacy_desktop_enhance")) return;
        if (!isMainAITaskHostWindow()) {
          const result = await requestMainEnhance(sessionId, opts);
          if (result.type === "no_model")
            throw new Error(
              "Set up summaries in Settings before generating a summary.",
            );
          return;
        }
        const service = getEnhancerService();
        if (!service)
          throw new Error("Summary service is unavailable. Please try again.");
        const result = await service.enhance(sessionId, opts);
        if (result.type === "no_model")
          throw new Error(
            "Set up summaries in Settings before generating a summary.",
          );
      } finally {
        guard.dispose();
      }
    },
  });

  if (!isAvailable("legacy_desktop_enhance"))
    return (
      <p role="alert">
        This connection does not support desktop summary generation.
      </p>
    );

  if (hasSource && !modelReady && !connecting)
    return (
      <ConfigError
        sessionTitle={sessionTitle}
        titleTrailerElement={titleTrailerElement}
      />
    );

  return (
    <div className="flex min-h-[300px] flex-col items-center justify-center gap-4 px-6 text-center">
      <p className="text-muted-foreground text-sm">
        {hasSource
          ? "Generate a summary from your notes or transcript."
          : EMPTY_SUMMARY_SOURCE_MESSAGE}
      </p>
      <Button
        disabled={!allowed || !hasSource || !modelReady || generate.isPending}
        onClick={() => generate.mutate(undefined)}
      >
        {connecting
          ? "Connecting to your AI provider…"
          : generate.isPending
            ? "Starting summary…"
            : "Generate summary"}
      </Button>
      <Button variant="ghost" onClick={openSummaryPrompt}>
        <Trans>Edit summary prompt</Trans>
      </Button>
      {generate.error && (
        <p role="alert" className="text-destructive text-sm">
          {generate.error.message}
        </p>
      )}
    </div>
  );
}
