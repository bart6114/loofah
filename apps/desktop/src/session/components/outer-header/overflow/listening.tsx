import { MicIcon, MicOffIcon } from "lucide-react";

import { DropdownMenuItem } from "@hypr/ui/components/ui/dropdown-menu";

import { useLegacySessionActivity } from "~/main/use-legacy-activity";
import { useStartListening } from "~/stt/useStartListening";
import {
  isMainWebviewWindow,
  requestMainListenerControl,
} from "~/stt/window-control";

export function Listening({
  sessionId,
  resume,
  onStartRecording,
}: {
  sessionId: string;
  resume: boolean;
  onStartRecording?: () => void;
}) {
  const legacy = useLegacySessionActivity(sessionId);
  const { isAvailable } = legacy;
  if (!isAvailable()) return null;
  return (
    <LegacyListening
      sessionId={sessionId}
      resume={resume}
      onStartRecording={onStartRecording}
    />
  );
}

function LegacyListening({
  sessionId,
  resume,
  onStartRecording,
}: {
  sessionId: string;
  resume: boolean;
  onStartRecording?: () => void;
}) {
  const legacy = useLegacySessionActivity(sessionId);
  const { isAvailable } = legacy;
  const mode = legacy.mode;
  const stop = () => legacy.listener!.getState().stop();
  const isListening = mode === "active" || mode === "finalizing";
  const isFinalizing = mode === "finalizing";
  const isBatching = mode === "running_batch";
  const startListening = useStartListening(sessionId, isAvailable);

  const handleToggleListening = () => {
    if (!isAvailable() || isBatching) {
      return;
    }

    if (!isListening) onStartRecording?.();

    if (!isMainWebviewWindow()) {
      void requestMainListenerControl(
        isListening ? "stop" : "start",
        sessionId,
      );
      return;
    }

    if (isListening) {
      stop();
    } else {
      startListening();
    }
  };

  const startLabel = resume ? "Resume listening" : "Start listening";

  return (
    <DropdownMenuItem
      className="cursor-pointer"
      onClick={handleToggleListening}
      disabled={isFinalizing || isBatching}
    >
      {isListening ? <MicOffIcon /> : <MicIcon />}
      <span>
        {isBatching
          ? "Batch processing"
          : isListening
            ? "Stop listening"
            : startLabel}
      </span>
    </DropdownMenuItem>
  );
}
