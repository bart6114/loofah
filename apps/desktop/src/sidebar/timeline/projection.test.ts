import { describe, expect, it } from "vitest";

import {
  TimelineController,
  tagBucketId,
  UNDATED_ID,
  UNTAGGED_ID,
} from "./controller";
import { projectTimeline } from "./projection";
import { header, page, timelineFixture } from "./test-fixture";

describe("paged timeline projection", () => {
  it("uses only server timestamps and exposes handwritten values explicitly as Undated", () => {
    const f = timelineFixture(),
      owner = new TimelineController(f.scope, "x");
    const state = owner.getSnapshot();
    state.undatedOpen = true;
    state.streams.set("past", {
      view: { kind: "date", before_ms: 100 },
      limit: 200,
      pages: [page([header("dated", 1)], { undated_count: 1 })],
      pending: false,
      error: null,
    });
    state.streams.set("undated", {
      view: { kind: "undated" },
      limit: 200,
      pages: [
        page([
          {
            ...header("raw", null),
            header: { ...header("raw", null).header, created_at: "01/02/2024" },
          },
        ]),
      ],
      pending: false,
      error: null,
    });
    const result = projectTimeline(state, new Set());
    const raw = result.buckets.find((b) => b.id === UNDATED_ID);
    expect(raw?.items[0]).toMatchObject({
      id: "raw",
      timestampMs: null,
      data: { created_at: "01/02/2024" },
    });
    expect(
      result.buckets
        .flatMap((b) => b.items)
        .filter((item) => item.id === "raw"),
    ).toHaveLength(1);
    owner.dispose();
    f.scope.dispose();
  });
  it("keeps complete subtree counts while displaying only loaded direct bucket pages with locale-sorted siblings", () => {
    const f = timelineFixture(),
      owner = new TimelineController(f.scope, "x");
    const state = owner.getSnapshot();
    state.groupBy = "tag";
    state.facets = {
      revision: "r1",
      untagged_count: 7,
      nodes: [
        {
          path: "z",
          parent_path: null,
          label: "z",
          direct_count: 600,
          subtree_count: 600,
        },
        {
          path: "a",
          parent_path: null,
          label: "a",
          direct_count: 0,
          subtree_count: 900,
        },
        {
          path: "a/child",
          parent_path: "a",
          label: "child",
          direct_count: 900,
          subtree_count: 900,
        },
        {
          path: "Untagged",
          parent_path: null,
          label: "Untagged",
          direct_count: 8,
          subtree_count: 8,
        },
      ],
    };
    state.expanded = [tagBucketId("a"), tagBucketId("a/child"), UNTAGGED_ID];
    state.streams.set(tagBucketId("a/child"), {
      view: { kind: "tag", path: "a/child" },
      limit: 200,
      pages: [page([header("loaded")], { total_count: 900 })],
      pending: false,
      error: null,
    });
    const { buckets } = projectTimeline(state, new Set());
    expect(buckets.map((b) => b.label)).toEqual([
      "a",
      "child",
      "Untagged",
      "z",
      "Untagged",
    ]);
    expect(buckets.find((b) => b.label === "a")).toMatchObject({
      totalCount: 900,
      directCount: 0,
      items: [],
    });
    expect(buckets.find((b) => b.label === "child")).toMatchObject({
      totalCount: 900,
      items: [{ id: "loaded" }],
    });
    expect(
      buckets.filter((b) => b.label === "Untagged").map((b) => b.id),
    ).toEqual([tagBucketId("Untagged"), UNTAGGED_ID]);
    owner.dispose();
    f.scope.dispose();
  });
  it("does not include another scope's tombstone or selected IDs in its source data", () => {
    const f = timelineFixture(),
      owner = new TimelineController(f.scope, "x");
    const state = owner.getSnapshot();
    state.streams.set("past", {
      view: { kind: "date", before_ms: 100 },
      limit: 200,
      pages: [page([header("own"), header("keep")])],
      pending: false,
      error: null,
    });
    expect(Object.keys(projectTimeline(state, new Set(["own"])).table)).toEqual(
      ["keep"],
    );
    expect(Object.keys(projectTimeline(state, new Set()).table)).toEqual([
      "own",
      "keep",
    ]);
    owner.dispose();
    f.scope.dispose();
  });
});
