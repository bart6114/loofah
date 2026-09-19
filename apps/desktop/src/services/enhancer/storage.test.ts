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
  commands: { sessionEnsureSummary: mocks.write },
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
    mocks.write.mockImplementation(async (sessionId) => {
      notes.push(note(sessionId, "summary", 0));
      return { status: "ok", data: "" };
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
  it("creates a session summary without replacing template outputs", async () => {
    notes = [note("legacy", "template_output", 1)];
    expect((await ensureSummaryDocument("s")).id).toBe("s");
    expect(notes[0].id).toBe("legacy");
    expect(mocks.write).toHaveBeenCalledExactlyOnceWith("s");
  });
  it("serializes simultaneous requests without creating duplicate summaries", async () => {
    const results = await Promise.all([
      ensureSummaryDocument("s"),
      ensureSummaryDocument("s"),
      ensureSummaryDocument("s"),
    ]);
    expect(results.map((result) => result.id)).toEqual(["s", "s", "s"]);
    expect(mocks.write).toHaveBeenCalledExactlyOnceWith("s");
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
