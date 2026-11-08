import { vi } from "vitest";

import { tagBucketId, UNTAGGED_ID } from "./controller";
import {
  buildTagTimelineBuckets,
  buildTimelineBuckets,
  deriveTimelineWindowData,
  type TimelineSessionRow,
} from "./utils";

export function viewFixture(
  table: Record<string, TimelineSessionRow>,
  timezone: string | undefined,
  groupBy: "date" | "tag",
  expanded: string[],
) {
  const window = deriveTimelineWindowData({
    timelineSessionsTable: table,
    timezone,
  });
  const allBuckets =
    groupBy === "date"
      ? buildTimelineBuckets({
          timelineSessionsTable: window.timelineSessionsTable,
          timezone,
        })
      : buildTagTimelineBuckets({ timelineSessionsTable: table });
  const id = (path: string) =>
    path === "Untagged" ? UNTAGGED_ID : tagBucketId(path);
  const expandedIds = expanded.map(id);
  const buckets =
    groupBy === "date"
      ? allBuckets
      : allBuckets
          .filter((bucket) => {
            for (
              let parent = bucket.parentId;
              parent;
              parent = allBuckets.find((value) => value.id === parent)?.parentId
            )
              if (!expanded.includes(parent)) return false;
            return true;
          })
          .map((bucket) => ({
            ...bucket,
            id: id(bucket.id),
            parentId: bucket.parentId ? id(bucket.parentId) : undefined,
            items: expanded.includes(bucket.id) ? bucket.items : [],
          }));
  return {
    owner: {
      setExpanded: vi.fn(),
      toggleUndated: vi.fn(),
      showFuture: vi.fn(),
      clearNotice: vi.fn(),
      retryFacets: vi.fn(),
      readViewport: () => undefined,
      saveViewport: vi.fn(),
      viewportUnavailable: vi.fn(),
      ensure: vi.fn(),
      ensureVisible: vi.fn(),
    },
    state: {
      epoch: 0,
      expanded: expandedIds,
      undatedOpen: false,
      allFuture: false,
      revision: "r1",
      notice: null,
      facetsError: null,
      facetsPending: false,
      streams: new Map(),
      facets: {
        revision: "r1",
        untagged_count: 0,
        nodes: allBuckets
          .filter((bucket) => bucket.id !== "Untagged")
          .map((bucket) => ({
            path: bucket.id,
            parent_path: bucket.parentId ?? null,
            label: bucket.label,
            direct_count: bucket.items.length,
            subtree_count: bucket.totalCount ?? bucket.items.length,
          })),
      },
    },
    available: true,
    supported: true,
    connection: { state: "connected" },
    buckets,
    table,
    laterCount: window.hasMoreFutureItems ? 1 : 0,
  };
}
