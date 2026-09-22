import { describe, expect, test } from "vitest";

import { normalizeTagNames, splitTagPath, TAG_NAME_RE } from "./normalize";

describe("normalizeTagNames", () => {
  test("keeps plain tags and lowercase-dedupes", () => {
    expect(normalizeTagNames(["#Work", "work", "personal"])).toEqual([
      "work",
      "personal",
    ]);
  });

  test("accepts slash-separated tags", () => {
    expect(normalizeTagNames(["#Dataroots/Interviews"])).toEqual([
      "dataroots/interviews",
    ]);
    expect(normalizeTagNames(["a/b/c"])).toEqual(["a/b/c"]);
  });

  test("allows digit-first tags and path segments", () => {
    expect(
      normalizeTagNames(["#3E", "3e", "#2026", "#3", "projects/2024", "123/a"]),
    ).toEqual(["3e", "2026", "3", "projects/2024", "123/a"]);
  });

  test("collapses duplicate slashes and strips leading/trailing slashes", () => {
    expect(normalizeTagNames(["a//b"])).toEqual(["a/b"]);
    expect(normalizeTagNames(["/a/"])).toEqual(["a"]);
    expect(normalizeTagNames(["///"])).toEqual([]);
  });

  test("drops names outside the charset", () => {
    expect(normalizeTagNames(["", "a b", "a/b c", "#"])).toEqual([]);
  });
});

describe("TAG_NAME_RE", () => {
  test("matches slash paths but not empty or slash-edged names", () => {
    expect(TAG_NAME_RE.test("a/b")).toBe(true);
    expect(TAG_NAME_RE.test("a/")).toBe(false);
    expect(TAG_NAME_RE.test("/a")).toBe(false);
    expect(TAG_NAME_RE.test("a//b")).toBe(false);
  });
});

describe("splitTagPath", () => {
  test("splits on slashes and drops empty segments", () => {
    expect(splitTagPath("a/b/c")).toEqual(["a", "b", "c"]);
    expect(splitTagPath("a//b")).toEqual(["a", "b"]);
    expect(splitTagPath("///")).toEqual([]);
  });
});
