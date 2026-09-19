import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useMemo, useRef } from "react";

import { TRANSCRIPT_RENDER_CACHE_TIME_MS } from "../cache";
import { useTranscriptRenderData } from "../render-request-hooks";

import type { Person } from "~/people/queries";
import {
  getMaxSpeakerNumberForParticipants,
  type Segment,
} from "~/stt/live-segment";
import type { TranscriptRecord } from "~/stt/queries";
import {
  getRenderTranscriptRequestKey,
  renderRequestHasDiarizedChannel,
  renderTranscriptSegments,
} from "~/stt/render-transcript";

export function useRenderedTranscriptData(
  transcriptId: string,
  transcript: TranscriptRecord | null,
  people: readonly Person[],
) {
  const { request } = useTranscriptRenderData(transcript, people);
  const requestKey = useMemo(
    () => getRenderTranscriptRequestKey(request),
    [request],
  );

  // eslint-disable-next-line @tanstack/query/exhaustive-deps -- requestKey is the canonical hash of the complete render request.
  const query = useQuery({
    queryKey: ["rendered-transcript-segments", transcriptId, requestKey],
    queryFn: async () => {
      if (!request) {
        return [];
      }

      return renderTranscriptSegments(request);
    },
    enabled: !!request,
    // Local IPC render call: never let a misreported offline state pause it.
    networkMode: "always",
    // Keep the previous segments on screen while a changed request re-renders,
    // so a speaker rename doesn't blank the transcript and remount every segment.
    placeholderData: keepPreviousData,
    staleTime: Number.POSITIVE_INFINITY,
    gcTime: TRANSCRIPT_RENDER_CACHE_TIME_MS,
  });

  const maxSpeakerNumber = useMemo(
    () =>
      // Diarization can surface more speakers than the participant list;
      // capping would merge two distinct diarized speakers under one label.
      request && !renderRequestHasDiarizedChannel(request)
        ? getMaxSpeakerNumberForParticipants(
            request.participant_human_ids,
            request.self_human_id,
          )
        : undefined,
    [request],
  );

  const previous = useRef<{ id: string; data: Segment[] } | null>(null);
  if (!request) previous.current = null;
  else if (query.data)
    previous.current = { id: transcriptId, data: query.data };
  const data =
    query.data ??
    (previous.current?.id === transcriptId ? previous.current.data : undefined);
  return {
    maxSpeakerNumber,
    segments: request ? (data ?? []) : [],
    isLoading: Boolean(request) && !data && !query.isError,
    isError: query.isError,
    refetch: query.refetch,
  };
}

export function useTranscriptOffset(
  transcript: TranscriptRecord | null,
  transcripts: readonly TranscriptRecord[],
): number {
  return useMemo(() => {
    if (!transcript) {
      return 0;
    }

    const earliestStartedAt = Math.min(
      ...transcripts.map((current) => current.startedAt),
    );

    return Number.isFinite(earliestStartedAt)
      ? transcript.startedAt - earliestStartedAt
      : 0;
  }, [transcript, transcripts]);
}
