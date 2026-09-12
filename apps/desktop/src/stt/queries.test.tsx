import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { SegmentKeyUtils } from "./live-segment";

import { DEFAULT_USER_ID } from "~/shared/utils";
import type { TranscriptWithData } from "~/types/tauri.gen";

const mocks = vi.hoisted(() => ({
  sessionWriteTranscript: vi.fn(
    (
      _sessionId: string,
      _transcript: TranscriptWithData,
    ): Promise<
      { status: "ok"; data: null } | { status: "error"; error: string }
    > => Promise.resolve({ status: "ok", data: null }),
  ),
  sessionTranscripts: vi.fn(
    (
      _sessionId: string,
    ): Promise<
      | { status: "ok"; data: Array<Record<string, unknown>> }
      | { status: "error"; error: string }
    > => Promise.resolve({ status: "ok", data: [] }),
  ),
  transcriptGet: vi.fn(
    (
      _transcriptId: string,
    ): Promise<
      | { status: "ok"; data: Record<string, unknown> | null }
      | { status: "error"; error: string }
    > => Promise.resolve({ status: "ok", data: null }),
  ),
  sessionReplaceTranscripts: vi.fn(
    (
      _sessionId: string,
      _transcript: TranscriptWithData,
    ): Promise<
      { status: "ok"; data: null } | { status: "error"; error: string }
    > => Promise.resolve({ status: "ok", data: null }),
  ),
  sessionAssignTranscriptSpeaker: vi.fn(
    (
      _transcriptId: string,
      _channel: number,
      _speakerIndex: number | null,
      _speakerLabel: string,
      _anchorWordId: string,
    ): Promise<
      { status: "ok"; data: null } | { status: "error"; error: string }
    > => Promise.resolve({ status: "ok", data: null }),
  ),
  peopleList: vi.fn(
    (): Promise<
      | { status: "ok"; data: Array<{ id: string; name: string }> }
      | { status: "error"; error: string }
    > => Promise.resolve({ status: "ok", data: [] }),
  ),
}));

vi.mock("~/types/tauri.gen", () => ({
  commands: {
    sessionWriteTranscript: mocks.sessionWriteTranscript,
    sessionTranscripts: mocks.sessionTranscripts,
    transcriptGet: mocks.transcriptGet,
    sessionReplaceTranscripts: mocks.sessionReplaceTranscripts,
    sessionAssignTranscriptSpeaker: mocks.sessionAssignTranscriptSpeaker,
    peopleList: mocks.peopleList,
  },
  events: {
    indexChanged: {
      listen: vi.fn().mockResolvedValue(() => {}),
    },
  },
}));

import {
  appendTranscriptWordsAndHints,
  assignTranscriptSpeaker,
  createTranscript,
  softDeleteTranscript,
  useSessionTranscripts,
  useTranscript,
  useTranscriptLabelContext,
} from "./queries";

function createWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });

  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    );
  };
}

