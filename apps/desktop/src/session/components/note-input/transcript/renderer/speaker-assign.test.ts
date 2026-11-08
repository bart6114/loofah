import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { createElement } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  buildChannelAssignmentStates,
  getAssignmentAnchorWordId,
  SpeakerRenameControl,
} from "./speaker-assign";

import type { Segment } from "~/stt/live-segment";
import type { TranscriptRecord } from "~/stt/queries";

const { assignTranscriptSpeakerMock, ensurePersonMock } = vi.hoisted(() => ({
  assignTranscriptSpeakerMock: vi.fn(),
  ensurePersonMock: vi.fn(),
}));

vi.mock("~/stt/queries", () => ({
  assignTranscriptSpeaker: assignTranscriptSpeakerMock,
}));

vi.mock("~/people/queries", () => ({
  ensurePerson: ensurePersonMock,
}));

const source = vi.hoisted(() => {
  const snapshot = { state: "connected", error: null };
  return {
    scope: {
      client: {
        canEdit: true,
        features: ["legacy_desktop_transcript_controls"],
      },
      getSnapshot: () => snapshot,
      subscribe: () => () => {},
    },
  };
});
vi.mock("~/shared/application/vault-provider", () => ({
  useVaultHost: () => ({ scope: source.scope }),
}));

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  const snapshot = { state: "connected", error: null };
  source.scope = {
    client: { canEdit: true, features: ["legacy_desktop_transcript_controls"] },
    getSnapshot: () => snapshot,
    subscribe: () => () => {},
  };
  assignTranscriptSpeakerMock.mockResolvedValue(undefined);
  ensurePersonMock.mockImplementation(async (name: string) => ({
    id: name
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "_"),
    name: name.trim(),
  }));
});

function remoteSegment(): Segment {
  return {
    id: "segment-1",
    key: {
      channel: "RemoteParty",
      speaker_index: 2,
      speaker_human_id: null,
    },
    start_ms: 0,
    end_ms: 100,
    text: "hello",
    words: [
      {
        id: "word-1",
        text: "hello",
        start_ms: 0,
        end_ms: 100,
        channel: "RemoteParty",
        is_final: true,
      },
    ],
  } as Segment;
}

describe("buildChannelAssignmentStates", () => {
  it("builds all channel state in one pass while preserving hint overrides", () => {
    const transcript: TranscriptRecord = {
      id: "transcript-1",
      ownerUserId: "self",
      sessionId: "session-1",
      startedAt: 0,
      words: [
        { id: "word-1", text: "one", start_ms: 0, end_ms: 1, channel: 1 },
        { id: "word-2", text: "two", start_ms: 1, end_ms: 2, channel: 1 },
        { id: "word-3", text: "three", start_ms: 2, end_ms: 3, channel: 0 },
      ],
      speakerHints: [
        {
          word_id: "word-1",
          type: "provider_speaker_index",
          value: JSON.stringify({ channel: 2, speaker_index: 4 }),
        },
        {
          word_id: "word-2",
          type: "provider_speaker_index",
          value: { channel: 1, speaker_index: 3 },
        },
        {
          word_id: "word-3",
          type: "provider_speaker_index",
          value: "malformed",
        },
        { word_id: "word-2", type: "speaker_label", value: "alice" },
        { word_id: "missing", type: "speaker_label", value: "ignored" },
      ],
    };

    const states = buildChannelAssignmentStates(transcript);

    expect([...states.get("MixedCapture")!.anchorWordIdBySpeakerIndex]).toEqual(
      [[4, "word-1"]],
    );
    expect([...states.get("RemoteParty")!.anchorWordIdBySpeakerIndex]).toEqual([
      [3, "word-2"],
    ]);
    expect(states.get("RemoteParty")!.channelHasAssignment).toBe(true);
    expect(states.get("DirectMic")!.anchorWordIdBySpeakerIndex.size).toBe(0);
    expect(states.get("DirectMic")!.channelHasAssignment).toBe(false);
  });

  it("keeps the first anchor for each speaker index on a channel", () => {
    const transcript: TranscriptRecord = {
      id: "transcript-1",
      ownerUserId: "self",
      sessionId: "session-1",
      startedAt: 0,
      words: [
        { id: "word-1", text: "one", start_ms: 0, end_ms: 1, channel: 1 },
        { id: "word-2", text: "two", start_ms: 1, end_ms: 2, channel: 1 },
      ],
      speakerHints: [
        {
          word_id: "word-1",
          type: "provider_speaker_index",
          value: { speaker_index: 2 },
        },
        {
          word_id: "word-2",
          type: "provider_speaker_index",
          value: { speaker_index: 2 },
        },
      ],
    };

    expect(
      buildChannelAssignmentStates(transcript)
        .get("RemoteParty")
        ?.anchorWordIdBySpeakerIndex.get(2),
    ).toBe("word-1");
  });
});

