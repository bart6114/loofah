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
import { useEnhancedNote } from "~/session/queries";
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
  const taskId = enhancedNoteId
    ? createTaskId(enhancedNoteId, "enhance")
    : null;

  const noteTemplateId =
    useEnhancedNote(enhancedNoteId ?? "")?.templateId || undefined;

  const enhanceTask = useAITaskTask(taskId, "enhance");

  const onRegenerate = useCallback(
    async (templateId: string | null) => {
      if (!enhancedNoteId) {
        return;
      }

      if (!model) {
        sonnerToast.error(
          "Set up Intelligence in Settings before regenerating this summary.",
        );
        return;
      }

      try {
        await flushDatabaseWrites([`session:${sessionId}:note`]);
        const snapshot = await loadSessionContentSnapshot(sessionId);
        if (
          !snapshot ||
          !hasSummarySource(snapshot.rawMarkdown, snapshot.transcripts)
        ) {
          sonnerToast.error(EMPTY_SUMMARY_SOURCE_MESSAGE);
          return;
        }

        if (!isMainAITaskHostWindow()) {
          const result = await requestMainEnhance(sessionId, {
            templateId: templateId ?? noteTemplateId,
            targetNoteId: enhancedNoteId,
          });
          if (result.type === "no_model")
            throw new Error(
              "Set up Intelligence in Settings before regenerating this summary.",
            );
          return;
        }

        await enhanceTask.start({
          model,
          args: {
            sessionId,
            enhancedNoteId,
            templateId: templateId ?? noteTemplateId,
          },
        });
      } catch (error) {
        sonnerToast.error(
          error instanceof Error ? error.message : String(error),
        );
      }
    },
    [enhancedNoteId, model, enhanceTask.start, sessionId, noteTemplateId],
  );

  const onCancel = useCallback(() => {
    if (!taskId) {
      return;
    }

    if (!isMainAITaskHostWindow()) {
      void requestMainAITaskCancel(taskId);
      return;
    }

    enhanceTask.cancel();
  }, [enhanceTask.cancel, taskId]);

  return {
    isGenerating: enhanceTask.isGenerating,
    isError: enhanceTask.isError,
    error: enhanceTask.error,
    onRegenerate,
    onCancel,
  };
}
