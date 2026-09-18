import { describe, expect, it } from "vitest";

import {
  countNormalizedCharacters,
  countTranscriptWordCharacters,
} from "./summary-length";

describe("transcript eligibility character counts", () => {
  it("counts across languages and normalizes whitespace", () => {
    expect(
      countTranscriptWordCharacters([
        { words: [{ text: "이번" }, { text: "회의는" }, { text: "짧음" }] },
      ]),
    ).toBe(9);
    expect(countNormalizedCharacters("  a\n b  ")).toBe(3);
  });
});
