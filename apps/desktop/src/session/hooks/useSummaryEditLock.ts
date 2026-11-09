import { getCurrentWebviewWindowLabel } from "@hypr/plugin-windows";

import { useAITask } from "~/ai/contexts";
import { useLegacySessionActivity } from "~/main/use-legacy-activity";
import { useSummaryActivity } from "~/session/summary-activity";
import { createTaskId } from "~/store/zustand/ai-task/task-configs";

export function useSummaryEditLock(sessionId: string) {
  const legacy = useLegacySessionActivity(sessionId);
  const { isAvailable } = legacy;
  const allowed = isAvailable("legacy_desktop_enhance");
  const activity = useSummaryActivity((state) =>
    allowed ? state.sessions[sessionId] : undefined,
  );
  const remoteActivity = useSummaryActivity((state) =>
    allowed ? state.remoteSessions[sessionId] : undefined,
  );
  const remoteReady = useSummaryActivity((state) =>
    allowed ? state.remoteReady : true,
  );
  const mode = legacy.mode;
  const starting = legacy.starting;
  const taskId = createTaskId(sessionId, "enhance");
  const status = useAITask((state) =>
    allowed ? state.tasks[taskId]?.status : undefined,
  );

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
