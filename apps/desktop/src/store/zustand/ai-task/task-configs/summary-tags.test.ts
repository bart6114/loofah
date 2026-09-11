import { describe, expect, it } from "vitest";

import {
  appendTagLineToMarkdown,
  extractEnhanceTagNames,
} from "./summary-tags";

function createEnhanceArgs(
  overrides: Partial<Parameters<typeof extractEnhanceTagNames>[1]> = {},
): Parameters<typeof extractEnhanceTagNames>[1] {
  return {
    language: "en",
    promptOverride: "",
    session: {
      title: "Weekly Review",
      startedAt: null,
      endedAt: null,
      event: null,
    },
    participants: [],
    preMeetingMemo: "",
    postMeetingMemo: "",
    transcripts: [],
    imageContext: [],
    ...overrides,
  };
}

describe("summary tags", () => {
  it("extracts unique hashtags from summary, memos, and the shared prompt", () => {
    const tags = extractEnhanceTagNames(
      "# Summary\n\nDiscussed #Launch and issue #123.",
      createEnhanceArgs({
        preMeetingMemo: "Prep #prep #launch",
        postMeetingMemo: "Next #follow-up",
        promptOverride: "Use #customer and #owners",
      }),
    );

    expect(tags).toEqual(["launch", "prep", "follow-up", "customer", "owners"]);
  });

  it("appends tags at the bottom without duplicating existing trailing tags", () => {
    expect(
      appendTagLineToMarkdown("Body\n\n#old #tags", ["old", "tags", "new"]),
    ).toBe("Body\n\n#old #tags #new");
  });

  it("extracts slash tags but not URL fragments", () => {
    const tags = extractEnhanceTagNames(
      "Talked #Dataroots/Interviews and #projects/2024, trailing #foo/ and https://x.com/#skip.",
      createEnhanceArgs(),
    );

    expect(tags).toEqual(["dataroots/interviews", "projects/2024", "foo"]);
  });

  it("round-trips a trailing tag line containing slash tags", () => {
    expect(
      appendTagLineToMarkdown("Body\n\n#dataroots/interviews", [
        "dataroots/interviews",
        "hiring",
      ]),
    ).toBe("Body\n\n#dataroots/interviews #hiring");
  });
});
