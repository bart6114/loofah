import {
  differenceInCalendarDays,
  differenceInCalendarMonths,
  safeParseDate,
  startOfDay,
  startOfMonth,
  TZDate,
} from "@hypr/utils";

import { splitTagPath } from "~/tags/normalize";

function toTZ(date: Date, timezone?: string): Date {
  return timezone ? new TZDate(date, timezone) : date;
}

export type TimelineSessionRow = {
  title?: string | null;
  created_at?: string | null;
  folder_id?: string | null;
  tags?: string[] | null;
  author?: string | null;
  timestamp_ms?: number | null;
};

export type TimelineSessionsTable =
  | Record<string, TimelineSessionRow>
  | null
  | undefined;

export type TimelineItem = {
  type: "session";
  id: string;
  data: TimelineSessionRow;
  /** Scoped reads supply the canonical timestamp; legacy projections parse it once. */
  timestampMs: number | null;
};

export function makeTimelineItem(
  sessionId: string,
  row: TimelineSessionRow,
): TimelineItem {
  return {
    type: "session",
    id: sessionId,
    data: row,
    timestampMs:
      row.timestamp_ms !== undefined
        ? row.timestamp_ms
        : (safeParseDate(row.created_at)?.getTime() ?? null),
  };
}

export type TimelinePrecision = "time" | "date";

export type TimelineBucket = {
  /** Stable runtime identity; scoped tag and synthetic bucket IDs are distinct. */
  id: string;
  /** Display text: last path segment for tag buckets. */
  label: string;
  precision: TimelinePrecision;
  items: TimelineItem[];
  // Absent means "date".
  kind?: "date" | "tag";
  // Tag-mode tree metadata; absent on date buckets and 0/absent for roots.
  depth?: number;
  parentId?: string;
  /** Own + descendant sessions, deduped by session id. */
  totalCount?: number;
  streamKey?: string;
  view?: import("~/shared/application/protocol.gen").TimelineView;
  directCount?: number;
};

export type TimelineWindowData = {
  timelineSessionsTable: TimelineSessionsTable;
  hasMoreFutureItems: boolean;
};

export type TimelineIndicatorPlacement =
  | { type: "before"; index: number }
  | { type: "after" };

export function getBucketInfo(
  date: Date,
  timezone?: string,
): {
  label: string;
  sortKey: number;
  precision: TimelinePrecision;
} {
  const now = new Date();
  const tzDate = toTZ(date, timezone);
  const tzNow = toTZ(now, timezone);
  const daysDiff = differenceInCalendarDays(tzDate, tzNow);
  const sortKey = startOfDay(tzDate).getTime();
  const absDays = Math.abs(daysDiff);

  if (daysDiff === 0) {
    return { label: "Today", sortKey, precision: "time" };
  }

  if (daysDiff === -1) {
    return { label: "Yesterday", sortKey, precision: "time" };
  }

  if (daysDiff === 1) {
    return { label: "Tomorrow", sortKey, precision: "time" };
  }

  if (daysDiff < 0) {
    if (absDays <= 6) {
      return { label: `${absDays} days ago`, sortKey, precision: "time" };
    }

    if (absDays <= 27) {
      const weeks = Math.max(1, Math.round(absDays / 7));
      const weekRangeEndDay = Math.max(7, weeks * 7 - 3);
      const weekRangeEnd = new Date(
        now.getTime() - weekRangeEndDay * 24 * 60 * 60 * 1000,
      );
      const weekSortKey = startOfDay(toTZ(weekRangeEnd, timezone)).getTime();

      return {
        label: weeks === 1 ? "a week ago" : `${weeks} weeks ago`,
        sortKey: weekSortKey,
        precision: "date",
      };
    }

    let months = Math.abs(differenceInCalendarMonths(tzDate, tzNow));
    if (months === 0) {
      months = 1;
    }
    const monthStartKey = startOfMonth(tzDate).getTime();
    const lastDayInMonthBucket = new Date(
      now.getTime() - 28 * 24 * 60 * 60 * 1000,
    );
    const lastDayKey = startOfDay(
      toTZ(lastDayInMonthBucket, timezone),
    ).getTime();
    const monthSortKey = Math.min(monthStartKey, lastDayKey);
    return {
      label: months === 1 ? "a month ago" : `${months} months ago`,
      sortKey: monthSortKey,
      precision: "date",
    };
  }

  if (absDays <= 6) {
    return { label: `in ${absDays} days`, sortKey, precision: "time" };
  }

  if (absDays <= 27) {
    const weeks = Math.max(1, Math.round(absDays / 7));
    const weekRangeStartDay = Math.max(7, weeks * 7 - 3);
    const weekRangeStart = new Date(
      now.getTime() + weekRangeStartDay * 24 * 60 * 60 * 1000,
    );
    const weekSortKey = startOfDay(toTZ(weekRangeStart, timezone)).getTime();

    return {
      label: weeks === 1 ? "next week" : `in ${weeks} weeks`,
      sortKey: weekSortKey,
      precision: "date",
    };
  }

  let months = differenceInCalendarMonths(tzDate, tzNow);
  if (months === 0) {
    months = 1;
  }
  const monthStartKey = startOfMonth(tzDate).getTime();
  const firstDayInMonthBucket = new Date(
    now.getTime() + 28 * 24 * 60 * 60 * 1000,
  );
  const firstDayKey = startOfDay(
    toTZ(firstDayInMonthBucket, timezone),
  ).getTime();
  const monthSortKey = Math.max(monthStartKey, firstDayKey);
  return {
    label: months === 1 ? "next month" : `in ${months} months`,
    sortKey: monthSortKey,
    precision: "date",
  };
}

