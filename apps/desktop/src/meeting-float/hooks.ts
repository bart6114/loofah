import { subscribeIndexChanged } from "~/shared/index-query";
import { DEFAULT_USER_ID } from "~/shared/utils";
import type { RenderLabelContext } from "~/stt/live-segment";
import { commands, type SessionListHeader } from "~/types/tauri.gen";

export type MeetingFloatData = {
  sessions: Record<
    string,
    {
      title: string;
      ownerUserId: string;
    }
  >;
};

export async function loadMeetingFloatData(): Promise<MeetingFloatData> {
  const result = await commands.sessionListHeaders();
  if (result.status === "error") {
    throw new Error(result.error);
  }
  return mapMeetingFloatEntries(result.data);
}

export async function subscribeMeetingFloatData(
  onData: (data: MeetingFloatData) => void,
  onError: (error: string) => void,
): Promise<() => Promise<void>> {
  const push = async () => {
    try {
      onData(await loadMeetingFloatData());
    } catch (error) {
      onError(error instanceof Error ? error.message : String(error));
    }
  };

  const unsubscribe = subscribeIndexChanged("session_headers", () => {
    void push();
  });
  await push();

  return async () => {
    unsubscribe();
  };
}

export function createMeetingFloatLabelContext(
  data: MeetingFloatData,
  sessionId: string,
): RenderLabelContext {
  const session = data.sessions[sessionId];
  return {
    getSelfHumanId: () => session?.ownerUserId || undefined,
    getHumanName: (speakerLabel) => speakerLabel || undefined,
    getParticipantHumanIds: () => [],
  };
}

function mapMeetingFloatEntries(
  entries: SessionListHeader[],
): MeetingFloatData {
  const sessions: MeetingFloatData["sessions"] = {};

  for (const entry of entries) {
    sessions[entry.id] = {
      title: entry.title,
      // The owner concept died with the workspaces removal (D10).
      ownerUserId: DEFAULT_USER_ID,
    };
  }

  return { sessions };
}
