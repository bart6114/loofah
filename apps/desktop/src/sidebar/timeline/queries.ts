import { useEffect, useMemo } from "react";

import { streamItems } from "./controller";
import { useTimelineSnapshot } from "./owner";
import { headerRow, projectTimeline } from "./projection";
import { buildTimelineBuckets } from "./utils";

import { useConfigValue } from "~/shared/config";
import { useCurrentDay } from "~/shared/hooks/useCurrentDay";
import { useScopedPendingDeletions } from "~/store/zustand/undo-delete";

function useConfiguredTimeline() {
  const timeline = useTimelineSnapshot();
  const timezone = useConfigValue("timezone") || undefined;
  const day = useCurrentDay(timezone);
  const groupBy = useConfigValue("sidebar_group_by") === "tag" ? "tag" : "date";
  const expanded = useConfigValue("sidebar_expanded_tags");
  const { owner } = timeline;
  useEffect(
    () => owner.configure(timezone, day, groupBy, expanded),
    [owner, timezone, day, groupBy, expanded],
  );
  return { ...timeline, timezone };
}

export function usePagedTimeline() {
  const timeline = useConfiguredTimeline();
  const { owner, state, available, timezone } = timeline;
  const pending = useScopedPendingDeletions(
    owner.scope.client.scope,
    owner.scope,
  );
  useEffect(() => {
    if (!available || state !== owner.getSnapshot()) return;
    if (state.groupBy === "tag") owner.ensureFacets();
    else {
      owner.ensure("past", { kind: "date", before_ms: state.now + 1 });
      owner.ensure("future", {
        kind: "upcoming",
        after_ms: state.now,
        before_ms: state.allFuture ? null : state.cutoff,
      });
      if (state.undatedOpen) owner.ensure("undated", { kind: "undated" });
    }
  }, [owner, available, state]);
  const projected = useMemo(
    () =>
      projectTimeline(
        available ? state : { ...state, streams: new Map(), facets: null },
        new Set(
          Object.keys(pending).filter(
            (id) => pending[id].data.confirmedAbsent !== false,
          ),
        ),
        timezone,
      ),
    [state, available, pending, timezone],
  );
  const past = state.streams.get("past")?.pages[0];
  const future = state.streams.get("future")?.pages[0];
  const laterCount = Math.max(
    0,
    (past?.future_count ?? 0) - (future?.total_count ?? 0),
  );
  return {
    ...timeline,
    ...projected,
    laterCount,
    supported: owner.scope.client.features.includes("timeline_read"),
  };
}

export function useNearestTimelineBuckets() {
  const { owner, state, available, timezone } = useConfiguredTimeline();
  const pending = useScopedPendingDeletions(
    owner.scope.client.scope,
    owner.scope,
  );
  const stream = state.streams.get("nearest");
  const nextTime = stream?.pages[0]?.items[0]?.timestamp_ms;
  useEffect(() => {
    if (!available) return;
    owner.ensureNearest();
    if (nextTime === null || nextTime === undefined) return;
    const timer = setTimeout(
      () => owner.ensureNearest(),
      Math.max(1, Math.min(2_147_483_647, nextTime - Date.now() + 1)),
    );
    return () => clearTimeout(timer);
  }, [owner, available, state.epoch, nextTime]);
  return useMemo(
    () =>
      buildTimelineBuckets({
        timelineSessionsTable: available
          ? Object.fromEntries(
              streamItems(stream)
                .filter(
                  (item) =>
                    !pending[item.header.id] ||
                    pending[item.header.id].data.confirmedAbsent === false,
                )
                .map((item) => [item.header.id, headerRow(item)]),
            )
          : {},
        timezone,
      }),
    [stream, available, pending, timezone],
  );
}
