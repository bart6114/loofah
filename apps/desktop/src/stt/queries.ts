import { useMemo } from "react";

import { type Person, usePeople } from "~/people/queries";
import { useIndexQuery } from "~/shared/index-query";
import { DEFAULT_USER_ID } from "~/shared/utils";
import { enqueueDatabaseWrite } from "~/shared/write-queue";
import type { RenderLabelContext, SegmentKey } from "~/stt/live-segment";
import {
  collectAssignedHumanIdsFromTranscriptRows,
  type TranscriptRow,
} from "~/stt/render-transcript";
import type { SpeakerHintWithId, WordWithId } from "~/stt/types";
import { createTranscriptAccumulator } from "~/stt/utils";
import type {
  TranscriptSpeakerHint,
  TranscriptWithData,
  TranscriptWord,
} from "~/types/tauri.gen";
import { commands } from "~/types/tauri.gen";

type TranscriptInsert = {
  id: string;
  sessionId: string;
  ownerUserId: string;
  createdAt: string;
  startedAt: number;
  endedAt?: number;
  memo?: string;
  source?: string;
  provider?: string;
  model?: string;
  language?: string;
  words?: WordWithId[];
  speakerHints?: SpeakerHintWithId[];
  replaceSession?: boolean;
};

export type TranscriptRecord = {
  id: string;
  ownerUserId: string;
  sessionId: string;
  startedAt: number;
  endedAt?: number;
  words: NonNullable<TranscriptRow["words"]>;
  speakerHints: NonNullable<TranscriptRow["speaker_hints"]>;
};

const EMPTY_TRANSCRIPTS: TranscriptRecord[] = [];
const EMPTY_IDS: string[] = [];

export function useSessionTranscripts(sessionId: string): TranscriptRecord[] {
  const { data = EMPTY_TRANSCRIPTS } = useIndexQuery({
    // Transcript events carry the session id. session_transcripts is already
    // ordered (started_at, id).
    entity: "transcripts",
    ids: [sessionId],
    queryKey: ["session-transcripts", sessionId],
    queryFn: async () => {
      const result = await commands.sessionTranscripts(sessionId);
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return result.data.map(mapTranscript);
    },
    enabled: Boolean(sessionId),
  });
  return sessionId ? data : EMPTY_TRANSCRIPTS;
}

export function useTranscript(transcriptId: string): TranscriptRecord | null {
  const { data = null } = useIndexQuery({
    // Transcript events carry session ids and the owning session isn't known
    // here, so this one stays table-level.
    entity: "transcripts",
    queryKey: ["transcript", transcriptId],
    queryFn: async () => {
      const result = await commands.transcriptGet(transcriptId);
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return result.data ? mapTranscript(result.data) : null;
    },
    enabled: Boolean(transcriptId),
  });
  return transcriptId ? data : null;
}

export function useTranscriptLabelContext(
  transcriptId: string,
): RenderLabelContext | undefined {
  const transcript = useTranscript(transcriptId);
  const people = usePeople();
  return useMemo(
    () => createTranscriptLabelContext(transcript, people),
    [people, transcript],
  );
}

export function createTranscriptLabelContext(
  transcript: TranscriptRecord | null,
  people: readonly Person[],
): RenderLabelContext | undefined {
  if (!transcript) return undefined;

  const assignedSpeakerLabels = collectAssignedHumanIdsFromTranscriptRows([
    { speaker_hints: transcript.speakerHints },
  ]);
  const nameById = new Map(people.map((person) => [person.id, person.name]));
  const labels = new Set(assignedSpeakerLabels);
  return {
    getSelfHumanId: () => transcript.ownerUserId || undefined,
    getHumanName: (speakerLabel) =>
      nameById.get(speakerLabel) ??
      (labels.has(speakerLabel) ? speakerLabel : undefined),
    getParticipantHumanIds: () => EMPTY_IDS,
  };
}

