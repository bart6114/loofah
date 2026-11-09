import { cleanup, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { SegmentHeader } from "./segment-header";

import type { Segment } from "~/stt/live-segment";

vi.mock("./speaker-assign", () => ({
  SpeakerRenameControl: ({
    label,
    disabled,
  }: {
    label: string;
    disabled: boolean;
  }) => (
    <button type="button" disabled={disabled}>
      {label}
    </button>
  ),
}));

beforeEach(() => {
  cleanup();
});

describe("SegmentHeader", () => {
  it("keeps the speaker label visible without exposing timestamps", () => {
    render(
      <SegmentHeader
        sessionId="session-1"
        transcriptId="transcript-1"
        segment={createRemoteSegment(2)}
        label="Speaker 3"
        people={[]}
        channelAssignmentState={emptyAssignmentState()}
        recording={false}
      />,
    );

    expect(screen.getByRole("button", { name: "Speaker 3" })).toBeTruthy();
    expect(screen.queryByText("00:12 - 00:18")).toBeNull();
  });

  it("renders the label resolved by the transcript renderer", () => {
    render(
      <SegmentHeader
        sessionId="session-1"
        transcriptId="transcript-1"
        segment={createRemoteSegment(0)}
        label="Artem"
        people={[]}
        channelAssignmentState={emptyAssignmentState()}
        recording={false}
      />,
    );

    expect(screen.getByRole("button", { name: "Artem" })).toBeTruthy();
  });

  it("updates labels and disables renaming while recording", () => {
    const segment = createRemoteSegment(0);
    const { rerender } = render(
      <SegmentHeader
        sessionId="session-1"
        transcriptId="transcript-1"
        segment={segment}
        label="Artem"
        people={[]}
        channelAssignmentState={emptyAssignmentState()}
        recording={false}
      />,
    );

    expect(screen.getByRole("button", { name: "Artem" })).toBeTruthy();

    rerender(
      <SegmentHeader
        sessionId="session-1"
        transcriptId="transcript-1"
        segment={segment}
        label="Speaker 1"
        people={[]}
        channelAssignmentState={emptyAssignmentState()}
        recording
      />,
    );

    const button = screen.getByRole("button", { name: "Speaker 1" });
    expect((button as HTMLButtonElement).disabled).toBe(true);
  });
});

function emptyAssignmentState() {
  return {
    anchorWordIdBySpeakerIndex: new Map<number, string>(),
    channelHasAssignment: false,
  };
}

function createRemoteSegment(speakerIndex: number): Segment {
  return {
    id: "segment-1",
    key: {
      channel: "RemoteParty",
      speaker_index: speakerIndex,
      speaker_human_id: null,
    },
    start_ms: 12_000,
    end_ms: 18_000,
    text: "hello world",
    words: [
      {
        id: "word-1",
        text: "hello",
        start_ms: 12_000,
        end_ms: 13_000,
        channel: "RemoteParty",
        is_final: true,
      },
    ],
  } as Segment;
}