export function calculateIndicatorIndex(
  entries: Array<{ timestamp: Date | null }>,
  current: Date,
): number {
  const index = entries.findIndex(({ timestamp }) => {
    if (!timestamp) {
      return true;
    }

    return timestamp.getTime() < current.getTime();
  });

  if (index === -1) {
    return entries.length;
  }

  return index;
}

export function calculateTodayIndicatorPlacement(
  entries: Array<{ item: TimelineItem; timestamp: Date | null }>,
  current: Date,
): TimelineIndicatorPlacement {
  const indicatorIndex = calculateIndicatorIndex(entries, current);
  if (indicatorIndex === entries.length) {
    return { type: "after" };
  }

  return { type: "before", index: indicatorIndex };
}

export function getItemTimestamp(item: TimelineItem): Date | null {
  return item.timestampMs === null ? null : new Date(item.timestampMs);
}

export function isTimelineItemInFuture(item: TimelineItem): boolean {
  return (item.timestampMs ?? 0) > Date.now();
}

export function hasFutureTimelineItems(
  buckets: TimelineBucket[],
  nowMs: number,
): boolean {
  return buckets.some((bucket) =>
    bucket.items.some((item) => (item.timestampMs ?? 0) > nowMs),
  );
}

function getTomorrowUpperBound(timezone?: string): number {
  const dayAfterTomorrow = new Date(Date.now() + 2 * 24 * 60 * 60 * 1000);
  return startOfDay(toTZ(dayAfterTomorrow, timezone)).getTime();
}

function isAtOrBeforeTomorrow(date: Date | null, timezone?: string): boolean {
  if (!date) {
    return true;
  }

  return date.getTime() < getTomorrowUpperBound(timezone);
}

function isAfterTomorrow(date: Date | null, timezone?: string): boolean {
  if (!date) {
    return false;
  }

  return date.getTime() >= getTomorrowUpperBound(timezone);
}

export function filterTimelineTablesUpToTomorrow({
  timelineSessionsTable,
  timezone,
}: {
  timelineSessionsTable: TimelineSessionsTable;
  timezone?: string;
}): {
  timelineSessionsTable: TimelineSessionsTable;
} {
  return {
    timelineSessionsTable: timelineSessionsTable
      ? Object.fromEntries(
          Object.entries(timelineSessionsTable).filter(([, row]) =>
            isAtOrBeforeTomorrow(safeParseDate(row.created_at), timezone),
          ),
        )
      : timelineSessionsTable,
  };
}

export function deriveTimelineWindowData({
  timelineSessionsTable,
  timezone,
}: {
  timelineSessionsTable: TimelineSessionsTable;
  timezone?: string;
}): TimelineWindowData {
  const filteredSessionsTable = timelineSessionsTable
    ? ({} as Record<string, TimelineSessionRow>)
    : timelineSessionsTable;
  let hasMoreFutureItems = false;

  if (timelineSessionsTable && filteredSessionsTable) {
    for (const [sessionId, row] of Object.entries(timelineSessionsTable)) {
      const date = safeParseDate(row.created_at);

      if (isAfterTomorrow(date, timezone)) {
        hasMoreFutureItems = true;
        continue;
      }

      if (isAtOrBeforeTomorrow(date, timezone)) {
        filteredSessionsTable[sessionId] = row;
      }
    }
  }

  return {
    timelineSessionsTable: filteredSessionsTable,
    hasMoreFutureItems,
  };
}

export function hasTimelineItemsAfterTomorrow({
  timelineSessionsTable,
  timezone,
}: {
  timelineSessionsTable: TimelineSessionsTable;
  timezone?: string;
}): boolean {
  return Boolean(
    timelineSessionsTable &&
    Object.values(timelineSessionsTable).some((row) =>
      isAfterTomorrow(safeParseDate(row.created_at), timezone),
    ),
  );
}

function compareItemsNewestFirst(a: TimelineItem, b: TimelineItem): number {
  const timeAValue = a.timestampMs ?? 0;
  const timeBValue = b.timestampMs ?? 0;
  if (timeBValue == timeAValue) {
    return (a.data.title ?? "Untitled") > (b.data.title ?? "Untitled")
      ? 1
      : (a.data.title ?? "Untitled") < (b.data.title ?? "Untitled")
        ? -1
        : 0;
  }
  return timeBValue - timeAValue;
}

