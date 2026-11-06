import {
  AudioPlayerProvider,
  AudioPlayerStatus,
  useAudioPlayer,
} from "./provider";
import { Timeline } from "./timeline";

import type { VaultScope } from "~/shared/application/vault-scope";

export function RecordingPlayer({
  scope,
  sessionId,
}: {
  scope: VaultScope;
  sessionId: string;
}) {
  return (
    <AudioPlayerProvider scope={scope} sessionId={sessionId}>
      <RecordingControls />
    </AudioPlayerProvider>
  );
}

function RecordingControls() {
  const { audioExists } = useAudioPlayer();
  return (
    <>
      <AudioPlayerStatus />
      {audioExists && <Timeline />}
    </>
  );
}
