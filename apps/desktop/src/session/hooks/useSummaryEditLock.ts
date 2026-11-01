import { getCurrentWebviewWindowLabel } from "@hypr/plugin-windows";

import { useAITask } from "~/ai/contexts";
import { useSummaryActivity } from "~/session/summary-activity";
import { createTaskId } from "~/store/zustand/ai-task/task-configs";
import { useListener } from "~/stt/contexts";

export function useSummaryEditLock(sessionId: string) {
  const activity = useSummaryActivity((state) => state.sessions[sessionId]);
  const remoteActivity = useSummaryActivity(
    (state) => state.remoteSessions[sessionId],
  );
  const remoteReady = useSummaryActivity((state) => state.remoteReady);
  const mode = useListener((state) => state.getSessionMode(sessionId));
  const starting = useListener(
    (state) => state.live.sessionId === sessionId && state.live.loading,
  );
  const taskId = createTaskId(sessionId, "enhance");
  const status = useAITask((state) => state.tasks[taskId]?.status);

  if (mode === "finalizing" || mode === "running_batch") return "processing";
  if (
    activity === "recording" ||
    remoteActivity === "recording" ||
    starting ||
    mode === "active"
  )
    return "recording";
  if (
    activity ||
    remoteActivity ||
    status === "generating" ||
    (!remoteReady && getCurrentWebviewWindowLabel() !== "main")
  )
    return "processing";
  return null;
}