describe("SpeakerRenameControl", () => {
  it("renders the current label as a clickable pill", () => {
    render(
      createElement(SpeakerRenameControl, {
        sessionId: "session-1",
        segment: remoteSegment(),
        transcriptId: "transcript-1",
        color: "red",
        label: "Speaker 2",
      }),
    );

    const trigger = screen.getByRole("button", { name: "Speaker 2" });
    expect(trigger.className).toContain("rounded-full");
    expect(trigger.className).toContain("hover:underline");
  });

  it("is inert while the session is recording", () => {
    render(
      createElement(SpeakerRenameControl, {
        sessionId: "session-1",
        segment: remoteSegment(),
        transcriptId: "transcript-1",
        color: "red",
        label: "Speaker 2",
        disabled: true,
      }),
    );

    const trigger = screen.getByRole("button", {
      name: "Speaker 2",
    }) as HTMLButtonElement;
    expect(trigger.disabled).toBe(true);
    fireEvent.click(trigger);
    expect(screen.queryByRole("textbox")).toBeNull();
  });

  it("ensures a person and assigns their id on Enter", async () => {
    const onAssigned = vi.fn();
    render(
      createElement(SpeakerRenameControl, {
        sessionId: "session-1",
        segment: remoteSegment(),
        transcriptId: "transcript-1",
        color: "red",
        label: "Speaker 2",
        onAssigned,
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: "Speaker 2" }));
    const input = screen.getByRole("textbox") as HTMLInputElement;
    expect(input.value).toBe("Speaker 2");

    fireEvent.change(input, { target: { value: "Alice Smith" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => {
      expect(ensurePersonMock).toHaveBeenCalledWith("Alice Smith");
      expect(assignTranscriptSpeakerMock).toHaveBeenCalledWith(
        {
          transcriptId: "transcript-1",
          segmentKey: {
            channel: "RemoteParty",
            speaker_index: 2,
            speaker_human_id: null,
          },
          speakerLabel: "alice_smith",
          anchorWordId: "word-1",
        },
        expect.any(Function),
      );
    });
    await waitFor(() => expect(onAssigned).toHaveBeenCalledWith("alice_smith"));
  });

  it("saves the rename on blur without pressing Enter", async () => {
    render(
      createElement(SpeakerRenameControl, {
        sessionId: "session-1",
        segment: remoteSegment(),
        transcriptId: "transcript-1",
        color: "red",
        label: "Speaker 2",
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: "Speaker 2" }));
    const input = screen.getByRole("textbox") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "Bob" } });
    fireEvent.blur(input);

    await waitFor(() => {
      expect(assignTranscriptSpeakerMock).toHaveBeenCalledWith(
        expect.objectContaining({ speakerLabel: "bob" }),
        expect.any(Function),
      );
    });
  });

  it("lists matching people and assigns the clicked person without ensure", async () => {
    const people = [
      { id: "bob_peters", name: "Bob Peters" },
      { id: "kim", name: "Kim" },
    ];
    const onAssigned = vi.fn();
    render(
      createElement(SpeakerRenameControl, {
        sessionId: "session-1",
        segment: remoteSegment(),
        transcriptId: "transcript-1",
        color: "red",
        label: "Speaker 2",
        people,
        onAssigned,
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: "Speaker 2" }));
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "bo" } });

    const option = screen.getByRole("option", { name: "Bob Peters" });
    expect(screen.queryByRole("option", { name: "Kim" })).toBeNull();
    fireEvent.click(option);

    await waitFor(() => {
      expect(assignTranscriptSpeakerMock).toHaveBeenCalledWith(
        expect.objectContaining({ speakerLabel: "bob_peters" }),
        expect.any(Function),
      );
    });
    expect(ensurePersonMock).not.toHaveBeenCalled();
    await waitFor(() => expect(onAssigned).toHaveBeenCalledWith("bob_peters"));
  });

  it("commits exactly once when a suggestion click races the blur save", async () => {
    const people = [{ id: "bob_peters", name: "Bob Peters" }];
    render(
      createElement(SpeakerRenameControl, {
        sessionId: "session-1",
        segment: remoteSegment(),
        transcriptId: "transcript-1",
        color: "red",
        label: "Speaker 2",
        people,
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: "Speaker 2" }));
    const input = screen.getByRole("textbox") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "Bob" } });

    const option = screen.getByRole("option", { name: "Bob Peters" });
    fireEvent.click(option);
    fireEvent.blur(input);

    await waitFor(() => {
      expect(assignTranscriptSpeakerMock).toHaveBeenCalledTimes(1);
    });
    expect(assignTranscriptSpeakerMock).toHaveBeenCalledWith(
      expect.objectContaining({ speakerLabel: "bob_peters" }),
      expect.any(Function),
    );
    expect(ensurePersonMock).not.toHaveBeenCalled();
  });

  it("selects the highlighted suggestion with arrow keys and Enter", async () => {
    const people = [
      { id: "anna", name: "Anna" },
      { id: "bob_peters", name: "Bob Peters" },
    ];
    render(
      createElement(SpeakerRenameControl, {
        sessionId: "session-1",
        segment: remoteSegment(),
        transcriptId: "transcript-1",
        color: "red",
        label: "Speaker 2",
        people,
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: "Speaker 2" }));
    const input = screen.getByRole("textbox") as HTMLInputElement;
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => {
      expect(assignTranscriptSpeakerMock).toHaveBeenCalledWith(
        expect.objectContaining({ speakerLabel: "bob_peters" }),
        expect.any(Function),
      );
    });
    expect(ensurePersonMock).not.toHaveBeenCalled();
  });

  it("discards the draft on Escape without saving", () => {
    render(
      createElement(SpeakerRenameControl, {
        sessionId: "session-1",
        segment: remoteSegment(),
        transcriptId: "transcript-1",
        color: "red",
        label: "Speaker 2",
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: "Speaker 2" }));
    const input = screen.getByRole("textbox") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "Bob" } });
    fireEvent.keyDown(input, { key: "Escape" });

    expect(assignTranscriptSpeakerMock).not.toHaveBeenCalled();
    expect(ensurePersonMock).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Speaker 2" })).toBeTruthy();
  });

  it("does not assign when the trimmed value is empty or unchanged", async () => {
    render(
      createElement(SpeakerRenameControl, {
        sessionId: "session-1",
        segment: remoteSegment(),
        transcriptId: "transcript-1",
        color: "red",
        label: "Speaker 2",
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: "Speaker 2" }));
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "   " } });
    fireEvent.blur(screen.getByRole("textbox"));

    expect(assignTranscriptSpeakerMock).not.toHaveBeenCalled();
    expect(ensurePersonMock).not.toHaveBeenCalled();
  });

  it("shows the typed name immediately while the assignment is pending", async () => {
    let resolveAssign!: () => void;
    assignTranscriptSpeakerMock.mockImplementation(
      () => new Promise<void>((resolve) => (resolveAssign = resolve)),
    );
    render(
      createElement(SpeakerRenameControl, {
        sessionId: "session-1",
        segment: remoteSegment(),
        transcriptId: "transcript-1",
        color: "red",
        label: "Speaker 2",
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: "Speaker 2" }));
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "Alice Smith" },
    });
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });

    // Optimistic: the new name shows before ensurePerson/assign settle.
    expect(screen.getByRole("button", { name: "Alice Smith" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Speaker 2" })).toBeNull();

    await waitFor(() => expect(assignTranscriptSpeakerMock).toHaveBeenCalled());
    resolveAssign();
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Alice Smith" })).toBeTruthy(),
    );
  });

  it("shows the clicked person's name immediately while the assignment is pending", async () => {
    const people = [{ id: "bob_peters", name: "Bob Peters" }];
    assignTranscriptSpeakerMock.mockImplementation(() => new Promise(() => {}));
    render(
      createElement(SpeakerRenameControl, {
        sessionId: "session-1",
        segment: remoteSegment(),
        transcriptId: "transcript-1",
        color: "red",
        label: "Speaker 2",
        people,
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: "Speaker 2" }));
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "bo" } });
    fireEvent.click(screen.getByRole("option", { name: "Bob Peters" }));

    expect(screen.getByRole("button", { name: "Bob Peters" })).toBeTruthy();
  });

  it("hands display back to the label prop once the store round trip lands", async () => {
    const onAssigned = vi.fn();
    const props = {
      sessionId: "session-1",
      segment: remoteSegment(),
      transcriptId: "transcript-1",
      color: "red",
      label: "Speaker 2",
      onAssigned,
    };
    const { rerender } = render(createElement(SpeakerRenameControl, props));

    fireEvent.click(screen.getByRole("button", { name: "Speaker 2" }));
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "Alice Smith" },
    });
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
    expect(screen.getByRole("button", { name: "Alice Smith" })).toBeTruthy();

    await waitFor(() => expect(onAssigned).toHaveBeenCalled());
    rerender(
      createElement(SpeakerRenameControl, { ...props, label: "Alice S." }),
    );
    expect(screen.getByRole("button", { name: "Alice S." })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Alice Smith" })).toBeNull();
  });

  it("keeps the newest pending label when an older rename's label change lands first", async () => {
    let resolveAssign!: () => void;
    assignTranscriptSpeakerMock.mockImplementation(
      () => new Promise<void>((resolve) => (resolveAssign = resolve)),
    );
    const props = {
      sessionId: "session-1",
      segment: remoteSegment(),
      transcriptId: "transcript-1",
      color: "red",
      label: "Speaker 2",
    };
    const { rerender } = render(createElement(SpeakerRenameControl, props));

    fireEvent.click(screen.getByRole("button", { name: "Speaker 2" }));
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "Bob" },
    });
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
    await waitFor(() => expect(assignTranscriptSpeakerMock).toHaveBeenCalled());

    // A label change arriving while Bob's commit is still in flight (an older
    // rename's round trip) must not evict the newest optimistic label.
    rerender(createElement(SpeakerRenameControl, { ...props, label: "Alice" }));
    expect(screen.getByRole("button", { name: "Bob" })).toBeTruthy();

    resolveAssign();
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: "Bob" })).toBeNull(),
    );
    expect(screen.getByRole("button", { name: "Alice" })).toBeTruthy();
  });

  it("clears the pending label when the rename resolves to the name already shown", async () => {
    // ensurePerson reuses people case-insensitively: retyping "bob peters" over
    // "Bob Peters" resolves to the existing person and the label prop never
    // changes, so the raw typed text must not stick around.
    ensurePersonMock.mockResolvedValue({
      id: "bob_peters",
      name: "Bob Peters",
    });
    render(
      createElement(SpeakerRenameControl, {
        sessionId: "session-1",
        segment: remoteSegment(),
        transcriptId: "transcript-1",
        color: "red",
        label: "Bob Peters",
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: "Bob Peters" }));
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "bob peters" },
    });
    fireEvent.blur(screen.getByRole("textbox"));
    expect(screen.getByRole("button", { name: "bob peters" })).toBeTruthy();

    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Bob Peters" })).toBeTruthy(),
    );
    expect(screen.queryByRole("button", { name: "bob peters" })).toBeNull();
  });

  it("reverts to the previous label when the assignment fails", async () => {
    assignTranscriptSpeakerMock.mockRejectedValue(new Error("write failed"));
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});
    render(
      createElement(SpeakerRenameControl, {
        sessionId: "session-1",
        segment: remoteSegment(),
        transcriptId: "transcript-1",
        color: "red",
        label: "Speaker 2",
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: "Speaker 2" }));
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "Bob" },
    });
    fireEvent.blur(screen.getByRole("textbox"));

    expect(screen.getByRole("button", { name: "Bob" })).toBeTruthy();

    await waitFor(() => expect(consoleError).toHaveBeenCalled());
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Speaker 2" })).toBeTruthy(),
    );
    expect(screen.queryByRole("button", { name: "Bob" })).toBeNull();
    consoleError.mockRestore();
  });

  it("skips the assignment when ensurePerson fails", async () => {
    ensurePersonMock.mockRejectedValue(new Error("disk full"));
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});
    render(
      createElement(SpeakerRenameControl, {
        sessionId: "session-1",
        segment: remoteSegment(),
        transcriptId: "transcript-1",
        color: "red",
        label: "Speaker 2",
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: "Speaker 2" }));
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "Bob" },
    });
    fireEvent.blur(screen.getByRole("textbox"));

    await waitFor(() => expect(consoleError).toHaveBeenCalled());
    expect(assignTranscriptSpeakerMock).not.toHaveBeenCalled();
    consoleError.mockRestore();
  });
});

