import { useCallback } from "react";

import { commands as fsSyncCommands } from "@hypr/plugin-fs-sync";
import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { confirmRegenerateSpeakerReset } from "./regenerate-confirm";

import { getEnhancerService } from "~/services/enhancer";
import { useListener } from "~/stt/contexts";
import { collectAssignedHumanIdsFromTranscriptRows } from "~/stt/render-transcript";
import { isStoppedTranscriptionError, useRunBatch } from "~/stt/useRunBatch";
import { commands } from "~/types/tauri.gen";

async function countAssignedSpeakers(
  sessionId: string,
  assertCurrent?: () => void,
): Promise<number> {
  try {
    assertCurrent?.();
    const result = await commands.sessionTranscripts(sessionId);
    assertCurrent?.();
    if (result.status === "error") {
      return 0;
    }
    return collectAssignedHumanIdsFromTranscriptRows(result.data).length;
  } catch {
    return 0;
  }
}

export function useRegenerateTranscript(
  sessionId: string,
  assertCurrent?: () => void,
) {
  const runBatch = useRunBatch(sessionId, assertCurrent);
  const handleBatchFailed = useListener((state) => state.handleBatchFailed);

  return useCallback(async () => {
    assertCurrent?.();
    const result = await fsSyncCommands.audioPath(sessionId);
    assertCurrent?.();
    if (result.status === "error") {
      sonnerToast.error("Recording not found. It may have been deleted.", {
        id: `transcript-regenerate-audio-missing-${sessionId}`,
      });
      return;
    }

    const audioPath = result.data;

    const assignedSpeakerCount = await countAssignedSpeakers(
      sessionId,
      assertCurrent,
    );
    assertCurrent?.();
    if (assignedSpeakerCount > 0) {
      const confirmed =
        await confirmRegenerateSpeakerReset(assignedSpeakerCount);
      assertCurrent?.();
      if (!confirmed) {
        return;
      }
    }

    try {
      assertCurrent?.();
      await runBatch(audioPath);
      assertCurrent?.();
      await getEnhancerService()?.queueAutoEnhanceIfSummaryEmpty(sessionId);
    } catch (error) {
      try {
        assertCurrent?.();
      } catch {
        return;
      }
      if (isStoppedTranscriptionError(error)) {
        return;
      }
      const msg = error instanceof Error ? error.message : String(error);
      handleBatchFailed(sessionId, msg);
      sonnerToast.error("Re-transcription failed", {
        id: `transcript-regenerate-failed-${sessionId}`,
        description: msg,
      });
    }
  }, [handleBatchFailed, runBatch, sessionId, assertCurrent]);
}
