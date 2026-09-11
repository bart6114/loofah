import { useCallback, useRef } from "react";

import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { useListener } from "./contexts";
import { getSessionKeywords } from "./useKeywords";
import {
  canRunBatchTranscription,
  isStoppedTranscriptionError,
  useRunBatch,
} from "./useRunBatch";
import { useSTTConnection } from "./useSTTConnection";

import { requestMainAutoEnhance } from "~/ai/task-window-sync";
import { useShell } from "~/contexts/shell";
import { getEnhancerService } from "~/services/enhancer";
import { catalogLocalSessionAudio } from "~/session/attachments";
import { enqueueSessionAudioOperation } from "~/session/audio-operations";
import { useSession, useSessionHasTranscript } from "~/session/queries";
import { useConfigValue } from "~/shared/config";
import { id } from "~/shared/utils";
import type {
  LiveTranscriptPersistCallback,
  OnStoppedCallback,
} from "~/store/zustand/listener/transcript";
import {
  getLiveTranscriptionConfig,
  getTranscriptionLanguages,
} from "~/stt/capabilities";
import { softDeleteTranscript } from "~/stt/queries";
import { queueTagSuggestions } from "~/tags/suggestions";
import { commands } from "~/types/tauri.gen";

export function getPostCaptureAction(
  details: {
    audioPath: string | null;
    liveTranscriptionActive: boolean;
    needsBatchRepair: boolean;
  },
  canRunBatch: boolean,
  // `liveTranscriptionActive` only reports the transcription *mode* the session ran in, so a
  // live stream that opens and dies without emitting a word is indistinguishable from one that
  // worked. Whether any words actually landed is the thing worth trusting: if none did, the
  // recording still has to be transcribed rather than handed to the summarizer empty.
  liveTranscriptEmpty = false,
) {
  if (
    details.liveTranscriptionActive &&
    !details.needsBatchRepair &&
    !liveTranscriptEmpty
  ) {
    return "enhance_only" as const;
  }

  if (!!details.audioPath && canRunBatch) {
    return "batch_then_enhance" as const;
  }

  return "none" as const;
}

