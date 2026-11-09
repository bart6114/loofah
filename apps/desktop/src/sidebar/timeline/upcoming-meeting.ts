import { useLingui } from "@lingui/react/macro";
import { useCallback, useMemo, useSyncExternalStore } from "react";

import { useNearestTimelineBuckets } from "./queries";
import { type TimelineBucket } from "./utils";

const UPCOMING_MEETING_VISIBLE_WINDOW_MS = 5 * 60 * 1000;
const UPCOMING_MEETING_STATUS_TICK_MS = 1_000;

export type SidebarUpcomingMeetingStatus = {
  itemKey: string;
  label: string;
  title: string;
  progress?: number;
};

export function useSidebarUpcomingMeetingStatus(): SidebarUpcomingMeetingStatus | null {
  const formatLabel = useUpcomingMeetingLabelFormatter();
  const buckets = useNearestTimelineBuckets();

  return useUpcomingMeetingStatus(buckets, formatLabel);
}

export function useUpcomingMeetingStatus(
  buckets: TimelineBucket[],
  formatLabel: (diffMs: number) => string,
): SidebarUpcomingMeetingStatus | null {
  const store = useMemo(
    () => createUpcomingMeetingStatusStore(buckets, formatLabel),
    [buckets, formatLabel],
  );

  return useSyncExternalStore(
    store.subscribe,
    store.getSnapshot,
    store.getSnapshot,
  );
}

function createUpcomingMeetingStatusStore(
  buckets: TimelineBucket[],
  formatLabel: (diffMs: number) => string,
) {
  const getCurrentStatus = () =>
    getUpcomingMeetingStatus(
      buckets,
      Math.floor(Date.now() / UPCOMING_MEETING_STATUS_TICK_MS) *
        UPCOMING_MEETING_STATUS_TICK_MS,
      formatLabel,
    );
  let status = getCurrentStatus();
  const listeners = new Set<() => void>();

  const refresh = () => {
    const nextStatus = getCurrentStatus();
    if (upcomingMeetingStatusesAreEqual(status, nextStatus)) {
      return;
    }

    status = nextStatus;
    listeners.forEach((listener) => listener());
  };

  const subscribe = (listener: () => void) => {
    listeners.add(listener);
    const interval = setInterval(refresh, UPCOMING_MEETING_STATUS_TICK_MS);
    const handleVisibilityChange = () => {
      if (document.visibilityState === "visible") {
        refresh();
      }
    };

    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", handleVisibilityChange);
    refresh();

    return () => {
      listeners.delete(listener);
      clearInterval(interval);
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", handleVisibilityChange);
    };
  };

  return {
    getSnapshot: () => status,
    subscribe,
  };
}

export function getUpcomingMeetingStatus(
  buckets: TimelineBucket[],
  currentTimeMs: number,
  formatLabel: (diffMs: number) => string = formatUpcomingMeetingLabelEnglish,
): SidebarUpcomingMeetingStatus | null {
  let nearest: { itemKey: string; title: string; diffMs: number } | null = null;

  for (const bucket of buckets) {
    for (const item of bucket.items) {
      if (item.timestampMs === null) {
        continue;
      }

      const diffMs = item.timestampMs - currentTimeMs;
      if (diffMs <= 0 || diffMs > UPCOMING_MEETING_VISIBLE_WINDOW_MS) {
        continue;
      }

      if (!nearest || diffMs < nearest.diffMs) {
        nearest = {
          itemKey: `${item.type}-${item.id}`,
          title: item.data.title || "Untitled",
          diffMs,
        };
      }
    }
  }

  if (!nearest) {
    return null;
  }

  return {
    itemKey: nearest.itemKey,
    label: formatLabel(nearest.diffMs),
    progress: getUpcomingMeetingProgress(nearest.diffMs),
    title: nearest.title,
  };
}

function getUpcomingMeetingProgress(diffMs: number): number {
  return Math.max(0, Math.min(diffMs / UPCOMING_MEETING_VISIBLE_WINDOW_MS, 1));
}

function upcomingMeetingStatusesAreEqual(
  left: SidebarUpcomingMeetingStatus | null,
  right: SidebarUpcomingMeetingStatus | null,
) {
  return (
    left === right ||
    (left !== null &&
      right !== null &&
      left.itemKey === right.itemKey &&
      left.label === right.label &&
      left.title === right.title &&
      left.progress === right.progress)
  );
}

export function useUpcomingMeetingLabelFormatter(): (diffMs: number) => string {
  const { t } = useLingui();

  return useCallback(
    (diffMs: number) => {
      const totalSeconds = Math.max(1, Math.floor(diffMs / 1000));
      const minutes = Math.floor(totalSeconds / 60);
      const seconds = totalSeconds % 60;

      if (minutes < 1) {
        return t`In ${totalSeconds}s`;
      }

      return t`In ${minutes}m ${seconds}s`;
    },
    [t],
  );
}

function formatUpcomingMeetingLabelEnglish(diffMs: number): string {
  const totalSeconds = Math.max(1, Math.floor(diffMs / 1000));
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;

  if (minutes < 1) {
    return `In ${totalSeconds}s`;
  }

  return `In ${minutes}m ${seconds}s`;
}
