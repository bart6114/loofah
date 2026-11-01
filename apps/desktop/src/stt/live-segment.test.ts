import { describe, expect, it } from "vitest";

import {
  getMaxSpeakerNumberForParticipants,
  SegmentKeyUtils,
  SpeakerLabelManager,
  type RenderLabelContext,
  type Segment,
} from "./live-segment";

function diarizedSegment(
  channel: "DirectMic" | "RemoteParty",
  speakerIndex: number,
): Segment {
  return {
    id: `segment-${channel}-${speakerIndex}`,
    key: {
      channel,
      speaker_index: speakerIndex,
      speaker_human_id: null,
    },
    words: [
      {
        id: `word-${channel}-${speakerIndex}`,
        text: "hi",
        start_ms: 0,
        end_ms: 100,
        channel,
        is_final: true,
      },
    ],
    start_ms: 0,
    end_ms: 100,
    text: "hi",
  } as Segment;
}

const ctx: RenderLabelContext = {
  getSelfHumanId: () => "self",
  getHumanName: (id) => (id === "self" ? "Me" : undefined),
};
const twoPersonCtx: RenderLabelContext = {
  getSelfHumanId: () => "self",
  getHumanName: (id) =>
    id === "self" ? "Me" : id === "remote" ? "Artem" : undefined,
  getParticipantHumanIds: () => ["self", "remote"],
};

describe("SegmentKeyUtils", () => {
  it.each([undefined, "", "   ", "00000000-0000-0000-0000-000000000000"])(
    "renders self as You when the resolved name is %s",
    (name) => {
      const selfId = "00000000-0000-0000-0000-000000000000";
      const selfContext: RenderLabelContext = {
        getSelfHumanId: () => selfId,
        getHumanName: () => name,
      };
      for (const assignedId of [null, selfId]) {
        expect(
          SegmentKeyUtils.renderLabel(
            {
              channel: "DirectMic",
              speaker_index: null,
              speaker_human_id: assignedId,
            },
            selfContext,
          ),
        ).toBe("You");
      }
    },
  );

  it("treats diarized direct-mic segments as self", () => {
    const key: Parameters<typeof SegmentKeyUtils.isKnownSpeaker>[0] = {
      channel: "DirectMic",
      speaker_index: 2,
      speaker_human_id: null,
    };

    expect(SegmentKeyUtils.isKnownSpeaker(key, ctx)).toBe(true);
    expect(SegmentKeyUtils.renderLabel(key, ctx)).toBe("Me");
  });

  it("renders an assigned but unresolved human id as the raw id", () => {
    const key: Parameters<typeof SegmentKeyUtils.renderLabel>[0] = {
      channel: "DirectMic",
      speaker_index: 1,
      speaker_human_id: "bob_peters",
    };

    expect(SegmentKeyUtils.renderLabel(key, ctx)).toBe("bob_peters");
  });

  it("never leaks the raw self id for heuristic self-assigned segments", () => {
    const selfUuid = "8b9f4a2e-usr-uuid";
    const uuidCtx: RenderLabelContext = {
      getSelfHumanId: () => selfUuid,
      getHumanName: () => undefined,
    };
    const key: Parameters<typeof SegmentKeyUtils.renderLabel>[0] = {
      channel: "DirectMic",
      speaker_index: null,
      speaker_human_id: selfUuid,
    };

    expect(SegmentKeyUtils.renderLabel(key, uuidCtx)).toBe("You");
  });

  it("caps unknown speaker labels when a participant max is provided", () => {
    const segments: Segment[] = [0, 1, 2].map(
      (speakerIndex) =>
        ({
          id: `segment-${speakerIndex}`,
          key: {
            channel: "RemoteParty",
            speaker_index: speakerIndex,
            speaker_human_id: null,
          },
          words: [],
          start_ms: 0,
          end_ms: 0,
          text: "",
        }) as Segment,
    );
    const manager = SpeakerLabelManager.fromSegments(segments, undefined, 2);

    expect(
      SegmentKeyUtils.renderLabel(segments[0]!.key, undefined, manager),
    ).toBe("Speaker 1");
    expect(
      SegmentKeyUtils.renderLabel(segments[1]!.key, undefined, manager),
    ).toBe("Speaker 2");
    expect(
      SegmentKeyUtils.renderLabel(segments[2]!.key, undefined, manager),
    ).toBe("Speaker 2");
  });

  it("lifts the participant cap when a channel is diarized", () => {
    const segments = [0, 1, 2].map((speakerIndex) =>
      diarizedSegment("RemoteParty", speakerIndex),
    );
    const manager = SpeakerLabelManager.fromSegments(segments, undefined, 2);

    expect(
      segments.map((segment) =>
        SegmentKeyUtils.renderLabel(segment.key, undefined, manager),
      ),
    ).toEqual(["Speaker 1", "Speaker 2", "Speaker 3"]);
  });

  it("does not absorb a diarized remote channel into the unique remote participant", () => {
    const segments = [0, 1].map((speakerIndex) =>
      diarizedSegment("RemoteParty", speakerIndex),
    );
    const manager = SpeakerLabelManager.fromSegments(segments, twoPersonCtx);

    expect(
      segments.map((segment) =>
        SegmentKeyUtils.renderLabel(segment.key, twoPersonCtx, manager),
      ),
    ).toEqual(["Speaker 1", "Speaker 2"]);
    // Channel-only gap segments keep today's heuristic label.
    expect(
      SegmentKeyUtils.renderLabel(
        { channel: "RemoteParty", speaker_index: null, speaker_human_id: null },
        twoPersonCtx,
        manager,
      ),
    ).toBe("Artem");
  });

  it("does not absorb a diarized direct-mic channel into self", () => {
    const segments = [0, 1].map((speakerIndex) =>
      diarizedSegment("DirectMic", speakerIndex),
    );
    const manager = SpeakerLabelManager.fromSegments(segments, ctx);

    expect(
      segments.map((segment) =>
        SegmentKeyUtils.renderLabel(segment.key, ctx, manager),
      ),
    ).toEqual(["Speaker 1", "Speaker 2"]);
    expect(
      SegmentKeyUtils.renderLabel(
        { channel: "DirectMic", speaker_index: null, speaker_human_id: null },
        ctx,
        manager,
      ),
    ).toBe("Me");
  });

  it("keeps single-index channels on the heuristic path", () => {
    const segments = [diarizedSegment("RemoteParty", 0)];
    const manager = SpeakerLabelManager.fromSegments(segments, twoPersonCtx);

    expect(
      SegmentKeyUtils.renderLabel(segments[0]!.key, twoPersonCtx, manager),
    ).toBe("Artem");
  });

  it("labels remote-party segments as the unique other participant", () => {
    const key: Parameters<typeof SegmentKeyUtils.renderLabel>[0] = {
      channel: "RemoteParty",
      speaker_index: 0,
      speaker_human_id: null,
    };

    expect(SegmentKeyUtils.isKnownSpeaker(key, twoPersonCtx)).toBe(true);
    expect(SegmentKeyUtils.renderLabel(key, twoPersonCtx)).toBe("Artem");
  });

  it("derives max speaker number from distinct participants plus self", () => {
    expect(getMaxSpeakerNumberForParticipants(["remote"], "self")).toBe(2);
    expect(getMaxSpeakerNumberForParticipants(["self", "remote"], "self")).toBe(
      2,
    );
    expect(getMaxSpeakerNumberForParticipants([], "self")).toBeUndefined();
  });
});
