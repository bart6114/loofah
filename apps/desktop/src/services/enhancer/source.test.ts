import { describe, expect, it } from "vitest";

import { hasSummarySource, summaryNoteText } from "./source";
import { getSummaryLengthPolicy } from "./summary-length";

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
  it("uses plain note length when transcript text is absent", () => {
    expect(getSummaryLengthPolicy([], summaryNoteText("**Go**"))).toMatchObject(
      { sourceCharacters: 2, maxCharacters: 320, maxSections: 2 },
    );
  });
  it("keeps transcript length when both sources are present", () => {
    const transcripts = [
      {
        segments: [{ speaker: "A", text: "Go" }],
        startedAt: null,
        endedAt: null,
      },
    ];
    expect(
      getSummaryLengthPolicy(transcripts, "Long note".repeat(100)),
    ).toEqual(getSummaryLengthPolicy(transcripts));
  });
});