describe("SpeakerRenameControl on a diarized channel", () => {
  function diarizedTranscript(withAssignment: boolean) {
    return {
      id: "transcript-1",
      ownerUserId: "self",
      sessionId: "session-1",
      startedAt: 0,
      words: [
        { id: "word-1", text: "hello", start_ms: 0, end_ms: 100, channel: 1 },
        { id: "word-9", text: "there", start_ms: 100, end_ms: 200, channel: 1 },
      ],
      speakerHints: [
        {
          word_id: "word-1",
          type: "provider_speaker_index",
          value: JSON.stringify({ channel: 1, speaker_index: 2 }),
        },
        {
          word_id: "word-9",
          type: "provider_speaker_index",
          value: JSON.stringify({ channel: 1, speaker_index: 0 }),
        },
        ...(withAssignment
          ? [{ word_id: "word-9", type: "speaker_label", value: "kim" }]
          : []),
      ],
    };
  }

  it("assigns every speaker index on the channel when the checkbox is checked", async () => {
    const transcript = diarizedTranscript(false);
    render(
      createElement(SpeakerRenameControl, {
        sessionId: "session-1",
        segment: remoteSegment(),
        transcriptId: "transcript-1",
        color: "red",
        label: "Speaker 2",
        channelAssignmentState:
          buildChannelAssignmentStates(transcript).get("RemoteParty"),
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: "Speaker 2" }));
    fireEvent.click(screen.getByRole("checkbox"));

    const input = screen.getByRole("textbox") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "Alice" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => {
      expect(assignTranscriptSpeakerMock).toHaveBeenCalledTimes(2);
    });
    expect(
      assignTranscriptSpeakerMock.mock.calls.map(([call]) => call),
    ).toEqual([
      {
        transcriptId: "transcript-1",
        segmentKey: {
          channel: "RemoteParty",
          speaker_index: 2,
          speaker_human_id: null,
        },
        speakerLabel: "alice",
        anchorWordId: "word-1",
      },
      {
        transcriptId: "transcript-1",
        segmentKey: {
          channel: "RemoteParty",
          speaker_index: 0,
          speaker_human_id: null,
        },
        speakerLabel: "alice",
        anchorWordId: "word-9",
      },
    ]);
  });

  it("defaults to assigning only the clicked cluster", async () => {
    const transcript = diarizedTranscript(false);
    render(
      createElement(SpeakerRenameControl, {
        sessionId: "session-1",
        segment: remoteSegment(),
        transcriptId: "transcript-1",
        color: "red",
        label: "Speaker 2",
        channelAssignmentState:
          buildChannelAssignmentStates(transcript).get("RemoteParty"),
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: "Speaker 2" }));
    const checkbox = screen.getByRole("checkbox");
    expect(checkbox.getAttribute("data-state")).toBe("unchecked");

    const input = screen.getByRole("textbox") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "Alice" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => {
      expect(assignTranscriptSpeakerMock).toHaveBeenCalledTimes(1);
    });
    expect(assignTranscriptSpeakerMock).toHaveBeenCalledWith(
      {
        transcriptId: "transcript-1",
        segmentKey: {
          channel: "RemoteParty",
          speaker_index: 2,
          speaker_human_id: null,
        },
        speakerLabel: "alice",
        anchorWordId: "word-1",
      },
      expect.any(Function),
    );
  });

  it("skips the channel-wide offer once the channel has an assignment", async () => {
    const transcript = diarizedTranscript(true);
    render(
      createElement(SpeakerRenameControl, {
        sessionId: "session-1",
        segment: remoteSegment(),
        transcriptId: "transcript-1",
        color: "red",
        label: "Speaker 2",
        channelAssignmentState:
          buildChannelAssignmentStates(transcript).get("RemoteParty"),
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: "Speaker 2" }));
    expect(screen.queryByRole("checkbox")).toBeNull();

    const input = screen.getByRole("textbox") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "Alice" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => {
      expect(assignTranscriptSpeakerMock).toHaveBeenCalledTimes(1);
    });
    expect(assignTranscriptSpeakerMock).toHaveBeenCalledWith(
      expect.objectContaining({
        segmentKey: {
          channel: "RemoteParty",
          speaker_index: 2,
          speaker_human_id: null,
        },
        anchorWordId: "word-1",
      }),
      expect.any(Function),
    );
  });
});

