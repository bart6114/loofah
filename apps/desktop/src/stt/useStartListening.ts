import { useCallback, useRef } from "react";

import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { useListener } from "./contexts";
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
import { beginSummaryActivity } from "~/session/summary-activity";
import { useVaultReadScope } from "~/shared/application/use-vault-read-scope";
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

export function useStartListening(
  sessionId: string,
  isAvailable: () => boolean,
) {
  const summaryOwner = useVaultReadScope();
  const session = useSession(sessionId);
  const hadTranscriptBeforeStart = useSessionHasTranscript(sessionId);

  const meetingLanguages = useConfigValue("meeting_languages");
  const transcriptionTiming = useConfigValue("transcription_timing");

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
    if (!isAvailable()) return;
    const requestId = crypto.randomUUID();
    const activity = beginSummaryActivity(sessionId, "recording");
    let started = false;
    try {
      let transcriptId: string | null = null;
      const startedAt = Date.now();
      let lastTranscriptWrite = Promise.resolve();
      let persistenceFailed = false;
      const reportTranscriptWriteError = (error: unknown) => {
        persistenceFailed = true;
        console.error("[listener] failed to persist transcript", error);
        sonnerToast.error(`Transcript is NOT being saved: ${error}`, {
          id: "live-transcript-persist-failed",
          duration: Infinity,
        });
      };
      const trackTranscriptWrite = (write: () => Promise<void>) => {
        lastTranscriptWrite = lastTranscriptWrite
          .then(write)
          .catch(reportTranscriptWriteError);
      };
      const onStopped: OnStoppedCallback = async (_sessionId, details) => {
        activity.setPhase("processing");
        try {
          // Cataloging can relocate the recording, so everything downstream reads the path it
          // settled at; the capture backend's path is only a fallback for when cataloging failed
          // and the file is therefore still where capture left it.
          let storedAudioPath = details.audioPath;
          if (details.audioPath) {
            const audioPath = details.audioPath;
            try {
              storedAudioPath = await enqueueSessionAudioOperation(
                sessionId,
                () => catalogLocalSessionAudio(sessionId, audioPath),
              );
            } catch (error) {
              persistenceFailed = true;
              console.error(
                "[listener] failed to catalog recorded audio",
                error,
              );
              sonnerToast.error(
                "Recording audio could not be moved into the session folder — it remains at its original location",
                { id: "audio-catalog-failed" },
              );
            }
          }
          await lastTranscriptWrite;
          if (transcriptId) {
            try {
              const result = await commands.sessionFinishTranscript(
                sessionId,
                transcriptId,
              );
              if (result.status === "error") throw new Error(result.error);
            } catch (error) {
              reportTranscriptWriteError(error);
            }
          }

          if (!persistenceFailed) {
            try {
              const acknowledged =
                await commands.applicationFinishLocalSessionWork(
                  sessionId,
                  requestId,
                );
              if (acknowledged.status === "error") throw acknowledged.error;
            } catch (error) {
              console.error(
                "[listener] could not confirm durable session work",
                error,
              );
              sonnerToast.warning(
                "Recording completion could not be confirmed.",
                { id: `local-session-work:${requestId}`, duration: Infinity },
              );
              return;
            }
          }
          if (!isAvailable()) return;

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
              await runBatchRef.current(storedAudioPath!, { imported: false });
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
          if (
            isAvailable() &&
            (postCaptureAction !== "none" || hasTranscriptEvidence)
          ) {
            const shouldRegenerateExistingSummary =
              hadTranscriptBeforeStart &&
              (transcriptId !== null || batchCompleted);
            const service = getEnhancerService();
            if (
              !summaryOwner.current() ||
              (service && service.scope !== summaryOwner.scope)
            )
              return;
            if (!service) {
              await requestMainAutoEnhance(
                summaryOwner.scope,
                sessionId,
                shouldRegenerateExistingSummary ? "regenerate" : "if_empty",
                () => summaryOwner.current() && isAvailable(),
              );
            } else if (shouldRegenerateExistingSummary) {
              await service.resetEnhanceTasks(sessionId);
              if (isAvailable() && summaryOwner.current())
                service.queueAutoEnhance(sessionId);
            } else {
              await service.queueAutoEnhanceIfSummaryEmpty(sessionId);
            }
          }
        } finally {
          activity.finish();
        }
      };

      const handlePersist: LiveTranscriptPersistCallback = (delta) => {
        if (delta.new_words.length === 0 && delta.replaced_ids.length === 0) {
          return;
        }
        if (!transcriptId) transcriptId = id();

        const captureTranscriptId = transcriptId;
        trackTranscriptWrite(() =>
          commands
            .sessionAppendTranscript(sessionId, {
              transcript_id: captureTranscriptId,
              new_words: delta.new_words.map((word) => ({
                ...word,
                metadata: { capture_source: "recording" },
              })),
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

      const languages = getTranscriptionLanguages(meetingLanguages);
      const liveTranscriptionConfig = await getLiveTranscriptionConfig({
        provider: conn?.provider,
        model: conn?.model,
        languages,
        timing: transcriptionTiming,
      });

      if (!isAvailable()) return;
      started = await start(
        {
          session_id: sessionId,
          languages: liveTranscriptionConfig.languages,
          onboarding: false,
          model: conn?.model ?? "",
          base_url: conn?.baseUrl ?? "",
          api_key: conn?.apiKey ?? "",
          transcription_mode: conn?.baseUrl.trim()
            ? liveTranscriptionConfig.transcriptionMode
            : "batch",
          participant_human_ids: [],
          self_human_id: session?.user_id || null,
        },
        {
          handlePersist,
          onStopped,
          isAvailable,
          requestId,
        },
      );

      if (!started) {
        await lastTranscriptWrite;
        // A lost start reply cannot prove the producer never wrote these words.
        return;
      }

      if (isAvailable()) setLeftSidebarExpanded(false);
    } finally {
      if (!started) activity.finish();
    }
  }, [
    conn,
    hadTranscriptBeforeStart,
    session,
    sessionId,
    setLeftSidebarExpanded,
    meetingLanguages,
    transcriptionTiming,
    start,
    isAvailable,
    summaryOwner.scope,
    summaryOwner.current,
  ]);

  return startListening;
}
