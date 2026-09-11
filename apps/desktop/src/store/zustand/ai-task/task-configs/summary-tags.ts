import type { TaskArgsMapTransformed } from ".";

import { normalizeTagNames, TAG_NAME_RE } from "~/tags/normalize";

type EnhanceArgs = TaskArgsMapTransformed["enhance"];

// Mirrors `TAG_NAME_RE`'s segment shape so extraction never emits a name that
// `normalizeTagNames` then drops; the preceding-char class keeps excluding `/`
// so URL fragments (`…/#foo`) stay unmatched.
const HASHTAG_RE =
  /(^|[^\p{L}\p{N}_/#])#([\p{L}_][\p{L}\p{N}_-]*(?:\/[\p{L}\p{N}_][\p{L}\p{N}_-]*)*)/gu;

export function extractEnhanceTagNames(
  summaryMarkdown: string,
  transformedArgs: EnhanceArgs,
): string[] {
  const sources = [
    summaryMarkdown,
    transformedArgs.preMeetingMemo,
    transformedArgs.postMeetingMemo,
    transformedArgs.promptOverride,
  ];

  return extractHashtagNames(sources);
}

export function appendTagLineToMarkdown(
  markdown: string,
  tagNames: string[],
): string {
  const normalizedTagNames = normalizeTagNames(tagNames);
  if (normalizedTagNames.length === 0) {
    return markdown;
  }

  const body = stripTrailingTagLines(markdown).trimEnd();
  const tagLine = normalizedTagNames.map((tagName) => `#${tagName}`).join(" ");

  return body ? `${body}\n\n${tagLine}` : tagLine;
}

function extractHashtagNames(sources: Array<string | null | undefined>) {
  const tagNames: string[] = [];

  for (const source of sources) {
    if (!source) {
      continue;
    }

    for (const match of source.matchAll(HASHTAG_RE)) {
      const tagName = match[2];
      if (tagName) {
        tagNames.push(tagName);
      }
    }
  }

  return normalizeTagNames(tagNames);
}

function stripTrailingTagLines(markdown: string) {
  const lines = markdown.split(/\r?\n/);
  let end = lines.length;

  while (end > 0 && lines[end - 1]?.trim() === "") {
    end -= 1;
  }

  while (end > 0 && isTagOnlyLine(lines[end - 1] ?? "")) {
    end -= 1;
    while (end > 0 && lines[end - 1]?.trim() === "") {
      end -= 1;
    }
  }

  return lines.slice(0, end).join("\n");
}

function isTagOnlyLine(line: string) {
  const tokens = line.trim().split(/\s+/);
  return (
    tokens.length > 0 &&
    tokens.every(
      (token) => token.startsWith("#") && TAG_NAME_RE.test(token.slice(1)),
    )
  );
}
