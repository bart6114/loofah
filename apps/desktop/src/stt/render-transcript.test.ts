import { beforeEach, describe, expect, it, vi } from "vitest";

const { renderTranscriptSegmentsCommand } = vi.hoisted(() => ({
  renderTranscriptSegmentsCommand: vi.fn(),
}));

vi.mock("@hypr/plugin-transcription", () => ({
  commands: {
    renderTranscriptSegments: renderTranscriptSegmentsCommand,
  },
}));

import {
  buildRenderTranscriptRequestFromRows,
  collectAssignedHumanIdsFromTranscriptRows,
  getRenderTranscriptRequestKey,
  renderRequestHasDiarizedChannel,
  renderTranscriptSegments,
  type TranscriptRow,
} from "./render-transcript";

const transcripts = {
  late: {
    started_at: 5_000,
    words: [
      {
        id: "late-word",
        text: " later",
        start_ms: 100,
        end_ms: 200,
        channel: 1,
      },
    ],
    speaker_hints: [
      {
        word_id: "late-word",
        type: "speaker_label",
        value: "remote",
      },
    ],
  },
  early: {
    started_at: 1_000,
    words: [
      {
        id: "early-word",
        text: " hello",
        start_ms: 0,
        end_ms: 100,
        channel: 0,
      },
    ],
    speaker_hints: [],
  },
  unordered: {
    started_at: 2_000,
    words: [
      {
        id: "unordered-word",
        text: " hello",
        start_ms: 0,
        end_ms: 100,
        channel: 1,
      },
    ],
    speaker_hints: [
      {
        word_id: "unordered-word",
        type: "speaker_label",
        value: "remote",
      },
      {
        word_id: "unordered-word",
        type: "provider_speaker_index",
        value: { channel: 1, speaker_index: 2 },
      },
    ],
  },
} as const;

function createRequest(
  transcriptIds: Array<keyof typeof transcripts> = ["late", "early"],
  participantIds = ["self", "remote"],
) {
  return buildRenderTranscriptRequestFromRows(
    transcriptIds.map(
      (transcriptId) => transcripts[transcriptId],
    ) as unknown as TranscriptRow[],
    {
      selfHumanId: "self",
      humans: [
        { human_id: "self", name: "Me" },
        { human_id: "remote", name: "Remote" },
        { human_id: "third", name: "Third" },
      ],
    },
    participantIds,
  );
}

