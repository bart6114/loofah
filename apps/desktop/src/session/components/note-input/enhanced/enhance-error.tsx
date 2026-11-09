import { Trans } from "@lingui/react/macro";
import { useMutation } from "@tanstack/react-query";
import { AlertCircleIcon, RefreshCwIcon } from "lucide-react";

import { Button } from "@hypr/ui/components/ui/button";

import { useAITask } from "~/ai/contexts";
import { useLanguageModel } from "~/ai/hooks";
import { useEnhancedDocumentTarget } from "~/session/queries";
import { captureSessionGenerationGuard } from "~/shared/application/session-generation-guard";
import { useVaultHost } from "~/shared/application/vault-provider";
import { createTaskId } from "~/store/zustand/ai-task/task-configs";

export function EnhanceError({
  sessionId,
  enhancedNoteId,
  error,
}: {
  sessionId: string;
  enhancedNoteId: string;
  error: Error | undefined;
}) {
  const model = useLanguageModel();
  const generate = useAITask((state) => state.generate);

  const { scope } = useVaultHost();
  const target = useEnhancedDocumentTarget(sessionId, enhancedNoteId);
  const allowed =
    !!scope &&
    !!target &&
    scope.client.canEdit &&
    scope.client.features.includes("document_editor_write") &&
    scope.client.features.includes("legacy_desktop_enhance");
  const retry = useMutation({
    mutationFn: async () => {
      if (!model || !scope || !target || !allowed)
        throw new Error("This document is not available for generation.");
      const guard = captureSessionGenerationGuard(scope, sessionId, target);
      try {
        await guard.prepare();
        guard.assertReady();

        const taskId = createTaskId(enhancedNoteId, "enhance");
        await generate(taskId, {
          model,
          taskType: "enhance",
          args: {
            sessionId,
            ...(enhancedNoteId === sessionId
              ? {}
              : { templateDocumentId: enhancedNoteId }),
          },
        });
      } finally {
        guard.dispose();
      }
    },
  });

  return (
    <div
      role="alert"
      className="flex h-full min-h-[400px] flex-col items-center justify-center px-6 text-center"
    >
      <AlertCircleIcon
        aria-hidden
        className="text-muted-foreground mb-5 size-9 stroke-[1.5]"
      />
      <div className="mb-6 flex max-w-md flex-col gap-2">
        <p className="text-base font-medium">Summary generation failed</p>
        <p className="text-muted-foreground text-sm leading-relaxed">
          {error?.message || (
            <Trans>Something went wrong while generating the summary.</Trans>
          )}
        </p>
      </div>
      {!scope?.client.features.includes("legacy_desktop_enhance") && (
        <p role="alert">
          This connection does not support desktop summary generation.
        </p>
      )}
      <Button
        onClick={() => retry.mutate()}
        disabled={!model || !allowed || retry.isPending}
        size="sm"
        className="gap-2"
        variant="default"
      >
        <RefreshCwIcon size={16} />
        <span>
          <Trans>Retry</Trans>
        </span>
      </Button>
      {retry.error && <p role="alert">{retry.error.message}</p>}
    </div>
  );
}
