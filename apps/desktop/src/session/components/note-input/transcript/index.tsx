import type { RefObject } from "react";
import { useCallback, useEffect, useState } from "react";

import { Spinner } from "@hypr/ui/components/ui/spinner";

import { useRegenerateTranscript } from "./actions";
import { TranscriptViewer } from "./renderer";
import { BatchState } from "./screens/batch";
import { TranscriptEmptyState } from "./screens/empty";
import { TranscriptListeningState } from "./screens/listening";
import { useTranscriptScreen } from "./state";

import { useListener } from "~/stt/contexts";
import type { TranscriptRecord } from "~/stt/queries";
import { useUploadFile } from "~/stt/useUploadFile";

export function Transcript({
  sessionId,
  transcripts,
  scrollRef,
}: {
  sessionId: string;
  transcripts: readonly TranscriptRecord[];
  scrollRef: RefObject<HTMLDivElement | null>;
}) {
  const screen = useTranscriptScreen({ sessionId, transcripts });
  const { uploadAudio, uploadTranscript } = useUploadFile(sessionId);
  const regenerateTranscript = useRegenerateTranscript(sessionId);
  const stopTranscription = useListener((state) => state.stopTranscription);
  const [viewerReady, setViewerReady] = useState(false);
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
      {screen.kind === "listening" && (
        <TranscriptListeningState status={screen.status} />
      )}
      {screen.kind === "empty" && (
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

function TranscriptLoadingState() {
  return (
    <div
      role="status"
      className="text-muted-foreground flex h-full items-center justify-center gap-2 text-sm"
    >
      <Spinner size={18} />
      <span>Loading transcript...</span>
    </div>
  );
}
