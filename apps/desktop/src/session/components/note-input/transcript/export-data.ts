import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";

import type { TranscriptItem } from "@hypr/plugin-export";
import type { RenderTranscriptRequest } from "@hypr/plugin-transcription";

import { TRANSCRIPT_RENDER_CACHE_TIME_MS } from "./cache";
import { transcriptExportKey, useTranscriptCacheOwner } from "./render-cache";
import { useSessionTranscriptRenderData } from "./render-request-hooks";

import {
  getRenderTranscriptRequestKey,
  renderTranscriptSegments,
} from "~/stt/render-transcript";

export type TranscriptExportSegment = TranscriptItem & {
  start_ms: number;
  end_ms: number;
};

export async function buildTranscriptExportSegments(
  request: RenderTranscriptRequest,
): Promise<TranscriptExportSegment[]> {
  const segments = await renderTranscriptSegments(request);

  return segments.map((segment) => ({
    text: segment.text,
    start_ms: segment.start_ms,
    end_ms: segment.end_ms,
    speaker: segment.speaker_label,
  }));
}

export function useTranscriptExportSegments(sessionId: string) {
  const owner = useTranscriptCacheOwner();
  const {
    request,
    transcriptRows,
    query: transcripts,
  } = useSessionTranscriptRenderData(sessionId);
  const requestKey = useMemo(
    () => getRenderTranscriptRequestKey(request),
    [request],
  );

  const transcriptIds = transcriptRows.map((row) => row.transcriptId);
  const ownsRequest = Boolean(
    sessionId &&
    transcriptIds.length &&
    transcripts.data?.every((transcript) => transcript.sessionId === sessionId),
  );

  // eslint-disable-next-line @tanstack/query/exhaustive-deps -- requestKey is the canonical hash of the complete render request.
  const rendered = useQuery(
    {
      queryKey: transcriptExportKey(
        owner.scope,
        sessionId,
        transcriptIds,
        requestKey,
      ),
      queryFn: async ({ signal }) => {
        owner.assertCurrent(signal);
        if (!request) {
          return [];
        }
        const segments = await buildTranscriptExportSegments(request);
        owner.assertCurrent(signal);
        return segments;
      },
      enabled: owner.available && ownsRequest && !!request,
      networkMode: "always",
      staleTime: Number.POSITIVE_INFINITY,
      gcTime: TRANSCRIPT_RENDER_CACHE_TIME_MS,
    },
    owner.scope.queryClient,
  );
  const available = owner.available && ownsRequest && !!request;

  return {
    data: available ? (rendered.data ?? []) : [],
    isLoading:
      owner.available &&
      (transcripts.isPending || (available && rendered.isPending)),
    error: owner.available
      ? (transcripts.error ?? (available ? rendered.error : null))
      : null,
    refetch: () => {
      if (!owner.isCurrent()) return;
      if (transcripts.isError) void transcripts.refetch();
      else if (ownsRequest && request) void rendered.refetch();
    },
  };
}

export function formatTranscriptExportSegments(
  segments: Array<{ speaker: string | null; text: string }>,
) {
  return segments
    .map((segment) => `${segment.speaker ?? "Speaker"}: ${segment.text}`)
    .join("\n\n");
}
