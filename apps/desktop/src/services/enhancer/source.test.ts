import { describe, expect, it } from "vitest";

import { hasSummarySource, summaryNoteText } from "./source";

describe("manual summary sources", () => {
  it.each([
    "",
    "  \n",
    "&nbsp;",
    "---",
    "# ",
    "![photo](attachments/photo.png)",
    '{"type":"doc","content":[{"type":"paragraph"}]}',
  ])("rejects empty source %s", (note) => {
    expect(hasSummarySource(note, [{ words: [{ text: " " }] }])).toBe(false);
  });
  it.each([
    "Go",
    "# Decision\n\nShip it.",
    '{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Go"}]}]}',
  ])("accepts note-only text %s", (note) => {
    expect(hasSummarySource(note, [])).toBe(true);
  });
  it("accepts a short transcript without a note", () => {
    expect(hasSummarySource("", [{ words: [{ text: "Go" }] }])).toBe(true);
  });
  it("extracts plain note text", () => {
    expect(summaryNoteText("**Go**")).toBe("Go");
  });
});
