// Integration repro for the speaker-assignment view-refresh bug: exercises the
// render request and segment cache after the parent transcript record changes.
import { cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createRenderCacheHost } from "../render-cache.test-support";
import { useRenderedTranscriptData } from "./data-hooks";

const mocks = vi.hoisted(() => {
  return {
    renderTranscriptSegments: vi.fn(),
  };
});

vi.mock("@hypr/plugin-transcription", () => ({
  commands: {
    renderTranscriptSegments: mocks.renderTranscriptSegments,
  },
}));

function word(id: string, startMs: number) {
  return { id, text: id, start_ms: startMs, end_ms: startMs + 100, channel: 1 };
}

function providerHint(wordId: string, speakerIndex: number) {
  return {
    id: `${wordId}:provider_speaker_index`,
    word_id: wordId,
    type: "provider_speaker_index",
    value: JSON.stringify({ channel: 1, speaker_index: speakerIndex }),
  };
}

const baseTranscript = {
  id: "t1",
  ownerUserId: "self",
  sessionId: "s1",
  startedAt: 0,
  words: [word("w1", 0), word("w2", 200), word("w3", 400), word("w4", 600)],
  speakerHints: [
    providerHint("w1", 0),
    providerHint("w2", 0),
    providerHint("w3", 1),
    providerHint("w4", 1),
  ],
};

describe("speaker assignment view refresh", () => {
  let host: ReturnType<typeof createRenderCacheHost>;
  afterEach(() => {
    cleanup();
    host?.dispose();
  });
  it("re-renders segments after an assignment lands in the index", async () => {
    // Fake renderer: one segment per (channel, speaker_index), echoing any
    // channel_speaker assignment into the segment key like the Rust one does.
    mocks.renderTranscriptSegments.mockImplementation(
      (request: {
        transcripts: Array<{
          words: Array<{
            id: string;
            text: string;
            start_ms: number;
            end_ms: number;
            channel: number;
            speaker_index: number | null;
          }>;
          assignments: Array<{
            human_id: string;
            scope: { kind: string; speaker_index?: number };
          }>;
        }>;
      }) => {
        const transcript = request.transcripts[0]!;
        const humanByIndex = new Map<number, string>();
        for (const assignment of transcript.assignments) {
          if (
            assignment.scope.kind === "channel_speaker" &&
            typeof assignment.scope.speaker_index === "number"
          ) {
            humanByIndex.set(
              assignment.scope.speaker_index,
              assignment.human_id,
            );
          }
        }
        const byIndex = new Map<
          number,
          Array<(typeof transcript.words)[number]>
        >();
        for (const w of transcript.words) {
          const index = w.speaker_index ?? -1;
          byIndex.set(index, [...(byIndex.get(index) ?? []), w]);
        }
        const segments = [...byIndex.entries()].map(([index, words]) => ({
          id: `seg-${index}`,
          start_ms: words[0]!.start_ms,
          end_ms: words[words.length - 1]!.end_ms,
          text: words.map((w) => w.text).join(" "),
          key: {
            channel: "RemoteParty",
            speaker_index: index,
            speaker_human_id: humanByIndex.get(index) ?? null,
          },
          words,
        }));
        return Promise.resolve({ status: "ok", data: segments });
      },
    );

    host = createRenderCacheHost();
    const wrapper = host.Host;

    const people = [{ id: "alice", name: "Alice" }];
    const { result, rerender } = renderHook(
      ({ transcript }) => useRenderedTranscriptData("t1", transcript, people),
      { wrapper, initialProps: { transcript: baseTranscript } },
    );

    await waitFor(() => {
      expect(result.current.segments).toHaveLength(2);
    });
    expect(result.current.segments.map((s) => s.key.speaker_human_id)).toEqual([
      null,
      null,
    ]);

    // The parent query delivered a new record containing both assignments.
    rerender({
      transcript: {
        ...baseTranscript,
        speakerHints: [
          ...baseTranscript.speakerHints,
          {
            id: "w1:speaker_label",
            word_id: "w1",
            type: "speaker_label",
            value: "alice",
          },
          {
            id: "w3:speaker_label",
            word_id: "w3",
            type: "speaker_label",
            value: "alice",
          },
        ],
      },
    });

    await waitFor(() => {
      expect(
        result.current.segments.map((s) => s.key.speaker_human_id),
      ).toEqual(["alice", "alice"]);
    });
  });
});
