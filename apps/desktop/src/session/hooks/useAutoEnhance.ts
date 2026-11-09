import { useEffect, useState, useRef } from "react";

import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { useLegacyDesktopAccess } from "~/main/use-legacy-activity";
import { getEnhancerService } from "~/services/enhancer";
import { type Tab, useTabsStore } from "~/store/zustand/tabs";

export function useAutoEnhance(tab: Extract<Tab, { type: "sessions" }>) {
  const sessionId = tab.id;
  const selectedSession = useRef(sessionId);
  selectedSession.current = sessionId;
  const legacy = useLegacyDesktopAccess();
  const { isAvailable } = legacy;
  const available = isAvailable("legacy_desktop_enhance");
  const tabsStore = useTabsStore();
  const [skipReason, setSkipReason] = useState<string | null>(null);

  useEffect(() => {
    if (!available) return;
    const service = getEnhancerService();
    if (!service) return;
    return service.on((event) => {
      if (
        !isAvailable("legacy_desktop_enhance") ||
        selectedSession.current !== sessionId ||
        event.sessionId !== sessionId
      )
        return;
      if (event.type === "auto-enhance-skipped") {
        setSkipReason(event.reason);
        if (event.reasonCode === "transcript_too_short") {
          sonnerToast.warning("Summary wasn't generated", {
            id: `auto-summary-too-short-${sessionId}`,
            description: event.reason,
          });
        }
      }
      if (event.type === "auto-enhance-started") {
        const tabsState = tabsStore.getState();
        const sessionTab = tabsState.tabs.find(
          (t): t is Extract<Tab, { type: "sessions" }> =>
            t.type === "sessions" && t.id === sessionId,
        );
        if (sessionTab) {
          tabsState.updateSessionTabState(sessionTab, {
            ...sessionTab.state,
            view: { type: "enhanced", id: event.noteId },
          });
        }
      }
      if (event.type === "auto-enhance-no-model") {
        setSkipReason("No AI model configured");
      }
    });
  }, [sessionId, tabsStore, isAvailable, available]);

  useEffect(() => {
    if (skipReason) {
      const timer = setTimeout(() => setSkipReason(null), 5000);
      return () => clearTimeout(timer);
    }
  }, [skipReason]);

  return { skipReason };
}