describe("getAssignmentAnchorWordId", () => {
  it("uses the first available word id in the segment", () => {
    const segment = {
      id: "segment-1",
      key: {
        channel: "RemoteParty",
        speaker_index: 1,
        speaker_human_id: null,
      },
      speaker_label: "Speaker 1",
      start_ms: 0,
      end_ms: 200,
      text: "hello there",
      words: [
        {
          text: "hello",
          start_ms: 0,
          end_ms: 100,
          channel: "RemoteParty",
          is_final: true,
        },
        {
          id: "word-2",
          text: "there",
          start_ms: 100,
          end_ms: 200,
          channel: "RemoteParty",
          is_final: true,
        },
      ],
    } as Segment;

    expect(getAssignmentAnchorWordId(segment)).toBe("word-2");
  });
});

describe("captured speaker mutation permission", () => {
  it("stops a rename when recording disables the control during ensure", async () => {
    let finish!: (value: { id: string; name: string }) => void;
    ensurePersonMock.mockImplementationOnce(
      () =>
        new Promise((done) => {
          finish = done;
        }),
    );
    const props = {
      sessionId: "session-1",
      transcriptId: "transcript-1",
      segment: remoteSegment(),
      color: "black",
      label: "Speaker",
      disabled: false,
      onAssigned: vi.fn(),
    };
    const view = render(createElement(SpeakerRenameControl, props));
    fireEvent.click(screen.getByRole("button", { name: "Speaker" }));
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "Pending" },
    });
    fireEvent.blur(screen.getByRole("textbox"));
    await waitFor(() => expect(ensurePersonMock).toHaveBeenCalledOnce());
    view.rerender(
      createElement(SpeakerRenameControl, { ...props, disabled: true }),
    );
    await act(async () => {
      finish({ id: "pending", name: "Pending" });
    });
    expect(assignTranscriptSpeakerMock).not.toHaveBeenCalled();
    expect(props.onAssigned).not.toHaveBeenCalled();
  });

  it("disables unsupported/remote controls before ensure or assignment", () => {
    source.scope.client.features = [];
    render(
      createElement(SpeakerRenameControl, {
        sessionId: "session-1",
        transcriptId: "transcript-1",
        segment: remoteSegment(),
        color: "black",
        label: "Speaker",
      }),
    );
    const button = screen.getByRole("button", { name: "Speaker" });
    expect(button.hasAttribute("disabled")).toBe(true);
    fireEvent.click(button);
    expect(ensurePersonMock).not.toHaveBeenCalled();
    expect(assignTranscriptSpeakerMock).not.toHaveBeenCalled();
  });
  it("does not assign after permission is lost while ensurePerson is pending", async () => {
    let finish!: (value: { id: string; name: string }) => void;
    ensurePersonMock.mockImplementationOnce(
      () =>
        new Promise((done) => {
          finish = done;
        }),
    );
    render(
      createElement(SpeakerRenameControl, {
        sessionId: "session-1",
        transcriptId: "transcript-1",
        segment: remoteSegment(),
        color: "black",
        label: "Speaker",
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Speaker" }));
    const input = screen.getByRole("textbox");
    fireEvent.change(input, { target: { value: "Private person" } });
    fireEvent.blur(input);
    await waitFor(() => expect(ensurePersonMock).toHaveBeenCalledOnce());
    source.scope.client.canEdit = false;
    await act(async () => {
      finish({ id: "private", name: "Private person" });
    });
    await waitFor(() => expect(screen.queryByRole("textbox")).toBeNull());
    expect(assignTranscriptSpeakerMock).not.toHaveBeenCalled();
  });
  it.each(["session", "scope"])(
    "refuses an old pending rename after %s replacement",
    async (kind) => {
      let finish!: (value: { id: string; name: string }) => void;
      ensurePersonMock.mockImplementationOnce(
        () =>
          new Promise((done) => {
            finish = done;
          }),
      );
      const props = {
        sessionId: "session-1",
        transcriptId: "same-transcript",
        segment: remoteSegment(),
        color: "black",
        label: "Speaker",
        onAssigned: vi.fn(),
      };
      const mounted = render(createElement(SpeakerRenameControl, props));
      fireEvent.click(screen.getByRole("button", { name: "Speaker" }));
      fireEvent.change(screen.getByRole("textbox"), {
        target: { value: "Pending person" },
      });
      fireEvent.blur(screen.getByRole("textbox"));
      await waitFor(() => expect(ensurePersonMock).toHaveBeenCalledOnce());
      if (kind === "scope")
        source.scope = { ...source.scope, client: { ...source.scope.client } };
      mounted.rerender(
        createElement(SpeakerRenameControl, {
          ...props,
          sessionId: kind === "session" ? "other-session" : props.sessionId,
          label: "Replacement speaker",
        }),
      );
      await act(async () => {
        finish({ id: "pending", name: "Pending person" });
      });
      expect(assignTranscriptSpeakerMock).not.toHaveBeenCalled();
      expect(props.onAssigned).not.toHaveBeenCalled();
      expect(
        screen.getByRole("button", { name: "Replacement speaker" }),
      ).toBeDefined();
    },
  );
});
