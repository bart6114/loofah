import { expect, test, vi } from "vitest";
import { createStore } from "zustand";

import type { BatchResponse } from "@hypr/plugin-transcription";

import { createBatchSlice, type BatchActions, type BatchState } from "./batch";

for (const providerWords of [true, false]) {
  test(`mixed batch preserves conversation speakers with provider words=${providerWords}`, () => {
    const store = createStore<BatchState & BatchActions>((set, get) =>
      createBatchSlice(set, get),
    );
    const persist = vi.fn();
    store.getState().setBatchPersist("synthetic", persist);
    const response: BatchResponse = {
      metadata: {
        duration: 2,
        session_audio: { source: "import", layout: "mixed" },
      },
      results: {
        channels: [
          {
            alternatives: [
              {
                transcript: "Blue lantern. Blue lantern.",
                confidence: 1,
                words: providerWords
                  ? [
                      {
                        word: "Blue",
                        punctuated_word: "Blue",
                        start: 0,
                        end: 0.2,
                        confidence: 1,
                        channel: 2,
                        speaker: 0,
                      },
                      {
                        word: "lantern.",
                        punctuated_word: "lantern.",
                        start: 0.2,
                        end: 0.5,
                        confidence: 1,
                        channel: 2,
                        speaker: 0,
                      },
                      {
                        word: "Blue",
                        punctuated_word: "Blue",
                        start: 1,
                        end: 1.2,
                        confidence: 1,
                        channel: 2,
                        speaker: 1,
                      },
                      {
                        word: "lantern.",
                        punctuated_word: "lantern.",
                        start: 1.2,
                        end: 1.5,
                        confidence: 1,
                        channel: 2,
                        speaker: 1,
                      },
                    ]
                  : [],
              },
            ],
          },
        ],
      },
    };
    store.getState().handleBatchResponse("synthetic", response);
    const [words, hints] = persist.mock.calls[0];
    expect(words).toHaveLength(4);
    expect(words.every((w: { channel: number }) => w.channel === 2)).toBe(true);
    expect(
      words
        .map((w: { text: string }) => w.text)
        .join("")
        .trim(),
    ).toBe("Blue lantern. Blue lantern.");
    expect(words[0].metadata.capture_source).toBe("import");
    if (providerWords)
      expect(
        hints.map(
          (h: { data: { speaker_index: number } }) => h.data.speaker_index,
        ),
      ).toEqual([0, 0, 1, 1]);
  });
}
