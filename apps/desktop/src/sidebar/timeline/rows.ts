import {
  calculateTodayIndicatorPlacement,
  getItemTimestamp,
  type TimelineBucket,
  type TimelineItem,
  type TimelinePrecision,
} from "./utils";

export const TIMELINE_ROW_HEIGHT = {
  bucketHeader: 27,
  currentTime: 1,
  currentTimeGap: 25,
  currentTimeTopGap: 37,
  emptyToday: 52,
  session: 32,
} as const;

export type TimelineRow =
  | { key: string; kind: "page"; streamKey: string }
  | {
      key: string;
      kind: "bucket-header";
      bucket: TimelineBucket;
    }
  | {
      key: string;
      kind: "session";
      bucketId: string;
      item: TimelineItem;
      precision: TimelinePrecision;
    }
  | {
      key: string;
      kind: "current-time";
      gap: "none" | "bucket" | "top-bucket";
      suppressed: boolean;
    }
  | {
      key: string;
      kind: "empty-today";
    };

export function buildTimelineRows({
  buckets,
  currentTimeMs,
  fallbackIndicatorIndex,
  groupBy,
  hasToday,
  suppressCurrentTimeIndicator,
}: {
  buckets: TimelineBucket[];
  currentTimeMs: number;
  fallbackIndicatorIndex: number;
  groupBy: "date" | "tag";
  hasToday: boolean;
  suppressCurrentTimeIndicator: boolean;
}): TimelineRow[] {
  const rows: TimelineRow[] = [];

  buckets.forEach((bucket, bucketIndex) => {
    if (
      groupBy === "date" &&
      !hasToday &&
      fallbackIndicatorIndex === bucketIndex
    ) {
      rows.push({
        key: "current-time:fallback",
        kind: "current-time",
        gap: suppressCurrentTimeIndicator
          ? "none"
          : bucketIndex === 0
            ? "top-bucket"
            : "bucket",
        suppressed: suppressCurrentTimeIndicator,
      });
    }

    rows.push({
      key: `bucket:${bucket.id}`,
      kind: "bucket-header",
      bucket,
    });

    if (groupBy === "date" && bucket.label === "Today") {
      appendTodayRows({
        bucket,
        currentTimeMs,
        rows,
        suppressCurrentTimeIndicator,
      });
      return;
    }

    appendSessionRows(rows, bucket);
  });

  if (
    groupBy === "date" &&
    !hasToday &&
    (fallbackIndicatorIndex === -1 || fallbackIndicatorIndex === buckets.length)
  ) {
    rows.push({
      key: "current-time:fallback",
      kind: "current-time",
      gap: "none",
      suppressed: suppressCurrentTimeIndicator,
    });
  }

  return rows;
}

export function getTimelineRowHeight(row: TimelineRow): number {
  switch (row.kind) {
    case "page":
      return 36;
    case "bucket-header":
      return TIMELINE_ROW_HEIGHT.bucketHeader;
    case "session":
      return TIMELINE_ROW_HEIGHT.session;
    case "empty-today":
      return TIMELINE_ROW_HEIGHT.emptyToday;
    case "current-time":
      if (row.gap === "top-bucket") {
        return TIMELINE_ROW_HEIGHT.currentTimeTopGap;
      }
      return row.gap === "bucket"
        ? TIMELINE_ROW_HEIGHT.currentTimeGap
        : TIMELINE_ROW_HEIGHT.currentTime;
  }
}

function appendTodayRows({
  bucket,
  currentTimeMs,
  rows,
  suppressCurrentTimeIndicator,
}: {
  bucket: TimelineBucket;
  currentTimeMs: number;
  rows: TimelineRow[];
  suppressCurrentTimeIndicator: boolean;
}) {
  const entries = bucket.items.map((item) => ({
    item,
    timestamp: getItemTimestamp(item),
  }));
  const placement = calculateTodayIndicatorPlacement(
    entries,
    new Date(currentTimeMs),
  );

  if (entries.length === 0) {
    rows.push({
      key: "current-time:today",
      kind: "current-time",
      gap: "none",
      suppressed: suppressCurrentTimeIndicator,
    });
    rows.push({ key: "empty:today", kind: "empty-today" });
    return;
  }

  entries.forEach(({ item }, index) => {
    if (placement.type === "before" && placement.index === index) {
      rows.push({
        key: "current-time:today",
        kind: "current-time",
        gap: "none",
        suppressed: suppressCurrentTimeIndicator,
      });
    }
    rows.push(sessionRow(bucket, item));
  });

  if (placement.type === "after") {
    rows.push({
      key: "current-time:today",
      kind: "current-time",
      gap: "none",
      suppressed: suppressCurrentTimeIndicator,
    });
  }
}

function appendSessionRows(rows: TimelineRow[], bucket: TimelineBucket) {
  for (const item of bucket.items) {
    rows.push(sessionRow(bucket, item));
  }
}

function sessionRow(bucket: TimelineBucket, item: TimelineItem): TimelineRow {
  return {
    key: `session:${bucket.id}:${item.id}`,
    kind: "session",
    bucketId: bucket.id,
    item,
    precision: bucket.precision,
  };
}
