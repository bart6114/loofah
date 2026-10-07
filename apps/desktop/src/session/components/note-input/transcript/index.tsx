import { useQueryClient } from "@tanstack/react-query";
import type { RefObject } from "react";
import { useCallback, useEffect, useState } from "react";

import { useRegenerateTranscript } from "./actions";
import { TranscriptLoadingState, TranscriptLoadError } from "./loading";
import { TranscriptViewer } from "./renderer";
import { BatchState } from "./screens/batch";
import { TranscriptEmptyState } from "./screens/empty";
import { TranscriptListeningState } from "./screens/listening";
import { useTranscriptScreen } from "./state";

import { useListener } from "~/stt/contexts";
import {
  type TranscriptRecord,
  useSessionTranscriptsQuery,
} from "~/stt/queries";
import { useUploadFile } from "~/stt/useUploadFile";

const EMPTY_TRANSCRIPTS: TranscriptRecord[] = [];

export function Transcript({
  sessionId,
  scrollRef,
}: {
  sessionId: string;
  scrollRef: RefObject<HTMLDivElement | null>;
}) {
  const client = useQueryClient();
  const query = useSessionTranscriptsQuery(sessionId);
  const cached =
    Boolean(query.data?.length) &&
    query.data!.every((transcript) =>
      client
        .getQueriesData({
          queryKey: ["rendered-transcript-segments", transcript.id],
        })
        .some(([, data]) => data !== undefined),
    );
  return (
    <TranscriptContent
      key={sessionId}
      sessionId={sessionId}
      scrollRef={scrollRef}
      transcripts={query.data ?? EMPTY_TRANSCRIPTS}
      initiallyReady={cached}
      pending={query.isPending}
      error={query.isError}
      retry={() => {
        void query.refetch();
      }}
    />
  );
}

export function TranscriptContent({
  sessionId,
  transcripts,
  initiallyReady = false,
  pending = false,
  error = false,
  retry = () => {},
  scrollRef,
}: {
  sessionId: string;
  transcripts: readonly TranscriptRecord[];
  initiallyReady?: boolean;
  pending?: boolean;
  error?: boolean;
  retry?: () => void;
  scrollRef: RefObject<HTMLDivElement | null>;
}) {
  const screen = useTranscriptScreen({ sessionId, transcripts });
  const { uploadAudio, uploadTranscript } = useUploadFile(sessionId);
  const regenerateTranscript = useRegenerateTranscript(sessionId);
  const stopTranscription = useListener((state) => state.stopTranscription);
  const [viewerReady, setViewerReady] = useState(initiallyReady);
  useEffect(() => {
    if (screen.kind !== "ready") {
      setViewerReady(false);
      return;
    }

    let renderFrame: number | undefined;
    // WebKit can suspend paint callbacks in an occluded window; content loading
    // must still make progress when no animation frame arrives.
    const fallback = setTimeout(() => setViewerReady(true), 100);
    const loadingFrame = requestAnimationFrame(() => {
      renderFrame = requestAnimationFrame(() => {
        clearTimeout(fallback);
        setViewerReady(true);
      });
    });

    return () => {
      clearTimeout(fallback);
      cancelAnimationFrame(loadingFrame);
      if (renderFrame !== undefined) {
        cancelAnimationFrame(renderFrame);
      }
    };
  }, [screen.kind]);
  const handleStopTranscription = useCallback(() => {
    void stopTranscription(sessionId);
  }, [sessionId, stopTranscription]);

  return (
    <div className="relative flex h-full flex-col overflow-hidden">
      {screen.kind === "running_batch" && (
        <TranscriptEmptyState
          isBatching
          percentage={screen.percentage}
          phase={screen.phase}
          onStopTranscription={
            screen.phase === "importing" ? undefined : handleStopTranscription
          }
        />
      )}
      {screen.kind === "batch_fallback" && (
        <BatchState
          requestedLiveTranscription={screen.requestedLiveTranscription}
          error={screen.error}
        />
      )}
      {!pending && screen.kind === "listening" && (
        <TranscriptListeningState status={screen.status} />
      )}
      {error && <TranscriptLoadError retry={retry} />}
      {pending &&
        screen.kind !== "ready" &&
        screen.kind !== "running_batch" &&
        screen.kind !== "batch_fallback" && <TranscriptLoadingState />}
      {!pending && !error && screen.kind === "empty" && (
        <TranscriptEmptyState
          isBatching={false}
          hasAudio={screen.hasAudio}
          error={screen.error}
          onRetranscribe={regenerateTranscript}
          onUploadAudio={uploadAudio}
          onUploadTranscript={uploadTranscript}
        />
      )}
      {screen.kind === "ready" && !viewerReady && <TranscriptLoadingState />}
      {screen.kind === "ready" && viewerReady && (
        <TranscriptViewer
          transcripts={screen.transcripts}
          liveSegments={screen.liveSegments}
          currentActive={screen.currentActive}
          scrollRef={scrollRef}
        />
      )}
    </div>
  );
}
