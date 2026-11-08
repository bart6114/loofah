import { expect, it } from "vitest";

import { createTimelineSelectionStore } from "./timeline-selection";

it("sets an anchor without selecting, then includes it when toggling another note", () => {
  const store = createTimelineSelectionStore();
  expect(store.getState()).toMatchObject({ selectedIds: [], anchorId: null });
  store.getState().setAnchor("session-first");
  expect(store.getState()).toMatchObject({
    selectedIds: [],
    anchorId: "session-first",
  });
  store.getState().toggleSelect("session-second");
  expect(store.getState().selectedIds).toEqual([
    "session-first",
    "session-second",
  ]);
  store.getState().toggleSelect("session-second");
  expect(store.getState()).toMatchObject({
    selectedIds: ["session-first"],
    anchorId: "session-first",
  });
  store.getState().toggleSelect("session-first");
  expect(store.getState()).toMatchObject({ selectedIds: [], anchorId: null });
});

it("selects a first toggled note as its own anchor", () => {
  const store = createTimelineSelectionStore();
  store.getState().toggleSelect("session-same");
  expect(store.getState()).toMatchObject({
    selectedIds: ["session-same"],
    anchorId: "session-same",
  });
  expect(store.getState().isSelected("session-same")).toBe(true);
  expect(store.getState().isSelected("session-other")).toBe(false);
});

it.each(["session-first", "session-last"])(
  "preserves range order and its %s anchor in either direction",
  (anchor) => {
    const store = createTimelineSelectionStore();
    const keys = ["session-first", "session-middle", "session-last"];
    store.getState().setAnchor(anchor);
    store.getState().selectRange(keys, anchor === keys[0] ? keys[2] : keys[0]);
    expect(store.getState()).toMatchObject({
      selectedIds: keys,
      anchorId: anchor,
    });
  },
);

it("resets a missing or collapsed range anchor to the target", () => {
  const store = createTimelineSelectionStore();
  store.getState().selectRange(["session-visible"], "session-visible");
  expect(store.getState()).toMatchObject({
    selectedIds: ["session-visible"],
    anchorId: "session-visible",
  });
  store.getState().setAnchor("session-collapsed");
  store.getState().selectRange(["session-visible"], "session-visible");
  expect(store.getState()).toMatchObject({
    selectedIds: ["session-visible"],
    anchorId: "session-visible",
  });
  store.getState().selectRange(["session-visible"], "session-unavailable");
  expect(store.getState()).toMatchObject({
    selectedIds: ["session-unavailable"],
    anchorId: "session-unavailable",
  });
});

it("preserves a visible anchor for select-all, then falls back or clears it", () => {
  const store = createTimelineSelectionStore();
  store.getState().setAnchor("session-second");
  store.getState().selectAll(["session-first", "session-second"]);
  expect(store.getState().anchorId).toBe("session-second");
  store.getState().selectAll(["session-third"]);
  expect(store.getState()).toMatchObject({
    selectedIds: ["session-third"],
    anchorId: "session-third",
  });
  store.getState().clear();
  expect(store.getState()).toMatchObject({ selectedIds: [], anchorId: null });
  store.getState().selectAll([]);
  expect(store.getState()).toMatchObject({ selectedIds: [], anchorId: null });
});

it("isolates identical note IDs and clears retired stores without reviving captured actions", () => {
  const first = createTimelineSelectionStore(),
    second = createTimelineSelectionStore();
  const old = first.getState();
  old.toggleSelect("session-same");
  expect(second.getState().selectedIds).toEqual([]);
  second.getState().toggleSelect("session-same");
  first.dispose();
  old.setAnchor("session-late");
  old.toggleSelect("session-same");
  old.selectAll(["session-same"]);
  old.selectRange(["session-same"], "session-same");
  old.clear();
  expect(first.getState()).toMatchObject({ selectedIds: [], anchorId: null });
  expect(old.isSelected("session-same")).toBe(false);
  expect(second.getState()).toMatchObject({
    selectedIds: ["session-same"],
    anchorId: "session-same",
  });
});

it("reconciles only loaded/visible rows without auto-selecting appended pages or retaining a hidden range anchor", () => {
  const store = createTimelineSelectionStore();
  store.getState().selectAll(["session-a", "session-b"]);
  store.getState().reconcile(["session-a", "session-b", "session-c"]);
  expect(store.getState().selectedIds).toEqual(["session-a", "session-b"]);
  store.getState().reconcile(["session-b", "session-c"]);
  expect(store.getState().selectedIds).toEqual(["session-b"]);
  expect(store.getState().anchorId).toBeNull();
  store.getState().selectRange(["session-b", "session-c"], "session-c");
  expect(store.getState().selectedIds).toEqual(["session-c"]);
  store.dispose();
});