describe("buildRenderTranscriptRequestFromRows", () => {
  beforeEach(() => {
    renderTranscriptSegmentsCommand.mockReset();
  });

  it("passes raw transcript rows and session participant ids to Rust", () => {
    const request = createRequest();

    expect(request).not.toBeNull();
    expect(
      request?.transcripts.map((transcript) => ({
        started_at: transcript.started_at,
        word_ids: transcript.words.map((word) => word.id),
      })),
    ).toEqual([
      {
        started_at: 5_000,
        word_ids: ["late-word"],
      },
      {
        started_at: 1_000,
        word_ids: ["early-word"],
      },
    ]);
    expect(request?.participant_human_ids).toEqual(["self", "remote"]);
    expect(request?.self_human_id).toBe("self");
  });

  it("flags transcripts whose words carry synthetic timing metadata", () => {
    const request = buildRenderTranscriptRequestFromRows([
      {
        started_at: 1_000,
        words: [
          {
            id: "synthetic-word",
            text: " hello",
            start_ms: 0,
            end_ms: 100,
            channel: 0,
            metadata: { timing: { source: "synthetic_speech" } },
          },
        ],
        speaker_hints: [],
      },
      {
        started_at: 2_000,
        words: [
          {
            id: "provider-word",
            text: " world",
            start_ms: 0,
            end_ms: 100,
            channel: 0,
          },
        ],
        speaker_hints: [],
      },
    ]);

    expect(
      request?.transcripts.map((transcript) => transcript.synthetic_timing),
    ).toEqual([true, undefined]);
  });

  it.each([undefined, "import", "unknown"])(
    "preserves channel-grouped timing fallback for %s capture",
    (captureSource) => {
      const request = buildRenderTranscriptRequestFromRows([
        {
          started_at: 1_000,
          words: [
            {
              id: "ch0-word-1",
              text: " hello",
              start_ms: 0,
              end_ms: 4_000,
              channel: 0,
              metadata: { capture_source: captureSource },
            },
            {
              id: "ch0-word-2",
              text: " there",
              start_ms: 20_000,
              end_ms: 24_000,
              channel: 0,
              metadata: { capture_source: captureSource },
            },
            {
              id: "ch1-word-1",
              text: " hi",
              start_ms: 100,
              end_ms: 4_100,
              channel: 1,
              metadata: { capture_source: captureSource },
            },
          ],
          speaker_hints: [],
        },
      ]);

      expect(request?.transcripts[0]?.synthetic_timing).toBe(true);
    },
  );

  it("does not flag chronological metadata-less words", () => {
    const request = buildRenderTranscriptRequestFromRows([
      {
        started_at: 1_000,
        words: [
          {
            id: "word-1",
            text: " hello",
            start_ms: 0,
            end_ms: 100,
            channel: 0,
          },
          {
            id: "word-2",
            text: " there",
            start_ms: 200,
            end_ms: 300,
            channel: 1,
          },
          {
            id: "word-3",
            text: " friend",
            start_ms: 400,
            end_ms: 500,
            channel: 0,
          },
        ],
        speaker_hints: [],
      },
    ]);

    expect(request?.transcripts[0]?.synthetic_timing).toBeUndefined();
  });

  it("tolerates small out-of-order jitter below the regression threshold", () => {
    const request = buildRenderTranscriptRequestFromRows([
      {
        started_at: 1_000,
        words: [
          {
            id: "word-1",
            text: " hello",
            start_ms: 10_000,
            end_ms: 10_100,
            channel: 0,
          },
          {
            id: "word-2",
            text: " there",
            start_ms: 6_000,
            end_ms: 6_100,
            channel: 1,
          },
          {
            id: "word-3",
            text: " friend",
            start_ms: 10_200,
            end_ms: 10_300,
            channel: 0,
          },
        ],
        speaker_hints: [],
      },
    ]);

    expect(request?.transcripts[0]?.synthetic_timing).toBeUndefined();
  });

  it("does not flag live-shape words where a channel re-appears after a regression", () => {
    // Live persist order: finalized partials can re-append at the end with early
    // start_ms, but channels alternate — never one contiguous run per channel.
    const request = buildRenderTranscriptRequestFromRows([
      {
        started_at: 1_000,
        words: [
          {
            id: "ch0-early",
            text: " we",
            start_ms: 20_000,
            end_ms: 20_100,
            channel: 0,
          },
          {
            id: "ch1-reply",
            text: " yes",
            start_ms: 26_000,
            end_ms: 26_100,
            channel: 1,
          },
          {
            id: "ch0-late-final",
            text: " hello",
            start_ms: 100,
            end_ms: 200,
            channel: 0,
          },
        ],
        speaker_hints: [],
      },
    ]);

    expect(request?.transcripts[0]?.synthetic_timing).toBeUndefined();
  });

  it("does not flag single-channel words with a large start_ms regression", () => {
    const request = buildRenderTranscriptRequestFromRows([
      {
        started_at: 1_000,
        words: [
          {
            id: "word-1",
            text: " hello",
            start_ms: 20_000,
            end_ms: 20_100,
            channel: 0,
          },
          {
            id: "word-2",
            text: " there",
            start_ms: 100,
            end_ms: 200,
            channel: 0,
          },
        ],
        speaker_hints: [],
      },
    ]);

    expect(request?.transcripts[0]?.synthetic_timing).toBeUndefined();
  });

  it("passes through all mapped participant ids for Rust-side resolution", () => {
    const request = createRequest(["early"], ["self", "remote", "third"]);

    expect(request?.participant_human_ids).toEqual(["self", "remote", "third"]);
  });

  it("applies provider speaker hints before user assignments regardless of storage order", () => {
    const request = createRequest(["unordered"]);

    expect(request?.transcripts[0]?.words[0]?.speaker_index).toBe(2);
    expect(request?.transcripts[0]?.assignments).toEqual([
      {
        human_id: "remote",
        scope: {
          kind: "channel_speaker",
          channel: "RemoteParty",
          speaker_index: 2,
        },
      },
    ]);
  });

  it("keeps a channel-scope assignment as fallback alongside channel_speaker assignments", () => {
    const request = buildRenderTranscriptRequestFromRows([
      {
        started_at: 1_000,
        words: [
          {
            id: "gap-word",
            text: " gap",
            start_ms: 0,
            end_ms: 100,
            channel: 1,
          },
          {
            id: "indexed-word",
            text: " indexed",
            start_ms: 100,
            end_ms: 200,
            channel: 1,
          },
        ],
        speaker_hints: [
          {
            word_id: "gap-word",
            type: "speaker_label",
            value: "remote",
          },
          {
            word_id: "indexed-word",
            type: "provider_speaker_index",
            value: { channel: 1, speaker_index: 1 },
          },
          {
            word_id: "indexed-word",
            type: "speaker_label",
            value: "third",
          },
        ],
      },
    ]);

    expect(request?.transcripts[0]?.assignments).toEqual([
      {
        human_id: "remote",
        scope: { kind: "channel", channel: "RemoteParty" },
      },
      {
        human_id: "third",
        scope: {
          kind: "channel_speaker",
          channel: "RemoteParty",
          speaker_index: 1,
        },
      },
    ]);
  });

  it("collects assigned speaker labels from transcript rows", () => {
    expect(
      collectAssignedHumanIdsFromTranscriptRows([
        {
          speaker_hints: [
            {
              word_id: "word-1",
              type: "speaker_label",
              value: "remote",
            },
            {
              word_id: "word-2",
              type: "speaker_label",
              value: "third",
            },
            {
              word_id: "word-3",
              type: "provider_speaker_index",
              value: JSON.stringify({ speaker_index: 1 }),
            },
          ],
        },
      ]),
    ).toEqual(["remote", "third"]);
  });

  it("rounds fractional millisecond timings before invoking Rust", async () => {
    renderTranscriptSegmentsCommand.mockResolvedValue({
      status: "ok",
      data: [],
    });

    await renderTranscriptSegments({
      transcripts: [
        {
          started_at: 1_000.6,
          words: [
            {
              id: "word-1",
              text: " hello",
              start_ms: 10.4,
              end_ms: 19.6,
              channel: 0,
              speaker_index: null,
            },
          ],
          assignments: [],
        },
      ],
      participant_human_ids: [],
      self_human_id: null,
      humans: [],
    });

    expect(renderTranscriptSegmentsCommand).toHaveBeenCalledWith({
      transcripts: [
        {
          started_at: 1_001,
          words: [
            {
              id: "word-1",
              text: " hello",
              start_ms: 10,
              end_ms: 20,
              channel: 0,
              speaker_index: null,
            },
          ],
          assignments: [],
        },
      ],
      participant_human_ids: [],
      self_human_id: null,
      humans: [],
    });
  });

  it("reattaches word metadata after Rust renders transcript segments", async () => {
    renderTranscriptSegmentsCommand.mockResolvedValue({
      status: "ok",
      data: [
        {
          id: "segment-1",
          key: {
            channel: "DirectMic",
            speaker_index: null,
            speaker_human_id: null,
          },
          speaker_label: "You",
          start_ms: 10,
          end_ms: 20,
          text: "hello",
          words: [
            {
              id: "word-1",
              text: "hello",
              start_ms: 10,
              end_ms: 20,
              channel: "DirectMic",
              is_final: true,
            },
          ],
        },
      ],
    });

    const segments = await renderTranscriptSegments({
      transcripts: [
        {
          started_at: 1_000,
          words: [
            {
              id: "word-1",
              text: " hello",
              start_ms: 10,
              end_ms: 20,
              channel: 0,
              speaker_index: null,
              metadata: {
                timing: {
                  source: "synthetic_text",
                },
              },
            } as never,
          ],
          assignments: [],
        },
      ],
      participant_human_ids: [],
      self_human_id: null,
      humans: [],
    });

    expect(segments[0]?.words[0]?.metadata).toEqual({
      timing: {
        source: "synthetic_text",
      },
    });
  });
});

