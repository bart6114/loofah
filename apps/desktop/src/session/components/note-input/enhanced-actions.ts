import { useCallback } from "react";

import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { useAITaskTask } from "~/ai/hooks";
import { useLanguageModel } from "~/ai/hooks";
import {
  isMainAITaskHostWindow,
  requestMainAITaskCancel,
  requestMainEnhance,
} from "~/ai/task-window-sync";
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
  const model = useLanguageModel();
  const { scope } = useVaultHost();
  const target = useEnhancedDocumentTarget(sessionId, enhancedNoteId ?? "");
  const taskId = enhancedNoteId
    ? createTaskId(enhancedNoteId, "enhance")
    : null;

  const enhanceTask = useAITaskTask(taskId, "enhance");

  const onRegenerate = useCallback(async () => {
    if (!enhancedNoteId) {
      return;
    }

    if (!scope?.client.features.includes("legacy_desktop_enhance")) {
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
      const snapshot = await loadSessionContentSnapshot(sessionId);
      if (
        !snapshot ||
        !hasSummarySource(snapshot.rawMarkdown, snapshot.transcripts)
      ) {
        sonnerToast.error(EMPTY_SUMMARY_SOURCE_MESSAGE);
        return;
      }

      guard.assertReady();
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

      await enhanceTask.start({
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
  }, [enhancedNoteId, model, enhanceTask.start, sessionId, scope, target]);

  const onCancel = useCallback(() => {
    if (!taskId || !scope?.client.features.includes("legacy_desktop_enhance")) {
      return;
    }

    if (!isMainAITaskHostWindow()) {
      void requestMainAITaskCancel(taskId);
      return;
    }

    enhanceTask.cancel();
  }, [enhanceTask.cancel, taskId, scope]);

  return {
    generationUnavailable: !scope?.client.features.includes(
      "legacy_desktop_enhance",
    ),
    canRegenerate:
      !!scope &&
      !!target &&
      scope.client.canEdit &&
      scope.client.features.includes("document_editor_write") &&
      scope.client.features.includes("legacy_desktop_enhance"),
    isGenerating:
      !!scope?.client.features.includes("legacy_desktop_enhance") &&
      enhanceTask.isGenerating,
    isError:
      !!scope?.client.features.includes("legacy_desktop_enhance") &&
      enhanceTask.isError,
    error: scope?.client.features.includes("legacy_desktop_enhance")
      ? enhanceTask.error
      : undefined,
    onRegenerate,
    onCancel,
  };
}