export function useStartListening(sessionId: string) {
  const session = useSession(sessionId);
  const hadTranscriptBeforeStart = useSessionHasTranscript(sessionId);

  const aiLanguage = useConfigValue("ai_language");
  const spokenLanguages = useConfigValue("spoken_languages");
  const dictionaryTerms = useConfigValue("personalization_dictionary_terms");

  const start = useListener((state) => state.start);
  const { conn } = useSTTConnection();
  const runBatch = useRunBatch(sessionId);
  const { leftsidebar } = useShell();
  const setLeftSidebarExpanded = leftsidebar.setExpanded;

  const runBatchRef = useRef(runBatch);
  const canRunBatchRef = useRef(canRunBatchTranscription(conn));
  runBatchRef.current = runBatch;
  canRunBatchRef.current = canRunBatchTranscription(conn);

  const startListening = useCallback(async () => {
    let transcriptId: string | null = null;
    const startedAt = Date.now();
    let lastTranscriptWrite = Promise.resolve();
    const reportTranscriptWriteError = (error: unknown) => {
      console.error("[listener] failed to persist transcript", error);
      sonnerToast.error(`Transcript is NOT being saved: ${error}`, {
        id: "live-transcript-persist-failed",
        duration: Infinity,
      });
    };
    const trackTranscriptWrite = (write: Promise<void>) => {
      lastTranscriptWrite = write.catch(reportTranscriptWriteError);
    };
    const keywords = await getSessionKeywords({
      sessionId,
      dictionaryTerms,
    });

    const onStopped: OnStoppedCallback = async (_sessionId, details) => {
      // Cataloging can relocate the recording, so everything downstream reads the path it
      // settled at; the capture backend's path is only a fallback for when cataloging failed
      // and the file is therefore still where capture left it.
      let storedAudioPath = details.audioPath;
      if (details.audioPath) {
        const audioPath = details.audioPath;
        try {
          storedAudioPath = await enqueueSessionAudioOperation(sessionId, () =>
            catalogLocalSessionAudio(sessionId, audioPath),
          );
        } catch (error) {
          console.error("[listener] failed to catalog recorded audio", error);
          sonnerToast.error(
            "Recording audio could not be moved into the session folder — it remains at its original location",
            { id: "audio-catalog-failed" },
          );
        }
      }
      await lastTranscriptWrite;
      if (transcriptId) {
        try {
          const result = await commands.sessionFlushTranscript(sessionId);
          if (result.status === "error") throw new Error(result.error);
        } catch (error) {
          reportTranscriptWriteError(error);
        }
      }

      // Restricted to sessions that came out of this capture with no transcript at all:
      // re-transcribing one that already has words would duplicate them over the same audio.
      const liveTranscriptEmpty =
        !hadTranscriptBeforeStart && transcriptId === null;

      const postCaptureAction = getPostCaptureAction(
        details,
        canRunBatchRef.current,
        liveTranscriptEmpty,
      );

      let batchCompleted = false;
      if (postCaptureAction === "batch_then_enhance") {
        try {
          await runBatchRef.current(storedAudioPath!);
          batchCompleted = true;
        } catch (error) {
          if (isStoppedTranscriptionError(error)) {
            return;
          }
          console.error(
            "[listener] failed to run post-capture transcription",
            error,
          );
          sonnerToast.error(
            "Post-meeting transcription failed. Summarizing the live transcript instead.",
            { id: "post-capture-batch-failed" },
          );
        }
      }

      const hasTranscriptEvidence =
        hadTranscriptBeforeStart || transcriptId !== null || batchCompleted;
      if (!batchCompleted && transcriptId !== null) {
        await queueTagSuggestions(sessionId);
      }
      if (postCaptureAction !== "none" || hasTranscriptEvidence) {
        const shouldRegenerateExistingSummary =
          hadTranscriptBeforeStart && (transcriptId !== null || batchCompleted);
        const service = getEnhancerService();
        if (!service) {
          await requestMainAutoEnhance(
            sessionId,
            shouldRegenerateExistingSummary ? "regenerate" : "if_empty",
          );
        } else if (shouldRegenerateExistingSummary) {
          await service.resetEnhanceTasks(sessionId);
          service.queueAutoEnhance(sessionId);
        } else {
          await service.queueAutoEnhanceIfSummaryEmpty(sessionId);
        }
      }
    };

    const handlePersist: LiveTranscriptPersistCallback = (delta) => {
      if (delta.new_words.length === 0 && delta.replaced_ids.length === 0) {
        return;
      }
      if (!transcriptId) transcriptId = id();

      trackTranscriptWrite(
        commands
          .sessionAppendTranscript(sessionId, {
            transcript_id: transcriptId,
            new_words: delta.new_words,
            replaced_ids: delta.replaced_ids,
            // Live deltas from the transcription plugin carry no speaker-hint data (that's
            // produced by the separate batch/assignment paths) -- nothing to forward here.
            new_hints: [],
            started_at_ms: startedAt,
          })
          .then((result) => {
            if (result.status === "error") throw new Error(result.error);
          }),
      );
    };

    const languages = getTranscriptionLanguages(aiLanguage, spokenLanguages);
    const liveTranscriptionConfig = await getLiveTranscriptionConfig({
      provider: conn?.provider,
      model: conn?.model,
      languages,
    });

    const started = await start(
      {
        session_id: sessionId,
        languages: liveTranscriptionConfig.languages,
        onboarding: false,
        model: conn?.model ?? "",
        base_url: conn?.baseUrl ?? "",
        api_key: conn?.apiKey ?? "",
        keywords,
        transcription_mode: liveTranscriptionConfig.transcriptionMode,
        participant_human_ids: [],
        self_human_id: session?.user_id || null,
      },
      {
        handlePersist,
        onStopped,
      },
    );

    if (!started) {
      await lastTranscriptWrite;
      if (transcriptId) {
        await softDeleteTranscript(sessionId, transcriptId);
      }
      return;
    }

    setLeftSidebarExpanded(false);
  }, [
    aiLanguage,
    conn,
    dictionaryTerms,
    hadTranscriptBeforeStart,
    session,
    sessionId,
    setLeftSidebarExpanded,
    spokenLanguages,
    start,
  ]);

  return startListening;
}