describe("renderRequestHasDiarizedChannel", () => {
  const wordRow = (id: string, channel: number) => ({
    id,
    text: ` ${id}`,
    start_ms: 0,
    end_ms: 100,
    channel,
  });
  const providerHint = (
    wordId: string,
    channel: number,
    speakerIndex: number,
  ) => ({
    word_id: wordId,
    type: "provider_speaker_index",
    value: { channel, speaker_index: speakerIndex },
  });

  it("detects a channel with two distinct speaker indexes", () => {
    const request = buildRenderTranscriptRequestFromRows([
      {
        started_at: 1_000,
        words: [wordRow("w1", 1), wordRow("w2", 1)],
        speaker_hints: [providerHint("w1", 1, 0), providerHint("w2", 1, 1)],
      },
    ]);

    expect(renderRequestHasDiarizedChannel(request)).toBe(true);
  });

  it("treats a single-index channel as undiarized", () => {
    const request = buildRenderTranscriptRequestFromRows([
      {
        started_at: 1_000,
        words: [wordRow("w1", 1), wordRow("w2", 1)],
        speaker_hints: [providerHint("w1", 1, 0), providerHint("w2", 1, 0)],
      },
    ]);

    expect(renderRequestHasDiarizedChannel(request)).toBe(false);
  });

  it("does not combine indexes across channels", () => {
    const request = buildRenderTranscriptRequestFromRows([
      {
        started_at: 1_000,
        words: [wordRow("w1", 0), wordRow("w2", 1)],
        speaker_hints: [providerHint("w1", 0, 0), providerHint("w2", 1, 1)],
      },
    ]);

    expect(renderRequestHasDiarizedChannel(request)).toBe(false);
  });

  it("treats channel-only words as undiarized", () => {
    const request = buildRenderTranscriptRequestFromRows([
      {
        started_at: 1_000,
        words: [wordRow("w1", 1), wordRow("w2", 1)],
        speaker_hints: [],
      },
    ]);

    expect(renderRequestHasDiarizedChannel(request)).toBe(false);
  });
});

