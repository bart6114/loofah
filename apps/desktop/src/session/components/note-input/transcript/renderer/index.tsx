import { ArrowDownIcon, ArrowUpIcon } from "lucide-react";
import { type RefObject, useCallback, useRef, useState } from "react";
import { useHotkeys } from "react-hotkeys-hook";

import { cn } from "@hypr/utils";

import { usePlaybackHighlight } from "./playback-highlight";
import { SelectionMenu } from "./selection-menu";
import { TranscriptSeparator } from "./separator";
import { RenderTranscript } from "./transcript";
import {
  useAutoScroll,
  usePlaybackAutoScroll,
  useScrollDetection,
} from "./viewport-hooks";

import { useAudioPlayer } from "~/audio-player";
import { useAudioTime } from "~/audio-player/provider";
import { usePeople } from "~/people/queries";
import type { Segment } from "~/stt/live-segment";
import type { TranscriptRecord } from "~/stt/queries";

const LIVE_TRANSCRIPT_PLACEHOLDER_ID = "__live-transcript__";

export function TranscriptViewer({
  transcripts,
  liveSegments,
  currentActive,
  scrollRef,
}: {
  transcripts: readonly TranscriptRecord[];
  liveSegments: Segment[];
  currentActive: boolean;
  scrollRef: RefObject<HTMLDivElement | null>;
}) {
  const people = usePeople();
  const containerRef = useRef<HTMLDivElement>(null);
  const [scrollElement, setScrollElement] = useState<HTMLDivElement | null>(
    null,
  );
  const handleContainerRef = useCallback(
    (node: HTMLDivElement | null) => {
      containerRef.current = node;
      setScrollElement(node);
      scrollRef.current = node;
    },
    [scrollRef],
  );

  const {
    isAtTop,
    isAtBottom,
    isNearBottom,
    canScroll,
    autoScrollEnabled,
    scrollToTop,
    scrollToBottom,
  } = useScrollDetection(containerRef, currentActive);

  const {
    state: playerState,
    pause,
    resume,
    start,
    seek,
    audioExists,
  } = useAudioPlayer();
  const isPlaying = playerState === "playing";

  useHotkeys(
    "space",
    (e) => {
      e.preventDefault();
      if (playerState === "playing") {
        pause();
      } else if (playerState === "paused") {
        resume();
      } else if (playerState === "stopped") {
        start();
      }
    },
    { enableOnFormTags: false },
  );

  const shouldAutoScroll = currentActive && autoScrollEnabled;
  const shouldScrollLastTranscriptToEnd = currentActive && isNearBottom;
  useAutoScroll(
    containerRef,
    [transcripts, liveSegments, shouldAutoScroll],
    shouldAutoScroll,
  );
  const visibleTranscripts: Array<TranscriptRecord | null> =
    transcripts.length > 0
      ? [...transcripts]
      : liveSegments.length > 0
        ? [null]
        : [];

  const handleSelectionAction = (action: string, selectedText: string) => {
    if (action === "copy") {
      void navigator.clipboard.writeText(selectedText);
    }
  };

  return (
    <div className="relative h-full">
      <div
        ref={handleContainerRef}
        data-transcript-container
        className={cn([
          "flex h-full flex-col gap-8 overflow-x-hidden overflow-y-auto",
          "scrollbar-hide",
          "scroll-pb-[calc(8rem+env(safe-area-inset-bottom))]",
          "pb-[calc(4rem+env(safe-area-inset-bottom))]",
        ])}
      >
        {visibleTranscripts.map((transcript, index) => {
          const transcriptId = transcript?.id ?? LIVE_TRANSCRIPT_PLACEHOLDER_ID;
          return (
            <div key={transcriptId} className="flex flex-col gap-8">
              <RenderTranscript
                scrollElement={scrollElement}
                isLastTranscript={index === visibleTranscripts.length - 1}
                shouldScrollToEnd={shouldScrollLastTranscriptToEnd}
                transcriptId={transcriptId}
                transcript={transcript}
                transcripts={transcripts}
                people={people}
                recording={currentActive}
                liveSegments={
                  index === visibleTranscripts.length - 1 && currentActive
                    ? liveSegments
                    : []
                }
                currentMs={0}
                seek={seek}
                startPlayback={start}
                audioExists={audioExists}
              />
              {index < visibleTranscripts.length - 1 && <TranscriptSeparator />}
            </div>
          );
        })}

        <SelectionMenu
          containerRef={containerRef}
          onAction={handleSelectionAction}
        />
      </div>

      <PlaybackTracking containerRef={containerRef} isPlaying={isPlaying} />
      {canScroll && (
        <div
          data-transcript-scroll-controls
          className={cn([
            "absolute top-1/2 right-1 z-40 flex -translate-y-1/2 flex-col overflow-hidden",
            "border-border/60 bg-muted/70 text-foreground rounded-full border",
          ])}
        >
          <button
            type="button"
            aria-label="Scroll to top"
            onClick={scrollToTop}
            disabled={isAtTop}
            className={cn([
              "flex size-8 items-center justify-center",
              "hover:bg-muted/85 active:bg-muted/85",
              "disabled:pointer-events-none disabled:opacity-30",
            ])}
          >
            <ArrowUpIcon aria-hidden="true" className="size-3.5" />
          </button>
          <div className="bg-border/70 h-px w-full" />
          <button
            type="button"
            aria-label="Scroll to bottom"
            onClick={scrollToBottom}
            disabled={isAtBottom}
            className={cn([
              "flex size-8 items-center justify-center",
              "hover:bg-muted/85 active:bg-muted/85",
              "disabled:pointer-events-none disabled:opacity-30",
            ])}
          >
            <ArrowDownIcon aria-hidden="true" className="size-3.5" />
          </button>
        </div>
      )}
    </div>
  );
}

function PlaybackTracking({
  containerRef,
  isPlaying,
}: {
  containerRef: RefObject<HTMLDivElement | null>;
  isPlaying: boolean;
}) {
  const time = useAudioTime();
  usePlaybackHighlight(containerRef, time.current * 1000);
  usePlaybackAutoScroll(containerRef, time.current * 1000, isPlaying);
  return null;
}
