import { memo, useCallback, useEffect, useMemo } from "react";

import { cn } from "@hypr/utils";

import { useSearch } from "../../search/context";
import { TranscriptLoadingState, TranscriptLoadError } from "../loading";
import { useRenderedTranscriptData, useTranscriptOffset } from "./data-hooks";
import {
  EMPTY_TRANSCRIPT_SEARCH,
  SegmentRenderer,
  type TranscriptSearchRenderState,
} from "./segment";
import {
  createSegmentKey,
  segmentsShallowEqual,
  useStableSegments,
} from "./segment-hooks";
import {
  buildChannelAssignmentStates,
  EMPTY_CHANNEL_ASSIGNMENT_STATE,
} from "./speaker-assign";

import type { Person } from "~/people/queries";
import {
  mergeRenderedAndLiveSegments,
  SegmentKeyUtils,
  type Segment,
  type SegmentWord,
} from "~/stt/live-segment";
import {
  createTranscriptLabelContext,
  type TranscriptRecord,
} from "~/stt/queries";
import { SpeakerLabelManager } from "~/stt/segment/shared";
import { isTranscriptWordSeekable } from "~/stt/timing";

export function RenderTranscript({
  scrollElement,
  isLastTranscript,
  shouldScrollToEnd,
  transcriptId,
  transcript,
  transcripts,
  people,
  recording,
  liveSegments,
  currentMs,
  seek,
  startPlayback,
  audioExists,
}: {
  scrollElement: HTMLDivElement | null;
  isLastTranscript: boolean;
  shouldScrollToEnd: boolean;
  transcriptId: string;
  transcript: TranscriptRecord | null;
  transcripts: readonly TranscriptRecord[];
  people: readonly Person[];
  recording: boolean;
  liveSegments: Segment[];
  currentMs: number;
  seek: (sec: number) => void;
  startPlayback: () => void;
  audioExists: boolean;
}) {
  const {
    maxSpeakerNumber,
    segments: storedSegments,
    isLoading,
    isError,
    refetch,
  } = useRenderedTranscriptData(transcriptId, transcript, people);
  const mergedSegments = useMemo(
    () => mergeRenderedAndLiveSegments(storedSegments, liveSegments),
    [liveSegments, storedSegments],
  );
  const segments = useStableSegments(mergedSegments);
  const offsetMs = useTranscriptOffset(transcript, transcripts);

  if (segments.length === 0 && isLoading) return <TranscriptLoadingState />;
  if (segments.length === 0 && !isError) return null;

  return (
    <>
      {isError && (
        <TranscriptLoadError
          retry={() => {
            void refetch();
          }}
        />
      )}
      <SegmentsList
        segments={segments}
        scrollElement={scrollElement}
        transcriptId={transcriptId}
        offsetMs={offsetMs}
        shouldScrollToEnd={isLastTranscript && shouldScrollToEnd}
        currentMs={currentMs}
        seek={seek}
        startPlayback={startPlayback}
        audioExists={audioExists}
        maxSpeakerNumber={maxSpeakerNumber}
        transcript={transcript}
        people={people}
        recording={recording}
      />
    </>
  );
}

const SegmentsList = memo(
  ({
    segments,
    scrollElement,
    transcriptId,
    offsetMs,
    shouldScrollToEnd,
    currentMs,
    seek,
    startPlayback,
    audioExists,
    maxSpeakerNumber,
    transcript,
    people,
    recording,
  }: {
    segments: Segment[];
    scrollElement: HTMLDivElement | null;
    transcriptId: string;
    offsetMs: number;
    shouldScrollToEnd: boolean;
    currentMs: number;
    seek: (sec: number) => void;
    startPlayback: () => void;
    audioExists: boolean;
    maxSpeakerNumber?: number;
    transcript: TranscriptRecord | null;
    people: readonly Person[];
    recording: boolean;
  }) => {
    const labelContext = useMemo(
      () => createTranscriptLabelContext(transcript, people),
      [people, transcript],
    );
    const channelAssignmentStates = useMemo(
      () => buildChannelAssignmentStates(transcript),
      [transcript],
    );
    const search = useSearch();
    const speakerLabelManager = useMemo(() => {
      return labelContext
        ? SpeakerLabelManager.fromSegments(
            segments,
            labelContext,
            maxSpeakerNumber,
          )
        : new SpeakerLabelManager();
    }, [labelContext, maxSpeakerNumber, segments]);
    const transcriptSearch = useMemo<TranscriptSearchRenderState>(() => {
      const query = search?.query.trim() ?? "";
      if (!search?.isVisible || !query) {
        return EMPTY_TRANSCRIPT_SEARCH;
      }

      return {
        query,
        activeMatchId: search.activeMatchId,
        caseSensitive: search.caseSensitive,
        wholeWord: search.wholeWord,
      };
    }, [
      search?.activeMatchId,
      search?.caseSensitive,
      search?.isVisible,
      search?.query,
      search?.wholeWord,
    ]);

    const seekAndPlay = useCallback(
      (word: SegmentWord) => {
        if (audioExists && isTranscriptWordSeekable(word)) {
          seek((offsetMs + word.start_ms) / 1000);
          startPlayback();
        }
      },
      [audioExists, offsetMs, seek, startPlayback],
    );

    useEffect(() => {
      if (!scrollElement || !shouldScrollToEnd) {
        return;
      }
      const raf = requestAnimationFrame(() => {
        scrollElement.scrollTo({
          top: scrollElement.scrollHeight,
          behavior: "auto",
        });
      });
      return () => cancelAnimationFrame(raf);
    }, [scrollElement, segments.length, shouldScrollToEnd]);

    return (
      <div>
        {segments.map((segment, index) => (
          <div
            key={createSegmentKey(segment, transcriptId, index)}
            className={cn([
              index > 0 && "pt-4",
              "[contain-intrinsic-size:auto_96px] [content-visibility:auto]",
            ])}
          >
            <SegmentRenderer
              segment={segment}
              offsetMs={offsetMs}
              transcriptId={transcriptId}
              speakerLabel={SegmentKeyUtils.renderLabel(
                segment.key,
                labelContext,
                speakerLabelManager,
              )}
              people={people}
              channelAssignmentState={
                channelAssignmentStates.get(segment.key.channel) ??
                EMPTY_CHANNEL_ASSIGNMENT_STATE
              }
              recording={recording}
              currentMs={currentMs}
              seekAndPlay={seekAndPlay}
              audioExists={audioExists}
              search={transcriptSearch}
            />
          </div>
        ))}
      </div>
    );
  },
  (prevProps, nextProps) => {
    return (
      prevProps.transcriptId === nextProps.transcriptId &&
      prevProps.scrollElement === nextProps.scrollElement &&
      prevProps.offsetMs === nextProps.offsetMs &&
      prevProps.shouldScrollToEnd === nextProps.shouldScrollToEnd &&
      prevProps.currentMs === nextProps.currentMs &&
      prevProps.audioExists === nextProps.audioExists &&
      prevProps.maxSpeakerNumber === nextProps.maxSpeakerNumber &&
      prevProps.transcript === nextProps.transcript &&
      prevProps.people === nextProps.people &&
      prevProps.recording === nextProps.recording &&
      prevProps.seek === nextProps.seek &&
      prevProps.startPlayback === nextProps.startPlayback &&
      segmentsShallowEqual(prevProps.segments, nextProps.segments)
    );
  },
);
