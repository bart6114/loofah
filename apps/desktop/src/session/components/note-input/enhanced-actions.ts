import { useCallback } from "react";

import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { useAITaskTask } from "~/ai/hooks";
import { useLanguageModel } from "~/ai/hooks";
import {
  isMainAITaskHostWindow,
  requestMainAITaskCancel,
  requestMainEnhance,
} from "~/ai/task-window-sync";
import { useLegacySessionActivity } from "~/main/use-legacy-activity";
import {
  EMPTY_SUMMARY_SOURCE_MESSAGE,
  hasSummarySource,
} from "~/services/enhancer/source";
import { loadSessionContentSnapshot } from "~/session/content-queries";
import { useEnhancedDocumentTarget } from "~/session/queries";
import { captureSessionGenerationGuard } from "~/shared/application/session-generation-guard";
import { useVaultHost } from "~/shared/application/vault-provider";
import { flushDatabaseWrites } from "~/shared/write-queue";
import { createTaskId } from "~/store/zustand/ai-task/task-configs";

export function useEnhancedNoteActions({
  enhancedNoteId,
  sessionId,
}: {
  enhancedNoteId: string | null;
  sessionId: string;
}) {
  const { isAvailable } = useLegacySessionActivity(sessionId);
  const available = isAvailable("legacy_desktop_enhance");
  const model = useLanguageModel(available);
  const { scope } = useVaultHost();
  const target = useEnhancedDocumentTarget(sessionId, enhancedNoteId ?? "");
  const taskId = enhancedNoteId
    ? createTaskId(enhancedNoteId, "enhance")
    : null;

  const enhanceTask = useAITaskTask(taskId, "enhance");
  const { start, cancel } = enhanceTask;

  const onRegenerate = useCallback(async () => {
    if (!enhancedNoteId) {
      return;
    }

    if (!scope || !isAvailable("legacy_desktop_enhance")) {
      sonnerToast.error(
        "This connection does not support desktop summary generation.",
      );
      return;
    }

    if (!model) {
      sonnerToast.error(
        "Set up summaries in Settings before regenerating this summary.",
      );
      return;
    }

    const guard =
      target &&
      scope.client.canEdit &&
      scope.client.features.includes("document_editor_write")
        ? captureSessionGenerationGuard(scope, sessionId, target)
        : null;
    try {
      if (!guard)
        throw new Error("This document is not available for generation.");
      await guard.prepare();
      await flushDatabaseWrites([`session:${sessionId}:note`]);
      if (!isAvailable("legacy_desktop_enhance")) return;
      const snapshot = await loadSessionContentSnapshot(sessionId);
      if (
        !snapshot ||
        !hasSummarySource(snapshot.rawMarkdown, snapshot.transcripts)
      ) {
        sonnerToast.error(EMPTY_SUMMARY_SOURCE_MESSAGE);
        return;
      }

      guard.assertReady();
      if (!isAvailable("legacy_desktop_enhance")) return;
      if (!isMainAITaskHostWindow()) {
        const result = await requestMainEnhance(sessionId, {
          targetNoteId: enhancedNoteId,
        });
        if (result.type === "no_model")
          throw new Error(
            "Set up summaries in Settings before regenerating this summary.",
          );
        return;
      }

      await start({
        model,
        args: {
          sessionId,
          ...(enhancedNoteId === sessionId
            ? {}
            : { templateDocumentId: enhancedNoteId }),
        },
      });
    } catch (error) {
      sonnerToast.error(error instanceof Error ? error.message : String(error));
    } finally {
      guard?.dispose();
    }
  }, [enhancedNoteId, model, start, sessionId, scope, target, isAvailable]);

  const onCancel = useCallback(() => {
    if (!taskId || !isAvailable("legacy_desktop_enhance")) {
      return;
    }

    if (!isMainAITaskHostWindow()) {
      void requestMainAITaskCancel(taskId);
      return;
    }

    cancel();
  }, [cancel, taskId, isAvailable]);

  return {
    generationUnavailable: !available,
    canRegenerate:
      !!scope &&
      !!target &&
      scope.client.canEdit &&
      scope.client.features.includes("document_editor_write") &&
      available,
    isGenerating: available && enhanceTask.isGenerating,
    isError: available && enhanceTask.isError,
    error: available ? enhanceTask.error : undefined,
    onRegenerate,
    onCancel,
  };
}
