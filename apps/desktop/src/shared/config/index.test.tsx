import { describe, expect, test } from "vitest";

import { resolveConfigValue } from ".";

describe("resolveConfigValue", () => {
  test("parses stored array values without exposing malformed entries", () => {
    expect(
      resolveConfigValue("spoken_languages", {
        values: { spoken_languages: '["en",2,"ko"]' },
        hasValues: new Set(["spoken_languages"]),
      }),
    ).toEqual(["en", "ko"]);
  });

  test("returns the same array reference for the same stored string", () => {
    const first = resolveConfigValue("sidebar_expanded_tags", {
      values: { sidebar_expanded_tags: '["work","personal"]' },
      hasValues: new Set(["sidebar_expanded_tags"]),
    });
    const second = resolveConfigValue("sidebar_expanded_tags", {
      values: { sidebar_expanded_tags: '["work","personal"]' },
      hasValues: new Set(["sidebar_expanded_tags"]),
    });

    expect(first).toEqual(["work", "personal"]);
    expect(second).toBe(first);
  });
});