describe("getRenderTranscriptRequestKey", () => {
  it("keeps large transcript payloads out of query keys", () => {
    const request = createRequest();

    expect(getRenderTranscriptRequestKey(request)).toMatch(/^\d+:\d+:\d+:/);
  });

  it("changes when rendered transcript inputs change", () => {
    const request = createRequest();
    const changedRequest = {
      ...request!,
      transcripts: request!.transcripts.map((transcript, index) =>
        index === 0
          ? {
              ...transcript,
              words: transcript.words.map((word, wordIndex) =>
                wordIndex === 0 ? { ...word, text: " changed" } : word,
              ),
            }
          : transcript,
      ),
    };

    expect(getRenderTranscriptRequestKey(changedRequest)).not.toBe(
      getRenderTranscriptRequestKey(request),
    );
  });

  it("changes when speaker assignments change", () => {
    const request = createRequest();
    const changedRequest = {
      ...request!,
      transcripts: request!.transcripts.map((transcript, index) =>
        index === 0
          ? {
              ...transcript,
              assignments: [
                {
                  human_id: "third",
                  scope: {
                    kind: "channel",
                    channel: "RemoteParty",
                  },
                } as const,
              ],
            }
          : transcript,
      ),
    };

    expect(getRenderTranscriptRequestKey(changedRequest)).not.toBe(
      getRenderTranscriptRequestKey(request),
    );
  });
});

it.each(["import", "unknown"])(
  "does not infer self from %s channel zero and preserves separate clusters",
  (captureSource) => {
    const request = buildRenderTranscriptRequestFromRows(
      [
        {
          words: [0, 1].map((channel) => ({
            id: `w${channel}`,
            text: "I will follow up.",
            start_ms: channel * 100,
            end_ms: channel * 100 + 50,
            channel,
            metadata: { capture_source: captureSource },
          })),
          speaker_hints: [
            { word_id: "w1", type: "speaker_label", value: "self" },
          ],
        },
      ],
      { selfHumanId: "self", humans: [] },
    );
    expect(request!.transcripts[0].words.map((word) => word.channel)).toEqual([
      2, 2,
    ]);
    expect(
      request!.transcripts[0].words.map((word) => word.speaker_index),
    ).toEqual([0, 1]);
    expect(request!.transcripts[0].assignments).toEqual([
      {
        human_id: "self",
        scope: {
          kind: "channel_speaker",
          channel: "MixedCapture",
          speaker_index: 1,
        },
      },
    ]);
  },
);
