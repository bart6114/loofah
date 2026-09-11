import { Trans } from "@lingui/react/macro";
import { useMutation } from "@tanstack/react-query";

import { Button } from "@hypr/ui/components/ui/button";

import { ConfigError } from "./config-error";

import { useLLMConnectionStatus } from "~/ai/hooks";
import {
  isMainAITaskHostWindow,
  requestMainEnhance,
} from "~/ai/task-window-sync";
import { getEnhancerService } from "~/services/enhancer";
import { EMPTY_SUMMARY_SOURCE_MESSAGE } from "~/services/enhancer/source";
import { useSummarySource } from "~/session/hooks/useSummarySource";
import { useOpenSummaryPrompt } from "~/settings/use-open-summary-prompt";
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
  const hasSource = useSummarySource(sessionId);
  const llmStatus = useLLMConnectionStatus();
  const modelReady = llmStatus.status === "success";
  const connecting =
    llmStatus.status === "pending" && llmStatus.reason === "connecting";
  const openSummaryPrompt = useOpenSummaryPrompt();
  const generate = useMutation({
    mutationFn: async () => {
      await flushDatabaseWrites([`session:${sessionId}:note`]);
      const opts = {
        targetNoteId: enhancedNoteId,
      };
      if (!isMainAITaskHostWindow()) {
        const result = await requestMainEnhance(sessionId, opts);
        if (result.type === "no_model")
          throw new Error(
            "Set up Intelligence in Settings before generating a summary.",
          );
        return;
      }
      const service = getEnhancerService();
      if (!service)
        throw new Error("Summary service is unavailable. Please try again.");
      const result = await service.enhance(sessionId, opts);
      if (result.type === "no_model")
        throw new Error(
          "Set up Intelligence in Settings before generating a summary.",
        );
    },
  });

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
        disabled={!hasSource || !modelReady || generate.isPending}
        onClick={() => generate.mutate(undefined)}
      >
        {connecting
          ? "Connecting Intelligence…"
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
