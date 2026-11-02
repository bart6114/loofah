import type { LiveTranscriptDelta } from "@hypr/plugin-transcription";

import type { SpeakerHintWithId, WordWithId } from "./types";

interface TranscriptStore {
  getCell(
    tableId: "transcripts",
    rowId: string,
    cellId: "words" | "speaker_hints",
  ): unknown;
  setCell(
    tableId: "transcripts",
    rowId: string,
    cellId: "words" | "speaker_hints",
    value: string,
  ): void;
}

const dirtyAccumulatorTranscriptIds = new Set<string>();
const activeAccumulatorCounts = new Map<string, number>();

type TranscriptAccumulatorInitialState = {
  words: WordWithId[];
  hints: SpeakerHintWithId[];
};

export function parseTranscriptWords(
  store: TranscriptStore,
  transcriptId: string,
): WordWithId[] {
  const wordsJson = store.getCell("transcripts", transcriptId, "words");
  if (typeof wordsJson !== "string" || !wordsJson) {
    return [];
  }

  try {
    return JSON.parse(wordsJson) as WordWithId[];
  } catch {
    return [];
  }
}

export function parseTranscriptHints(
  store: TranscriptStore,
  transcriptId: string,
): SpeakerHintWithId[] {
  const hintsJson = store.getCell("transcripts", transcriptId, "speaker_hints");
  if (typeof hintsJson !== "string" || !hintsJson) {
    return [];
  }

  try {
    return JSON.parse(hintsJson) as SpeakerHintWithId[];
  } catch {
    return [];
  }
}

export function updateTranscriptWords(
  store: TranscriptStore,
  transcriptId: string,
  words: WordWithId[],
): void {
  store.setCell("transcripts", transcriptId, "words", JSON.stringify(words));
}

export function updateTranscriptHints(
  store: TranscriptStore,
  transcriptId: string,
  hints: SpeakerHintWithId[],
): void {
  writeTranscriptHints(store, transcriptId, hints);
  markTranscriptAccumulatorDirty(transcriptId);
}

function writeTranscriptHints(
  store: TranscriptStore,
  transcriptId: string,
  hints: SpeakerHintWithId[],
): void {
  store.setCell(
    "transcripts",
    transcriptId,
    "speaker_hints",
    JSON.stringify(hints),
  );
}

export function createTranscriptAccumulator(
  store: TranscriptStore,
  transcriptId: string,
  initialState?: TranscriptAccumulatorInitialState,
): TranscriptAccumulator {
  return new TranscriptAccumulator(store, transcriptId, initialState);
}

export class TranscriptAccumulator {
  private words: WordWithId[];
  private hints: SpeakerHintWithId[];
  private disposed = false;

  constructor(
    private readonly store: TranscriptStore,
    private readonly transcriptId: string,
    initialState?: TranscriptAccumulatorInitialState,
  ) {
    this.words = initialState
      ? [...initialState.words]
      : parseTranscriptWords(store, transcriptId);
    this.hints = initialState
      ? [...initialState.hints]
      : parseTranscriptHints(store, transcriptId);

    activeAccumulatorCounts.set(
      transcriptId,
      (activeAccumulatorCounts.get(transcriptId) ?? 0) + 1,
    );
  }

  applyLiveDelta(delta: LiveTranscriptDelta): void {
    this.refreshIfDirty();

    const replacedIds = new Set(delta.replaced_ids);
    const newWords: WordWithId[] = delta.new_words.map((word) => ({
      id: word.id,
      text: word.text,
      start_ms: word.start_ms,
      end_ms: word.end_ms,
      channel: word.channel,
    }));
    const newWordIds = new Set(newWords.map((word) => word.id));

    this.words = this.words
      .filter((word) => {
        const wordId = word.id ?? "";
        return !replacedIds.has(wordId) && !newWordIds.has(wordId);
      })
      .concat(newWords)
      .sort((a, b) => (a.start_ms ?? 0) - (b.start_ms ?? 0));

    this.hints = this.hints
      .filter((hint) => {
        const wordId = hint.word_id ?? "";
        return !replacedIds.has(wordId) && !newWordIds.has(wordId);
      })
      .concat(delta.new_words.flatMap(toStorageSpeakerHints))
      .sort((a, b) => (a.word_id ?? "").localeCompare(b.word_id ?? ""));

    this.flush();
  }

  appendWordsAndHints(
    words: WordWithId[],
    hints: SpeakerHintWithId[],
    options?: { mode?: "append" | "replace" },
  ): void {
    if (options?.mode === "replace") {
      this.words = [];
      this.hints = [];
    } else {
      this.refreshIfDirty();
    }

    this.words = this.words.concat(words);
    this.hints = this.hints.concat(hints);
    this.flush();
  }

  dispose(): void {
    if (this.disposed) {
      return;
    }

    this.disposed = true;

    const nextCount = (activeAccumulatorCounts.get(this.transcriptId) ?? 1) - 1;
    if (nextCount > 0) {
      activeAccumulatorCounts.set(this.transcriptId, nextCount);
      return;
    }

    activeAccumulatorCounts.delete(this.transcriptId);
    dirtyAccumulatorTranscriptIds.delete(this.transcriptId);
  }

  private refreshIfDirty(): void {
    if (!dirtyAccumulatorTranscriptIds.delete(this.transcriptId)) {
      return;
    }

    this.words = parseTranscriptWords(this.store, this.transcriptId);
    this.hints = parseTranscriptHints(this.store, this.transcriptId);
  }

  private flush(): void {
    updateTranscriptWords(this.store, this.transcriptId, this.words);
    writeTranscriptHints(this.store, this.transcriptId, this.hints);
  }
}

function markTranscriptAccumulatorDirty(transcriptId: string): void {
  if (activeAccumulatorCounts.has(transcriptId)) {
    dirtyAccumulatorTranscriptIds.add(transcriptId);
  }
}

function toStorageSpeakerHints(
  word: LiveTranscriptDelta["new_words"][number],
): SpeakerHintWithId[] {
  if (word.speaker_index == null) {
    return [];
  }

  return [
    {
      id: `${word.id}:provider_speaker_index`,
      word_id: word.id,
      type: "provider_speaker_index",
      value: JSON.stringify({
        channel: word.channel,
        speaker_index: word.speaker_index,
      }),
    },
  ];
}
