import {
  streamItems,
  tagBucketId,
  UNDATED_ID,
  UNTAGGED_ID,
  type TimelineController,
} from "./controller";
import {
  buildTimelineBuckets,
  makeTimelineItem,
  type TimelineBucket,
  type TimelineSessionRow,
} from "./utils";

import type { TimelineHeader } from "~/shared/application/protocol.gen";

export function headerRow(item: TimelineHeader): TimelineSessionRow {
  return {
    title: item.header.title,
    created_at: item.header.created_at,
    folder_id: item.header.folder ?? "",
    tags: item.header.tags,
    author: item.header.author,
    timestamp_ms: item.timestamp_ms,
  };
}

export function projectTimeline(
  state: ReturnType<TimelineController["getSnapshot"]>,
  pendingIds: ReadonlySet<string>,
  timezone?: string,
) {
  const items = (key: string) =>
    streamItems(state.streams.get(key)).filter(
      (item) => !pendingIds.has(item.header.id),
    );
  const table = Object.fromEntries(
    [...state.streams.keys()].flatMap((key) =>
      items(key).map((item) => [item.header.id, headerRow(item)]),
    ),
  );
  if (state.groupBy === "date") {
    const dated = Object.fromEntries(
      [...items("future"), ...items("past")].map((item) => [
        item.header.id,
        headerRow(item),
      ]),
    );
    const buckets = buildTimelineBuckets({
      timelineSessionsTable: dated,
      timezone,
    });
    const undatedCount =
      state.streams.get("past")?.pages[0]?.undated_count ??
      state.streams.get("future")?.pages[0]?.undated_count ??
      0;
    if (undatedCount)
      buckets.push({
        id: UNDATED_ID,
        label: "Undated",
        kind: "tag",
        precision: "date",
        totalCount: undatedCount,
        directCount: undatedCount,
        streamKey: "undated",
        view: { kind: "undated" },
        items: state.undatedOpen
          ? items("undated").map((item) =>
              makeTimelineItem(item.header.id, headerRow(item)),
            )
          : [],
      });
    return { buckets, table };
  }
  const byParent = new Map<
    string | null,
    NonNullable<typeof state.facets>["nodes"]
  >();
  for (const facet of state.facets?.nodes ?? []) {
    const siblings = byParent.get(facet.parent_path) ?? [];
    siblings.push(facet);
    byParent.set(facet.parent_path, siblings);
  }
  for (const siblings of byParent.values())
    siblings.sort((a, b) => a.label.localeCompare(b.label));
  const buckets: TimelineBucket[] = [];
  const append = (parent: string | null, depth: number) => {
    for (const facet of byParent.get(parent) ?? []) {
      const id = tagBucketId(facet.path);
      buckets.push({
        id,
        label: facet.label,
        kind: "tag",
        precision: "date",
        depth,
        parentId: parent === null ? undefined : tagBucketId(parent),
        totalCount: facet.subtree_count,
        directCount: facet.direct_count,
        streamKey: id,
        view: { kind: "tag", path: facet.path },
        items: state.expanded.includes(id)
          ? items(id).map((item) =>
              makeTimelineItem(item.header.id, headerRow(item)),
            )
          : [],
      });
      if (state.expanded.includes(id)) append(facet.path, depth + 1);
    }
  };
  append(null, 0);
  if (state.facets)
    buckets.push({
      id: UNTAGGED_ID,
      label: "Untagged",
      kind: "tag",
      precision: "date",
      totalCount: state.facets.untagged_count,
      directCount: state.facets.untagged_count,
      streamKey: UNTAGGED_ID,
      view: { kind: "untagged" },
      items: state.expanded.includes(UNTAGGED_ID)
        ? items(UNTAGGED_ID).map((item) =>
            makeTimelineItem(item.header.id, headerRow(item)),
          )
        : [],
    });
  return { buckets, table };
}
