import type {
  ChannelProfile as BoundChannelProfile,
  LiveTranscriptSegment,
  RenderedTranscriptSegment,
  SegmentKey as BoundSegmentKey,
  SegmentWord as BoundSegmentWord,
} from "@hypr/plugin-transcription";

import type { TranscriptWordMetadata } from "~/stt/timing";

export enum ChannelProfile {
  DirectMic = 0,
  RemoteParty = 1,
  MixedCapture = 2,
}

export type WordLike = {
  text: string;
  start_ms: number;
  end_ms: number;
  channel: ChannelProfile;
  metadata?: TranscriptWordMetadata | null;
};

export type PartialWord = WordLike;

type SpeakerHintData = {
  type: "provider_speaker_index";
  speaker_index: number;
  provider?: string;
  channel?: number;
};

export type RuntimeSpeakerHint = {
  wordIndex: number;
  data: SpeakerHintData;
};

export type RenderLabelContext = {
  getSelfHumanId: () => string | undefined;
  getHumanName: (id: string) => string | undefined;
  getParticipantHumanIds?: () => string[];
};

export type SegmentKey = BoundSegmentKey;
export type SegmentWord = BoundSegmentWord & {
  metadata?: TranscriptWordMetadata | null;
};
type SegmentWithWordMetadata<T extends { words: BoundSegmentWord[] }> = Omit<
  T,
  "words"
> & {
  words: SegmentWord[];
};
export type Segment =
  | SegmentWithWordMetadata<LiveTranscriptSegment>
  | SegmentWithWordMetadata<RenderedTranscriptSegment>;
export type SegmentChannelProfile = BoundChannelProfile;

export class SpeakerLabelManager {
  private unknownSpeakerMap: Map<string, number> = new Map();
  private nextIndex = 1;

  constructor(
    private readonly maxUnknownSpeakerNumber?: number,
    private readonly diarizedChannels?: ReadonlySet<SegmentChannelProfile>,
  ) {}

  isChannelDiarized(channel: SegmentChannelProfile): boolean {
    return this.diarizedChannels?.has(channel) ?? false;
  }

  getUnknownSpeakerNumber(key: SegmentKey): number {
    const serialized = SegmentKeyUtils.serialize(key);
    const existing = this.unknownSpeakerMap.get(serialized);
    if (existing !== undefined) {
      return existing;
    }

    const newIndex =
      this.maxUnknownSpeakerNumber && this.maxUnknownSpeakerNumber > 0
        ? Math.min(this.nextIndex, this.maxUnknownSpeakerNumber)
        : this.nextIndex;
    this.unknownSpeakerMap.set(serialized, newIndex);
    this.nextIndex += 1;
    return newIndex;
  }

  static fromSegments(
    segments: Segment[],
    ctx?: RenderLabelContext,
    maxUnknownSpeakerNumber?: number,
  ): SpeakerLabelManager {
    const diarizedChannels = getDiarizedChannels(segments);
    // Once a channel is diarized the participant list no longer bounds the
    // speaker count -- capping would merge two distinct diarized speakers
    // under one label.
    const manager = new SpeakerLabelManager(
      diarizedChannels.size > 0 ? undefined : maxUnknownSpeakerNumber,
      diarizedChannels,
    );
    for (const segment of segments) {
      if (!SegmentKeyUtils.isKnownSpeaker(segment.key, ctx, diarizedChannels)) {
        manager.getUnknownSpeakerNumber(segment.key);
      }
    }
    return manager;
  }
}

// A channel counts as diarized once >=2 distinct speaker indexes have landed on
// final words -- partials are excluded so a flickering live index cannot flip
// labeling behavior mid-recording.
export function getDiarizedChannels(
  segments: readonly Segment[],
): Set<SegmentChannelProfile> {
  const indexesByChannel = new Map<SegmentChannelProfile, Set<number>>();
  const diarized = new Set<SegmentChannelProfile>();

  for (const segment of segments) {
    const speakerIndex = segment.key.speaker_index;
    if (speakerIndex == null) {
      continue;
    }
    if (!segment.words.some((word) => word.is_final)) {
      continue;
    }

    const indexes =
      indexesByChannel.get(segment.key.channel) ?? new Set<number>();
    indexes.add(speakerIndex);
    indexesByChannel.set(segment.key.channel, indexes);
    if (indexes.size >= 2) {
      diarized.add(segment.key.channel);
    }
  }

  return diarized;
}