// `source`/`provider`/`model`/`language` are accepted for caller compatibility but not yet
// persisted -- the store's `TranscriptWithData` shape (Tasks 6-8) has no columns for them.
export function createTranscript(input: TranscriptInsert): Promise<void> {
  return enqueueDatabaseWrite(`transcript:${input.id}`, async () => {
    const transcript = {
      id: input.id,
      user_id: input.ownerUserId,
      created_at: input.createdAt,
      session_id: input.sessionId,
      started_at: input.startedAt,
      ended_at: input.endedAt ?? null,
      memo_md: input.memo ?? "",
      words: (input.words ?? []).map(toTranscriptWord),
      speaker_hints: (input.speakerHints ?? []).map(toTranscriptSpeakerHint),
    };

    if (input.replaceSession) {
      // The store's supersede primitive: the new transcript replaces the session's whole
      // set (batch re-run must not show old and new side by side). The previous
      // transcript.json lands in `.trash/` and superseded index rows are removed.
      const result = await commands.sessionReplaceTranscripts(
        input.sessionId,
        transcript,
      );
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return;
    }

    await writeTranscriptOrThrow(input.sessionId, transcript);
  });
}

export function appendTranscriptWordsAndHints(
  transcriptId: string,
  words: WordWithId[],
  hints: SpeakerHintWithId[],
  options?: { mode?: "append" | "replace" },
): Promise<void> {
  return mutateTranscript(transcriptId, (store) => {
    const accumulator = createTranscriptAccumulator(store, transcriptId);
    accumulator.appendWordsAndHints(words, hints, options);
    accumulator.dispose();
  });
}

// Hints-only Rust command: the store rewrites `speaker_hints` and passes the words
// through untouched, so a rename never ships the full transcript over IPC (and can
// never mangle per-word metadata the way the old read-modify-write path did).
// Shares the per-transcript queue with the append path to keep write ordering.
export function assignTranscriptSpeaker({
  transcriptId,
  segmentKey,
  speakerLabel,
  anchorWordId,
}: {
  transcriptId: string;
  segmentKey: SegmentKey;
  speakerLabel: string;
  anchorWordId: string;
}): Promise<void> {
  return enqueueDatabaseWrite(`transcript:${transcriptId}`, async () => {
    const channel =
      segmentKey.channel === "DirectMic"
        ? 0
        : segmentKey.channel === "RemoteParty"
          ? 1
          : 2;
    const result = await commands.sessionAssignTranscriptSpeaker(
      transcriptId,
      channel,
      typeof segmentKey.speaker_index === "number"
        ? segmentKey.speaker_index
        : null,
      speakerLabel,
      anchorWordId,
    );
    if (result.status === "error") {
      throw new Error(result.error);
    }
  });
}

// Zeroes the transcript's content via a full overwrite rather than truly deleting the row --
// the store has no per-transcript delete, only per-session (`sessionDelete`). An empty
// `words_json` reads the same as "no transcript" everywhere the index is queried
// (`useSessionHasTranscript` etc).
export function softDeleteTranscript(
  sessionId: string,
  transcriptId: string,
): Promise<void> {
  return enqueueDatabaseWrite(`transcript:${transcriptId}`, () =>
    writeTranscriptOrThrow(sessionId, {
      id: transcriptId,
      session_id: sessionId,
      words: [],
      speaker_hints: [],
    }),
  );
}

async function writeTranscriptOrThrow(
  sessionId: string,
  transcript: Parameters<typeof commands.sessionWriteTranscript>[1],
): Promise<void> {
  const result = await commands.sessionWriteTranscript(sessionId, transcript);
  if (result.status === "error") {
    throw new Error(result.error);
  }
}

// `WordWithId`/`SpeakerHintWithId` are TinyBase storage types: every field is typed
// `T | undefined` regardless of whether the zod schema marks it optional, because storage
// cells can be genuinely absent at runtime. The Rust `TranscriptWord`/`TranscriptSpeakerHint`
// types are stricter (`text`/`start_ms`/`end_ms`/`channel`/`word_id` are required) -- default
// missing values rather than reject the word outright, so a partially-populated live word
// still lands on disk instead of silently vanishing from the transcript.
function toTranscriptWord(word: WordWithId): TranscriptWord {
  return {
    id: word.id ?? null,
    text: word.text ?? "",
    start_ms: word.start_ms ?? 0,
    end_ms: word.end_ms ?? 0,
    channel: word.channel ?? 0,
    speaker: word.speaker ?? null,
    metadata: parseMetadata(word.metadata),
  };
}

