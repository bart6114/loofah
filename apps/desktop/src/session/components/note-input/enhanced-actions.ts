import { useCallback } from "react";

import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { useEnhancedDocumentTarget } from "~/session/queries";
import { useVaultReadScope } from "~/shared/application/use-vault-read-scope";
import { useDocumentProcessing } from "~/shared/application/vault-processing";

export function useEnhancedNoteActions({
  enhancedNoteId,
  sessionId,
}: {
  enhancedNoteId: string | null;
  sessionId: string;
}) {
  const owner = useVaultReadScope();
  const target = useEnhancedDocumentTarget(sessionId, enhancedNoteId ?? "");
  const processing = useDocumentProcessing(
    owner.scope,
    sessionId,
    enhancedNoteId ? target : null,
    owner.current(),
  );
  const onRegenerate = useCallback(async () => {
    if (!owner.current() || !processing.canGenerate) return;
    try {
      await processing.generate.mutateAsync(
        owner.scope.client.features.includes("generated_titles")
          ? "generate_if_empty"
          : "preserve",
      );
    } catch (error) {
      if (owner.current())
        sonnerToast.error(
          error instanceof Error ? error.message : String(error),
        );
    }
  }, [owner.current, owner.scope, processing.canGenerate, processing.generate]);
  const onCancel = useCallback(async () => {
    if (!owner.current() || !processing.isGenerating) return;
    try {
      await processing.cancel.mutateAsync();
    } catch (error) {
      if (owner.current())
        sonnerToast.error(
          error instanceof Error ? error.message : String(error),
        );
    }
  }, [owner.current, processing.cancel, processing.isGenerating]);
  return {
    generationUnavailable: !processing.supported,
    canRegenerate: processing.canGenerate,
    isGenerating: processing.isGenerating,
    isError:
      processing.generate.isError || processing.rows[0]?.state === "failed",
    error: processing.generate.error,
    onRegenerate,
    onCancel,
  };
}
