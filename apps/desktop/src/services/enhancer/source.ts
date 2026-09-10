import { md2json } from "@hypr/editor/markdown";

import { countTranscriptWordCharacters } from "./summary-length";

import { extractPlainText } from "~/search/contexts/engine/utils";

export const EMPTY_SUMMARY_SOURCE_MESSAGE =
  "Add a note or transcript to generate a summary.";

export function summaryNoteText(content: string): string {
  if (!content.trim()) return "";
  try {
    const doc = content.trimStart().startsWith("{")
      ? JSON.parse(content)
      : md2json(content);
    return extractPlainText(JSON.stringify(doc))
      .replace(/\u00a0/g, " ")
      .trim();
  } catch {
    return content.trim();
  }
}

export function hasSummarySource(
  note: string,
  transcripts: ReadonlyArray<{ words: ReadonlyArray<{ text?: unknown }> }>,
): boolean {
  return (
    summaryNoteText(note).length > 0 ||
    countTranscriptWordCharacters(transcripts) > 0
  );
}
