import { useAITask } from "~/ai/contexts";
import { useLegacyDesktopAccess } from "~/main/use-legacy-activity";

const EMPTY: string[] = [];

// One store subscription for the whole timeline instead of a per-row
// enhanced-docs query; streaming chunks keep status "generating", so the
// shallow-compared selector output only changes when a task starts or stops.
export function useEnhancingSessionIds(): string[] {
  const legacy = useLegacyDesktopAccess();
  const { isAvailable } = legacy;
  return useAITask((state) => {
    if (!isAvailable("legacy_desktop_enhance")) return EMPTY;
    let ids: string[] | null = null;
    for (const task of Object.values(state.tasks)) {
      if (
        task.taskType === "enhance" &&
        task.status === "generating" &&
        task.sessionId
      ) {
        (ids ??= []).push(task.sessionId);
      }
    }
    return ids ? ids.sort() : EMPTY;
  });
}