export const SegmentKeyUtils = {
  serialize: (key: SegmentKey): string => {
    return JSON.stringify([
      key.channel,
      key.speaker_index ?? null,
      key.speaker_human_id ?? null,
    ]);
  },

  isKnownSpeaker: (
    key: SegmentKey,
    ctx?: RenderLabelContext,
    diarizedChannels?: ReadonlySet<SegmentChannelProfile>,
  ): boolean => {
    if (key.speaker_human_id) {
      return true;
    }

    // Diarized clusters are distinct people; channel-identity heuristics only
    // hold when the channel carries a single voice.
    if (key.speaker_index != null && diarizedChannels?.has(key.channel)) {
      return false;
    }

    if (ctx && key.channel === "DirectMic") {
      return Boolean(ctx.getSelfHumanId());
    }

    if (ctx && key.channel === "RemoteParty") {
      return Boolean(getUniqueRemoteParticipantHumanId(ctx));
    }

    return false;
  },

  renderLabel: (
    key: SegmentKey,
    ctx?: RenderLabelContext,
    manager?: SpeakerLabelManager,
  ): string => {
    const assignedHumanId = key.speaker_human_id;

    if (ctx && assignedHumanId != null) {
      const human = ctx.getHumanName(assignedHumanId);
      if (assignedHumanId === ctx.getSelfHumanId()) {
        return human?.trim() && human !== assignedHumanId ? human : "You";
      }
      if (human) {
        return human;
      }
      return assignedHumanId;
    }

    const heuristicsGated =
      key.speaker_index != null &&
      (manager?.isChannelDiarized(key.channel) ?? false);

    if (
      !heuristicsGated &&
      ctx &&
      key.channel === "DirectMic" &&
      assignedHumanId == null
    ) {
      const selfHumanId = ctx.getSelfHumanId();
      if (selfHumanId) {
        const selfHuman = ctx.getHumanName(selfHumanId);
        return selfHuman?.trim() && selfHuman !== selfHumanId
          ? selfHuman
          : "You";
      }
    }

    if (
      !heuristicsGated &&
      ctx &&
      key.channel === "RemoteParty" &&
      assignedHumanId == null
    ) {
      const remoteHumanId = getUniqueRemoteParticipantHumanId(ctx);
      if (remoteHumanId) {
        return ctx.getHumanName(remoteHumanId) || remoteHumanId;
      }
    }

    if (manager) {
      const speakerNumber = manager.getUnknownSpeakerNumber(key);
      return `Speaker ${speakerNumber}`;
    }

    const channelLabel =
      key.channel === "DirectMic"
        ? "A"
        : key.channel === "RemoteParty"
          ? "B"
          : "C";

    return key.speaker_index !== null && key.speaker_index !== undefined
      ? `Speaker ${key.speaker_index + 1}`
      : `Speaker ${channelLabel}`;
  },
};

function getUniqueRemoteParticipantHumanId(
  ctx: RenderLabelContext,
): string | undefined {
  const selfHumanId = ctx.getSelfHumanId();
  const participantHumanIds = ctx.getParticipantHumanIds?.() ?? [];
  const remoteHumanIds = [
    ...new Set(
      participantHumanIds.filter(
        (humanId) => humanId && humanId !== selfHumanId,
      ),
    ),
  ];

  return remoteHumanIds.length === 1 ? remoteHumanIds[0] : undefined;
}

export function getMaxSpeakerNumberForParticipants(
  participantHumanIds: readonly string[],
  selfHumanId?: string | null,
): number | undefined {
  const ids = new Set(participantHumanIds.filter(Boolean));
  if (selfHumanId) {
    ids.add(selfHumanId);
  }

  return ids.size > 1 ? ids.size : undefined;
}

export function mergeRenderedAndLiveSegments(
  renderedSegments: Segment[],
  liveSegments: Segment[],
): Segment[] {
  if (liveSegments.length === 0) {
    return renderedSegments;
  }

  if (renderedSegments.length === 0) {
    return liveSegments;
  }

  const liveSegmentIds = new Set(
    liveSegments
      .map((segment) => segment.id)
      .filter((id): id is string => typeof id === "string" && id.length > 0),
  );
  const liveWordIds = new Set(
    liveSegments.flatMap((segment) =>
      segment.words
        .map((word) => word.id)
        .filter((id): id is string => typeof id === "string" && id.length > 0),
    ),
  );
  const renderedOnlySegments = renderedSegments.filter((segment) => {
    if (segment.id && liveSegmentIds.has(segment.id)) {
      return false;
    }

    return !segment.words.some((word) => word.id && liveWordIds.has(word.id));
  });

  return [...renderedOnlySegments, ...liveSegments];
}
