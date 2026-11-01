import type { DegradedError } from "@hypr/plugin-transcription";

import { useAudioPlayer } from "~/audio-player";
import { getLiveCaptureUiMode } from "~/store/zustand/listener/general-shared";
import { useListener } from "~/stt/contexts";
import type { Segment } from "~/stt/live-segment";
import type { TranscriptRecord } from "~/stt/queries";

type ListeningStatus = "listening" | "finalizing";
type BatchPhase = "importing" | "transcribing";
type RequestedLiveTranscription = boolean | null;

export type TranscriptScreen =
  | {
      kind: "running_batch";
      percentage?: number;
      phase?: BatchPhase;
    }
  | {
      kind: "batch_fallback";
      requestedLiveTranscription: RequestedLiveTranscription;
      error: DegradedError | null;
    }
  | {
      kind: "listening";
      status: ListeningStatus;
    }
  | {
      kind: "empty";
      hasAudio: boolean;
      error: string | null;
    }
  | {
      kind: "ready";
      transcripts: readonly TranscriptRecord[];
      liveSegments: Segment[];
      currentActive: boolean;
    };

export function useTranscriptScreen({
  sessionId,
  transcripts,
}: {
  sessionId: string;
  transcripts: readonly TranscriptRecord[];
}): TranscriptScreen {
  const sessionMode = useListener((state) => state.getSessionMode(sessionId));
  const batchError = useListener(
    (state) => state.batch[sessionId]?.error ?? null,
  );
  const batchProgress = useListener((state) => state.batch[sessionId] ?? null);
  const live = useListener((state) => state.live);
  const { audioExists } = useAudioPlayer();

  const { liveSegments, hasTranscriptWords } =
    useTranscriptContent(transcripts);

  const currentActive =
    sessionMode === "active" || sessionMode === "finalizing";
  const captureMode = getLiveCaptureUiMode(live);
  const isRecordOnlyMode = sessionMode === "active" && captureMode !== "live";
  const hasTranscriptContent = hasTranscriptWords || liveSegments.length > 0;

  if (sessionMode === "running_batch") {
    return {
      kind: "running_batch",
      percentage: batchProgress?.percentage,
      phase: batchProgress?.phase,
    };
  }

  if (isRecordOnlyMode) {
    return {
      kind: "batch_fallback",
      requestedLiveTranscription: live.requestedLiveTranscription,
      error: live.degraded,
    };
  }

  if (currentActive && !hasTranscriptContent && !batchError) {
    return {
      kind: "listening",
      status: sessionMode === "finalizing" ? "finalizing" : "listening",
    };
  }

  if (!hasTranscriptContent) {
    return {
      kind: "empty",
      hasAudio: audioExists,
      error: batchError,
    };
  }

  return {
    kind: "ready",
    transcripts,
    liveSegments,
    currentActive,
  };
}

function useTranscriptContent(transcripts: readonly TranscriptRecord[]) {
  const liveSegments = useListener((state) => state.liveSegments);

  return {
    liveSegments,
    hasTranscriptWords: transcripts.some(
      (transcript) => transcript.words.length > 0,
    ),
  };
}
