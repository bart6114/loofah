import type { TextStreamPart } from "ai";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  extractSummaryTagFooter,
  MAX_TAG_FOOTER_LENGTH,
} from "./summary-tag-footer";

async function collect(
  chunks: string[],
  signal = new AbortController().signal,
) {
  const onResult = vi.fn();
  const output: string[] = [];
  const stream = (async function* () {
    for (const text of chunks)
      yield { type: "text-delta", id: "text", text } as TextStreamPart<any>;
  })();
  for await (const chunk of extractSummaryTagFooter(stream, {
    signal,
    onResult,
  })) {
    if (chunk.type === "text-delta") output.push(chunk.text);
  }
  return { text: output.join(""), onResult };
}

afterEach(() => vi.restoreAllMocks());

describe("summary tag footer", () => {
  const summary = "# Release\n- Ship Friday.\n\n";
  const footer =
    '<loofah-tags>{"tags":[{"name":"Release","confidence":0.93}]}</loofah-tags>';

  it("removes the footer at every possible split boundary", async () => {
    for (let boundary = 0; boundary <= footer.length; boundary++) {
      const result = await collect([
        summary + footer.slice(0, boundary),
        footer.slice(boundary),
      ]);
      expect(result.text).toBe(summary);
      expect(result.onResult).toHaveBeenCalledWith([
        { name: "Release", confidence: 0.93 },
      ]);
    }
  });

  it("handles single-character chunks and Unicode names", async () => {
    const result = await collect(
      Array.from(
        summary +
          '<loofah-tags>{"tags":[{"name":"研究","confidence":0.75}]}</loofah-tags>',
      ),
    );
    expect(result.text).toBe(summary);
    expect(result.onResult).toHaveBeenCalledWith([
      { name: "研究", confidence: 0.75 },
    ]);
  });

  it("distinguishes an empty suggestion set from missing metadata", async () => {
    const result = await collect([
      summary + '<loofah-tags>{"tags":[]}</loofah-tags>',
    ]);
    expect(result.onResult).toHaveBeenCalledWith([]);
  });

  it("accepts confidence values at both ends of the range", async () => {
    const result = await collect([
      summary +
        '<loofah-tags>{"tags":[{"name":"possible","confidence":0},{"name":"certain","confidence":1}]}</loofah-tags>',
    ]);
    expect(result.onResult).toHaveBeenCalledWith([
      { name: "possible", confidence: 0 },
      { name: "certain", confidence: 1 },
    ]);
  });

  it.each([
    "",
    "<loofah-tags",
    ...Array.from({ length: "<loofah-tags".length - 2 }, (_, index) =>
      "<loofah-tags".slice(0, index + 2),
    ),
    '<loofah-tags>{"tags":["Release"]}',
    "<loofah-tags>{bad}</loofah-tags>",
    '<loofah-tags>{"tags":"Release"}</loofah-tags>',
    '<loofah-tags>{"tags":["a", "b", "c", "d"]}</loofah-tags>',
    '<loofah-tags>{"tags":[2]}</loofah-tags>',
    '<loofah-tags>{"tags":[{"name":"Release"}]}</loofah-tags>',
    '<loofah-tags>{"tags":[{"name":"Release","confidence":"0.9"}]}</loofah-tags>',
    '<loofah-tags>{"tags":[{"name":"Release","confidence":null}]}</loofah-tags>',
    '<loofah-tags>{"tags":[{"name":"Release","confidence":-0.01}]}</loofah-tags>',
    '<loofah-tags>{"tags":[{"name":"Release","confidence":1.01}]}</loofah-tags>',
    '<loofah-tags>{"tags":[{"name":"Release","confidence":1e999}]}</loofah-tags>',
    '<loofah-tags>{"tags":[" "]}</loofah-tags>',
    '<loofah-tags>{"tags":[]}</loofah-tags> extra',
    '<loofah-tags>{"tags":[]}</loofah-tags><loofah-tags>{"tags":[]}</loofah-tags>',
    "<loofah-tags>" + "x".repeat(MAX_TAG_FOOTER_LENGTH) + "</loofah-tags>",
  ])("keeps usable Markdown when metadata is invalid: %s", async (invalid) => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const result = await collect([summary, invalid]);
    expect(result.text).toBe(summary);
    expect(result.onResult).toHaveBeenCalledWith(undefined);
    expect(warn).toHaveBeenCalled();
  });

  it("keeps ordinary less-than text", async () => {
    const result = await collect(["# Scope\n- Cost <", " 100.", footer]);
    expect(result.text).toBe("# Scope\n- Cost < 100.");
  });

  it("does not publish suggestions after cancellation", async () => {
    const controller = new AbortController();
    controller.abort();
    const result = await collect([summary + footer], controller.signal);
    expect(result.onResult).not.toHaveBeenCalled();
  });
});

it("does not accept metadata from a token-limited completion", async () => {
  vi.spyOn(console, "warn").mockImplementation(() => {});
  const onResult = vi.fn();
  const stream = (async function* () {
    yield {
      type: "text-delta",
      text: '# Decisions\n- Ship Friday<loofah-tags>{"tags":["Launch"]}</loofah-tags>',
    } as any;
    yield { type: "finish", finishReason: "length" } as any;
  })();
  for await (const _chunk of extractSummaryTagFooter(stream, {
    signal: new AbortController().signal,
    onResult,
  })) {
    /* drain */
  }
  expect(onResult).toHaveBeenCalledWith(undefined);
});