export const UNTAGGED_BUCKET_LABEL = "Untagged";

type TagTreeNode = {
  path: string;
  segment: string;
  ownItems: TimelineItem[];
  children: Map<string, TagTreeNode>;
};

/// Tag-mode grouping: slash-separated tags form a tree (`dataroots/interviews`
/// nests under `dataroots`), flattened preorder with `depth`/`parentId` so the
/// sidebar can render it as an indented flat list. A session sits under every
/// tag it carries; `totalCount` dedupes it across a subtree. `Untagged` last,
/// outside the tree. No date windowing -- callers skip
/// `deriveTimelineWindowData` in tag mode.
export function buildTagTimelineBuckets({
  timelineSessionsTable,
}: {
  timelineSessionsTable: TimelineSessionsTable;
}): TimelineBucket[] {
  const untagged: TimelineItem[] = [];
  const roots = new Map<string, TagTreeNode>();

  for (const [sessionId, row] of Object.entries(timelineSessionsTable ?? {})) {
    const item = makeTimelineItem(sessionId, row);
    // Hand-edited `_meta.json` bypasses `normalizeTagNames`, so tolerate empty
    // segments; a tag that is all slashes is treated as no tag.
    const tagPaths = new Map(
      (row.tags ?? [])
        .map((tag) => splitTagPath(tag))
        .filter((segments) => segments.length > 0)
        .map((segments) => [segments.join("/"), segments]),
    );
    if (tagPaths.size === 0) {
      untagged.push(item);
      continue;
    }
    for (const segments of tagPaths.values()) {
      let siblings = roots;
      let node: TagTreeNode | undefined;
      let path = "";
      for (const segment of segments) {
        path = path ? `${path}/${segment}` : segment;
        node = siblings.get(segment);
        if (!node) {
          node = { path, segment, ownItems: [], children: new Map() };
          siblings.set(segment, node);
        }
        siblings = node.children;
      }
      node!.ownItems.push(item);
    }
  }

  const buckets: TimelineBucket[] = [];
  const flatten = (
    node: TagTreeNode,
    depth: number,
    parentId: string | undefined,
  ): Set<string> => {
    const bucket: TimelineBucket = {
      id: node.path,
      label: node.segment,
      precision: "date",
      items: node.ownItems.sort(compareItemsNewestFirst),
      kind: "tag",
      depth,
      parentId,
      totalCount: 0,
    };
    buckets.push(bucket);
    const subtreeIds = new Set(node.ownItems.map((item) => item.id));
    for (const child of sortedChildren(node.children)) {
      for (const id of flatten(child, depth + 1, node.path)) {
        subtreeIds.add(id);
      }
    }
    bucket.totalCount = subtreeIds.size;
    return subtreeIds;
  };
  for (const root of sortedChildren(roots)) {
    flatten(root, 0, undefined);
  }

  if (untagged.length > 0) {
    buckets.push({
      id: UNTAGGED_BUCKET_LABEL,
      label: UNTAGGED_BUCKET_LABEL,
      precision: "date",
      items: untagged.sort(compareItemsNewestFirst),
      kind: "tag",
      depth: 0,
      totalCount: untagged.length,
    });
  }

  return buckets;
}

function sortedChildren(children: Map<string, TagTreeNode>): TagTreeNode[] {
  return [...children.values()].sort((a, b) =>
    a.segment.localeCompare(b.segment),
  );
}

export function buildTimelineBuckets({
  timelineSessionsTable,
  timezone,
}: {
  timelineSessionsTable: TimelineSessionsTable;
  timezone?: string;
}): TimelineBucket[] {
  const items: TimelineItem[] = [];

  if (timelineSessionsTable) {
    Object.entries(timelineSessionsTable).forEach(([sessionId, row]) => {
      const item = makeTimelineItem(sessionId, row);
      if (item.timestampMs === null) {
        return;
      }
      items.push(item);
    });
  }

  items.sort(compareItemsNewestFirst);

  const bucketMap = new Map<
    string,
    { sortKey: number; precision: TimelinePrecision; items: TimelineItem[] }
  >();

  items.forEach((item) => {
    const bucket = getBucketInfo(
      getItemTimestamp(item) ?? new Date(0),
      timezone,
    );

    if (!bucketMap.has(bucket.label)) {
      bucketMap.set(bucket.label, {
        sortKey: bucket.sortKey,
        precision: bucket.precision,
        items: [],
      });
    }
    bucketMap.get(bucket.label)!.items.push(item);
  });

  return Array.from(bucketMap.entries())
    .sort((a, b) => b[1].sortKey - a[1].sortKey)
    .map(
      ([label, value]) =>
        ({
          id: label,
          label,
          items: value.items,
          precision: value.precision,
        }) satisfies TimelineBucket,
    );
}