// `WordWithId.metadata` has two live shapes: TinyBase-heritage callers (useRunBatch)
// still hand in the JSON-stringified storage representation, while words read back
// from the Rust store (`transcriptGet` in `mutateTranscript`) arrive already parsed.
// Passing an object to JSON.parse would coerce it to "[object Object]", throw, and
// silently wipe every word's metadata — the renderer then loses the
// `timing.source: synthetic_speech` flag and shreds batch transcripts.
function parseMetadata(
  metadata: string | TranscriptWord["metadata"] | undefined,
): TranscriptWord["metadata"] {
  if (!metadata) return null;
  if (typeof metadata === "object") {
    return Array.isArray(metadata) ? null : metadata;
  }
  try {
    return JSON.parse(metadata) as TranscriptWord["metadata"];
  } catch {
    return null;
  }
}

function toTranscriptSpeakerHint(
  hint: SpeakerHintWithId,
): TranscriptSpeakerHint {
  return {
    id: hint.id ?? null,
    word_id: hint.word_id ?? "",
    type: hint.type ?? "",
    value: hint.value ?? null,
  };
}

// `words`/`speaker_hints` arrive as parsed objects from the store command -- no
// JSON parsing here, unlike the SQL era's `*_json` columns.
function mapTranscript(transcript: TranscriptWithData): TranscriptRecord {
  return {
    id: transcript.id,
    // The owner concept died with the workspaces removal (D10).
    ownerUserId: transcript.user_id || DEFAULT_USER_ID,
    sessionId: transcript.session_id,
    startedAt: transcript.started_at ?? 0,
    endedAt: transcript.ended_at ?? undefined,
    words: transcript.words ?? [],
    speakerHints: transcript.speaker_hints ?? [],
  };
}

// `enqueueDatabaseWrite`'s per-`transcriptId` queue already serializes every caller in this
// module (append/speaker-rename), so the read-compute-write below no longer needs its own
// compare-and-swap -- the old SQL CAS guarded against concurrent *SQL* writers, and
// `write_transcript` doesn't have an equivalent primitive to CAS against. A live-recording
// debounced flush can still race this from the Rust side (out of this queue's reach); see
// `write_transcript`'s doc for the one direction that's guarded (batch-supersedes-buffer).
async function mutateTranscript(
  transcriptId: string,
  mutation: (store: MemoryTranscriptStore) => void,
): Promise<void> {
  return enqueueDatabaseWrite(`transcript:${transcriptId}`, async () => {
    const read = await commands.transcriptGet(transcriptId);
    if (read.status === "error") {
      throw new Error(read.error);
    }
    const current = read.data;
    if (!current) {
      throw new Error(`Transcript ${transcriptId} does not exist`);
    }

    // The accumulator/assignment utilities are JSON-string based (TinyBase storage
    // heritage); the store hands us parsed arrays, so round-trip through strings --
    // the same payload shape the old `words_json` column held.
    const next = mutateTranscriptSnapshot(
      JSON.stringify(current.words ?? []),
      JSON.stringify(current.speaker_hints ?? []),
      transcriptId,
      mutation,
    );

    await writeTranscriptOrThrow(current.session_id, {
      id: transcriptId,
      session_id: current.session_id,
      started_at: current.started_at,
      words: (JSON.parse(next.wordsJson) as WordWithId[]).map(toTranscriptWord),
      speaker_hints: (JSON.parse(next.hintsJson) as SpeakerHintWithId[]).map(
        toTranscriptSpeakerHint,
      ),
    });
  });
}

type MemoryTranscriptStore = {
  getCell: (
    tableId: "transcripts",
    rowId: string,
    cellId: "words" | "speaker_hints",
  ) => string;
  setCell: (
    tableId: "transcripts",
    rowId: string,
    cellId: "words" | "speaker_hints",
    value: string,
  ) => void;
};

function mutateTranscriptSnapshot(
  wordsJson: string,
  hintsJson: string,
  transcriptId: string,
  mutation: (store: MemoryTranscriptStore) => void,
) {
  const snapshot = { wordsJson, hintsJson };
  const store: MemoryTranscriptStore = {
    getCell: (_tableId, rowId, cellId) => {
      if (rowId !== transcriptId) return "[]";
      return cellId === "words" ? snapshot.wordsJson : snapshot.hintsJson;
    },
    setCell: (_tableId, rowId, cellId, value) => {
      if (rowId !== transcriptId) return;
      if (cellId === "words") {
        snapshot.wordsJson = value;
      } else {
        snapshot.hintsJson = value;
      }
    },
  };

  mutation(store);
  return snapshot;
}