describe("transcript queries", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.sessionWriteTranscript.mockResolvedValue({
      status: "ok",
      data: null,
    });
    mocks.sessionTranscripts.mockResolvedValue({ status: "ok", data: [] });
    mocks.transcriptGet.mockResolvedValue({ status: "ok", data: null });
    mocks.sessionReplaceTranscripts.mockResolvedValue({
      status: "ok",
      data: null,
    });
    mocks.sessionAssignTranscriptSpeaker.mockResolvedValue({
      status: "ok",
      data: null,
    });
  });

  it("maps store transcripts into renderer records", async () => {
    mocks.sessionTranscripts.mockResolvedValue({
      status: "ok",
      data: [
        {
          id: "transcript-1",
          user_id: "user-1",
          session_id: "session-1",
          started_at: 1000,
          ended_at: 2000,
          words: [
            {
              id: "word-1",
              text: "Hello",
              start_ms: 0,
              end_ms: 500,
              channel: 0,
            },
          ],
          speaker_hints: [
            { word_id: "word-1", type: "provider_speaker_index", value: 0 },
          ],
        },
      ],
    });

    const { result } = renderHook(() => useSessionTranscripts("session-1"), {
      wrapper: createWrapper(),
    });

    await waitFor(() => expect(result.current).toHaveLength(1));
    expect(result.current).toEqual([
      expect.objectContaining({
        id: "transcript-1",
        ownerUserId: "user-1",
        sessionId: "session-1",
        startedAt: 1000,
        endedAt: 2000,
        words: [expect.objectContaining({ id: "word-1" })],
        speakerHints: [expect.objectContaining({ word_id: "word-1" })],
      }),
    ]);
    expect(mocks.sessionTranscripts).toHaveBeenCalledWith("session-1");
  });

  it("defaults absent word/hint payloads to empty without hiding the row", async () => {
    mocks.transcriptGet.mockResolvedValue({
      status: "ok",
      data: {
        id: "transcript-1",
        user_id: "user-1",
        session_id: "session-1",
        started_at: 1000,
        ended_at: null,
      },
    });

    const { result } = renderHook(() => useTranscript("transcript-1"), {
      wrapper: createWrapper(),
    });

    await waitFor(() => expect(result.current).not.toBeNull());
    expect(result.current).toEqual(
      expect.objectContaining({
        id: "transcript-1",
        endedAt: undefined,
        words: [],
        speakerHints: [],
      }),
    );
    expect(mocks.transcriptGet).toHaveBeenCalledWith("transcript-1");
  });

  it.each(["", null, undefined])(
    "labels live and saved microphone segments as You with owner %s",
    async (userId) => {
      mocks.transcriptGet.mockResolvedValue({
        status: "ok",
        data: {
          id: "transcript-1",
          user_id: userId,
          session_id: "session-1",
          started_at: 1000,
          words: [],
          speaker_hints: [],
        },
      });

      const { result } = renderHook(
        () => useTranscriptLabelContext("transcript-1"),
        { wrapper: createWrapper() },
      );

      await waitFor(() => expect(result.current).toBeDefined());
      for (const speakerHumanId of [DEFAULT_USER_ID, null]) {
        expect(
          SegmentKeyUtils.renderLabel(
            {
              channel: "DirectMic",
              speaker_index: null,
              speaker_human_id: speakerHumanId,
            },
            result.current,
          ),
        ).toBe("You");
      }
    },
  );

  it("resolves speaker labels straight from assigned hint values, not a lookup", async () => {
    mocks.transcriptGet.mockResolvedValue({
      status: "ok",
      data: {
        id: "transcript-1",
        user_id: "self",
        session_id: "session-1",
        started_at: 1000,
        ended_at: null,
        words: [],
        speaker_hints: [
          {
            id: "hint-1",
            word_id: "word-1",
            type: "speaker_label",
            value: "Alice",
          },
        ],
      },
    });

    const { result } = renderHook(
      () => useTranscriptLabelContext("transcript-1"),
      { wrapper: createWrapper() },
    );

    await waitFor(() => expect(result.current).toBeDefined());
    expect(result.current?.getSelfHumanId()).toBe("self");
    expect(result.current?.getHumanName("Alice")).toBe("Alice");
    expect(result.current?.getHumanName("self")).toBeUndefined();
    expect(result.current?.getParticipantHumanIds?.()).toEqual([]);
  });

  it("resolves people-registry names ahead of raw hint values", async () => {
    mocks.peopleList.mockResolvedValue({
      status: "ok",
      data: [{ id: "bob_peters", name: "Bob Peters" }],
    });
    mocks.transcriptGet.mockResolvedValue({
      status: "ok",
      data: {
        id: "transcript-1",
        user_id: "self",
        session_id: "session-1",
        started_at: 1000,
        ended_at: null,
        words: [],
        speaker_hints: [
          {
            id: "hint-1",
            word_id: "word-1",
            type: "speaker_label",
            value: "bob_peters",
          },
          {
            id: "hint-2",
            word_id: "word-2",
            type: "speaker_label",
            value: "kim",
          },
        ],
      },
    });

    const { result } = renderHook(
      () => useTranscriptLabelContext("transcript-1"),
      { wrapper: createWrapper() },
    );

    await waitFor(() =>
      expect(result.current?.getHumanName("bob_peters")).toBe("Bob Peters"),
    );
    // Legacy hint value with no registry entry still renders as itself.
    expect(result.current?.getHumanName("kim")).toBe("kim");
    expect(result.current?.getHumanName("unknown")).toBeUndefined();
  });

  it("writes a new transcript through the store", async () => {
    await createTranscript({
      id: "transcript-1",
      sessionId: "session-1",
      ownerUserId: "user-1",
      createdAt: "2026-07-10T12:00:00.000Z",
      startedAt: 1000,
      words: [
        { id: "word-1", text: "Hello", start_ms: 0, end_ms: 500, channel: 0 },
      ],
      speakerHints: [],
    });

    expect(mocks.sessionWriteTranscript).toHaveBeenCalledWith(
      "session-1",
      expect.objectContaining({
        id: "transcript-1",
        session_id: "session-1",
        started_at: 1000,
        words: [expect.objectContaining({ id: "word-1", text: "Hello" })],
      }),
    );
  });

  it("replaces the session's whole transcript set through the supersede primitive", async () => {
    await createTranscript({
      id: "transcript-new",
      sessionId: "session-1",
      ownerUserId: "user-1",
      createdAt: "2026-07-10T12:00:00.000Z",
      startedAt: 1000,
      replaceSession: true,
    });

    expect(mocks.sessionReplaceTranscripts).toHaveBeenCalledWith(
      "session-1",
      expect.objectContaining({
        id: "transcript-new",
        session_id: "session-1",
        started_at: 1000,
      }),
    );
    expect(mocks.sessionWriteTranscript).not.toHaveBeenCalled();
  });

  it("surfaces a supersede failure instead of swallowing it", async () => {
    mocks.sessionReplaceTranscripts.mockResolvedValue({
      status: "error",
      error: "disk gone",
    });

    await expect(
      createTranscript({
        id: "transcript-new",
        sessionId: "session-1",
        ownerUserId: "user-1",
        createdAt: "2026-07-10T12:00:00.000Z",
        startedAt: 1000,
        replaceSession: true,
      }),
    ).rejects.toThrow("disk gone");
  });

  it("appends words onto the current transcript and writes the merged result", async () => {
    mocks.transcriptGet.mockResolvedValueOnce({
      status: "ok",
      data: {
        id: "transcript-1",
        session_id: "session-1",
        started_at: 1000,
        words: [
          { id: "word-1", text: "Hello", start_ms: 0, end_ms: 500, channel: 0 },
        ],
        speaker_hints: [],
      },
    });

    await appendTranscriptWordsAndHints(
      "transcript-1",
      [{ id: "word-2", text: "world", start_ms: 500, end_ms: 900, channel: 0 }],
      [],
    );

    const call = mocks.sessionWriteTranscript.mock.calls[0];
    expect(call?.[0]).toBe("session-1");
    expect(call?.[1]?.id).toBe("transcript-1");
    expect(call?.[1]?.started_at).toBe(1000);
    expect(call?.[1]?.words).toEqual([
      expect.objectContaining({ id: "word-1", text: "Hello" }),
      expect.objectContaining({ id: "word-2", text: "world" }),
    ]);
  });

  // REGRESSION: the store returns metadata as a parsed object; JSON.parse-ing it
  // used to throw and silently null out every word's metadata on append, losing the
  // renderer's synthetic-timing flag.
  it("passes already-parsed word metadata through the append path untouched", async () => {
    mocks.transcriptGet.mockResolvedValueOnce({
      status: "ok",
      data: {
        id: "transcript-1",
        session_id: "session-1",
        started_at: 1000,
        words: [
          {
            id: "word-1",
            text: "Hello",
            start_ms: 0,
            end_ms: 500,
            channel: 0,
            metadata: { timing: { source: "synthetic_speech" } },
          },
        ],
        speaker_hints: [],
      },
    });

    await appendTranscriptWordsAndHints(
      "transcript-1",
      [{ id: "word-2", text: "world", start_ms: 500, end_ms: 900, channel: 0 }],
      [],
    );

    const call = mocks.sessionWriteTranscript.mock.calls[0];
    expect(call?.[1]?.words?.[0]?.metadata).toEqual({
      timing: { source: "synthetic_speech" },
    });
  });

  it("refuses to mutate a transcript that no longer exists", async () => {
    mocks.transcriptGet.mockResolvedValueOnce({ status: "ok", data: null });

    await expect(
      appendTranscriptWordsAndHints("transcript-1", [], []),
    ).rejects.toThrow("does not exist");
    expect(mocks.sessionWriteTranscript).not.toHaveBeenCalled();
  });

  it("assigns a speaker through the hints-only store command", async () => {
    await assignTranscriptSpeaker({
      transcriptId: "transcript-1",
      segmentKey: {
        channel: "RemoteParty",
        speaker_index: 0,
        speaker_human_id: null,
      },
      speakerLabel: "Alice",
      anchorWordId: "word-1",
    });

    // Hints-only path: no full-transcript read-modify-write round trips.
    expect(mocks.sessionAssignTranscriptSpeaker).toHaveBeenCalledWith(
      "transcript-1",
      1,
      0,
      "Alice",
      "word-1",
    );
    expect(mocks.transcriptGet).not.toHaveBeenCalled();
    expect(mocks.sessionWriteTranscript).not.toHaveBeenCalled();
  });

  it("maps a missing speaker index to null for the store command", async () => {
    await assignTranscriptSpeaker({
      transcriptId: "transcript-1",
      segmentKey: {
        channel: "DirectMic",
        speaker_index: null,
        speaker_human_id: null,
      },
      speakerLabel: "Me",
      anchorWordId: "word-9",
    });

    expect(mocks.sessionAssignTranscriptSpeaker).toHaveBeenCalledWith(
      "transcript-1",
      0,
      null,
      "Me",
      "word-9",
    );
  });

  it("surfaces a speaker-assignment failure instead of swallowing it", async () => {
    mocks.sessionAssignTranscriptSpeaker.mockResolvedValue({
      status: "error",
      error: "transcript gone",
    });

    await expect(
      assignTranscriptSpeaker({
        transcriptId: "transcript-1",
        segmentKey: {
          channel: "RemoteParty",
          speaker_index: 0,
          speaker_human_id: null,
        },
        speakerLabel: "Alice",
        anchorWordId: "word-1",
      }),
    ).rejects.toThrow("transcript gone");
  });

  it("zeroes a transcript's content instead of deleting the row", async () => {
    await softDeleteTranscript("session-1", "transcript-1");

    expect(mocks.sessionWriteTranscript).toHaveBeenCalledWith("session-1", {
      id: "transcript-1",
      session_id: "session-1",
      words: [],
      speaker_hints: [],
    });
  });
});
