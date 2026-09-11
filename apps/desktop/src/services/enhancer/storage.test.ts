import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  ensureSummaryDocument,
  selectSummaryDocument,
  type EnhancerNote,
} from "./storage";

const mocks = vi.hoisted(() => ({ load: vi.fn(), write: vi.fn() }));
vi.mock("~/session/content-queries", () => ({
  loadSessionContentSnapshot: mocks.load,
}));
vi.mock("~/shared/utils", () => ({ id: () => "new-note" }));
vi.mock("~/types/tauri.gen", () => ({
  commands: { sessionWriteEnhancedDoc: mocks.write },
}));

function note(id: string, kind: string, position: number): EnhancerNote {
  return {
    id,
    kind,
    position,
    title: id,
    templateId: kind === "summary" ? "" : "old",
    markdown: "Saved",
    content: "Saved",
    contentFormat: "md",
  };
}

describe("summary storage", () => {
  let notes: EnhancerNote[];
  beforeEach(() => {
    vi.clearAllMocks();
    notes = [];
    mocks.load.mockImplementation(async () => ({ enhancedNotes: notes }));
    mocks.write.mockImplementation(async (doc) => {
      notes.push(note(doc.id, doc.kind, doc.sort_order));
      return { status: "ok", data: null };
    });
  });
  it("prefers the first ordinary summary by position then ID", () => {
    expect(
      selectSummaryDocument([
        note("legacy", "template_output", 0),
        note("b", "summary", 2),
        note("c", "summary", 1),
        note("a", "summary", 1),
      ])?.id,
    ).toBe("a");
  });
  it("falls back to the first legacy document and preserves it", async () => {
    notes = [note("z", "template_output", 1), note("a", "template_output", 1)];
    const before = structuredClone(notes);
    expect((await ensureSummaryDocument("s")).id).toBe("a");
    expect(notes).toEqual(before);
    expect(mocks.write).not.toHaveBeenCalled();
  });
  it("serializes simultaneous requests without creating duplicate summaries", async () => {
    const results = await Promise.all([
      ensureSummaryDocument("s"),
      ensureSummaryDocument("s"),
      ensureSummaryDocument("s"),
    ]);
    expect(results.map((result) => result.id)).toEqual([
      "new-note",
      "new-note",
      "new-note",
    ]);
    expect(mocks.write).toHaveBeenCalledExactlyOnceWith({
      id: "new-note",
      session_id: "s",
      kind: "summary",
      title: "Summary",
      template_id: "",
      sort_order: 1,
      markdown: "",
    });
  });
  it("reports missing sessions and failed writes", async () => {
    mocks.load.mockResolvedValueOnce(null);
    await expect(ensureSummaryDocument("s")).rejects.toThrow(
      "no longer exists",
    );
    mocks.write.mockResolvedValueOnce({ status: "error", error: "disk full" });
    await expect(ensureSummaryDocument("s")).rejects.toThrow("disk full");
  });
});
