import { useQuery } from "@tanstack/react-query";
import { useMemo, useRef } from "react";

import { TRANSCRIPT_RENDER_CACHE_TIME_MS } from "../cache";
import {
  renderedTranscriptPrefix,
  useTranscriptCacheOwner,
} from "../render-cache";
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
  const owner = useTranscriptCacheOwner();
  const sessionId = transcript?.sessionId ?? "";
  const ownsRequest = Boolean(
    transcript && transcript.id === transcriptId && sessionId,
  );
  const { request } = useTranscriptRenderData(transcript, people);
  const requestKey = useMemo(
    () => getRenderTranscriptRequestKey(request),
    [request],
  );

  // eslint-disable-next-line @tanstack/query/exhaustive-deps -- requestKey is the canonical hash of the complete render request.
  const query = useQuery(
    {
      queryKey: [
        ...renderedTranscriptPrefix(owner.scope, sessionId, transcriptId),
        requestKey,
      ],
      queryFn: async ({ signal }) => {
        owner.assertCurrent(signal);
        if (!request) {
          return [];
        }

        const segments = await renderTranscriptSegments(request);
        owner.assertCurrent(signal);
        return segments;
      },
      enabled: owner.available && ownsRequest && !!request,
      // Local IPC render call: never let a misreported offline state pause it.
      networkMode: "always",
      // Keep the previous segments on screen while a changed request re-renders,
      // so a speaker rename doesn't blank the transcript and remount every segment.
      placeholderData: (data, previousQuery) =>
        owner.isCurrent() &&
        ownsRequest &&
        previousQuery &&
        Object.is(
          owner.scope.queryClient.getQueryCache().get(previousQuery.queryHash),
          previousQuery,
        ) &&
        previousQuery.queryKey[3] === sessionId &&
        previousQuery.queryKey[4] === transcriptId
          ? data
          : undefined,
      staleTime: Number.POSITIVE_INFINITY,
      gcTime: TRANSCRIPT_RENDER_CACHE_TIME_MS,
    },
    owner.scope.queryClient,
  );

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

  const previous = useRef<{
    sessionId: string;
    id: string;
    data: Segment[];
  } | null>(null);
  const available = owner.available && ownsRequest && !!request;
  if (!available) previous.current = null;
  else if (query.data)
    previous.current = { sessionId, id: transcriptId, data: query.data };
  const data =
    query.data ??
    (previous.current?.sessionId === sessionId &&
    previous.current.id === transcriptId
      ? previous.current.data
      : undefined);
  return {
    maxSpeakerNumber,
    segments: available ? (data ?? []) : [],
    isLoading: available && !data && !query.isError,
    isError: available && query.isError,
    refetch: async () => {
      if (owner.isCurrent() && ownsRequest && request) return query.refetch();
    },
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
