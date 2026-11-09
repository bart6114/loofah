import { cn } from "@hypr/utils";

import {
  type ChannelAssignmentState,
  SpeakerRenameControl,
} from "./speaker-assign";
import { useSegmentColorVars } from "./utils";

import type { Person } from "~/people/queries";
import type { Segment } from "~/stt/live-segment";

export function SegmentHeader({
  segment,
  sessionId,
  transcriptId,
  label,
  people,
  channelAssignmentState,
  recording,
}: {
  segment: Segment;
  sessionId: string;
  transcriptId: string;
  label: string;
  people: readonly Person[];
  channelAssignmentState: ChannelAssignmentState;
  recording: boolean;
}) {
  const colorVars = useSegmentColorVars(segment.key);
  const headerClassName = cn([
    "bg-card sticky top-0 z-20",
    "-mx-3 px-3 py-1",
    "timecode text-muted-foreground",
    "flex items-center gap-3",
    "[--segment-color:var(--segment-color-light)]",
    "dark:[--segment-color:var(--segment-color-dark)]",
  ]);

  return (
    <div className={headerClassName} style={colorVars}>
      <SpeakerRenameControl
        sessionId={sessionId}
        segment={segment}
        transcriptId={transcriptId}
        color="var(--segment-color)"
        label={label}
        disabled={recording}
        people={people}
        channelAssignmentState={channelAssignmentState}
      />
    </div>
  );
}
