import { useMemo } from "react";

import type { RenderTranscriptRequest } from "@hypr/plugin-transcription";

import { type Person, usePeople } from "~/people/queries";
import {
  type TranscriptRecord,
  useSessionTranscriptsQuery,
} from "~/stt/queries";
import {
  buildRenderTranscriptRequestFromRows,
  type TranscriptRow,
} from "~/stt/render-transcript";

export type TranscriptRowWithId = {
  transcriptId: string;
  row: TranscriptRow;
};

export function useTranscriptRenderData(
  transcript: TranscriptRecord | null,
  people: readonly Person[],
): {
  request: RenderTranscriptRequest | null;
  transcriptRows: TranscriptRowWithId[];
} {
  const transcripts = useMemo(
    () => (transcript ? [transcript] : emptyTranscripts),
    [transcript],
  );

  return useRenderData(transcripts, people);
}

export function useSessionTranscriptRenderData(sessionId: string) {
  const query = useSessionTranscriptsQuery(sessionId);
  const people = usePeople();
  const renderData = useRenderData(query.data ?? emptyTranscripts, people);
  return { ...renderData, query };
}

function useRenderData(
  transcripts: readonly TranscriptRecord[],
  people: readonly Person[],
): {
  request: RenderTranscriptRequest | null;
  transcriptRows: TranscriptRowWithId[];
} {
  const selfHumanId = transcripts[0]?.ownerUserId;

  const transcriptRows = useMemo(() => {
    return transcripts.map((transcript) => ({
      transcriptId: transcript.id,
      row: {
        started_at: transcript.startedAt,
        words: transcript.words,
        speaker_hints: transcript.speakerHints,
      },
    }));
  }, [transcripts]);

  const humans = useMemo(
    () => people.map((person) => ({ human_id: person.id, name: person.name })),
    [people],
  );

  const request = useMemo(
    () =>
      buildRenderTranscriptRequestFromRows(
        transcriptRows.map((transcriptRow) => transcriptRow.row),
        { humans, selfHumanId },
      ),
    [humans, selfHumanId, transcriptRows],
  );

  return { request, transcriptRows };
}

const emptyTranscripts: TranscriptRecord[] = [];
