import type { TextStreamPart, ToolSet } from "ai";

const FOOTER_START = "<loofah-tags";
const FOOTER_OPEN = `${FOOTER_START}>`;
const FOOTER_CLOSE = "</loofah-tags>";
export const MAX_TAG_FOOTER_LENGTH = 4096;

export async function* extractSummaryTagFooter<TOOLS extends ToolSet>(
  stream: AsyncIterable<TextStreamPart<TOOLS>>,
  {
    signal,
    onResult,
  }: {
    signal: AbortSignal;
    onResult: (tags: string[] | undefined) => void;
  },
): AsyncIterable<TextStreamPart<TOOLS>> {
  let pending = "";
  let footer: string | undefined;
  let oversized = false;
  let truncated = false;
  let lastTextChunk:
    | Extract<TextStreamPart<TOOLS>, { type: "text-delta" }>
    | undefined;

  for await (const chunk of stream) {
    if (signal.aborted) return;
    if (
      (chunk.type === "finish" || chunk.type === "finish-step") &&
      chunk.finishReason === "length"
    )
      truncated = true;
    if (chunk.type !== "text-delta") {
      yield chunk;
      continue;
    }
    lastTextChunk = chunk;
    if (footer !== undefined) {
      if (!oversized) {
        if (footer.length + chunk.text.length > MAX_TAG_FOOTER_LENGTH) {
          oversized = true;
          footer = "";
        } else footer += chunk.text;
      }
      continue;
    }

    pending += chunk.text;
    const start = pending.indexOf(FOOTER_START);
    if (start !== -1) {
      const text = pending.slice(0, start);
      oversized = pending.length - start > MAX_TAG_FOOTER_LENGTH;
      footer = oversized ? "" : pending.slice(start);
      pending = "";
      if (text) yield { ...chunk, text };
      continue;
    }

    let held = Math.min(pending.length, FOOTER_START.length - 1);
    while (held > 0 && !FOOTER_START.startsWith(pending.slice(-held))) held--;
    const text = pending.slice(0, pending.length - held);
    pending = held ? pending.slice(-held) : "";
    if (text) yield { ...chunk, text };
  }

  if (signal.aborted) return;
  // A partial marker at EOF belongs to truncated metadata, not the summary.
  if (pending === "<" && lastTextChunk) {
    yield { ...lastTextChunk, text: pending };
  }
  const tags = oversized || truncated ? undefined : parseTagFooter(footer);
  if (tags === undefined) {
    console.warn(
      "Summary tag suggestions unavailable: missing or invalid footer.",
    );
  }
  onResult(tags);
}

function parseTagFooter(footer: string | undefined): string[] | undefined {
  if (!footer?.startsWith(FOOTER_OPEN)) return undefined;
  const end = footer.indexOf(FOOTER_CLOSE, FOOTER_OPEN.length);
  if (end === -1 || footer.slice(end + FOOTER_CLOSE.length).trim())
    return undefined;
  try {
    const value: unknown = JSON.parse(footer.slice(FOOTER_OPEN.length, end));
    if (!value || typeof value !== "object" || Array.isArray(value))
      return undefined;
    const tags = (value as { tags?: unknown }).tags;
    if (
      !Array.isArray(tags) ||
      tags.length > 3 ||
      tags.some(
        (tag) => typeof tag !== "string" || !tag.trim() || tag.length > 120,
      )
    ) {
      return undefined;
    }
    return tags;
  } catch {
    return undefined;
  }
}
