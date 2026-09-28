import { describe, expect, it } from "vitest";

import { json2md, md2json } from "../markdown";
import { normalizeTaskContent } from "../tasks";
import { areEquivalentEditorContents } from "./content-equivalence";
import type { JSONContent } from "./index";

describe("persisted editor content equivalence", () => {
  it("does not equate documents the Markdown serializer cannot represent", () => {
    const linked = (url: string): JSONContent => ({
      type: "doc",
      content: [
        { type: "paragraph", content: [{ type: "appLink", attrs: { url } }] },
      ],
    });
    expect(
      areEquivalentEditorContents(
        linked("https://example.com/one"),
        linked("https://example.com/two"),
      ),
    ).toBe(false);
  });
  it("recognizes a Markdown round trip with regenerated task identities", () => {
    const original = normalizeTaskContent(md2json("# Title\n\n- [x] Done"))!;
    const loaded = normalizeTaskContent(md2json(json2md(original)))!;
    expect(original).not.toEqual(loaded);
    expect(areEquivalentEditorContents(original, loaded)).toBe(true);
  });

  it("distinguishes in-progress tasks from unchecked tasks", () => {
    const original = normalizeTaskContent(md2json("- [ ] Working"))!;
    const changed = structuredClone(original);
    changed.content![0].content![0].attrs!.status = "in_progress";
    expect(json2md(original)).toBe(json2md(changed));
    expect(areEquivalentEditorContents(original, changed)).toBe(false);
  });

  it("recognizes resolved attachment URLs without ignoring attachment identity", () => {
    const original: JSONContent = {
      type: "doc",
      content: [
        {
          type: "image",
          attrs: {
            attachmentId: "image.png",
            src: "asset://localhost/vault/image.png",
          },
        },
        { type: "paragraph" },
      ],
    };
    const loaded = md2json(json2md(original));
    expect(areEquivalentEditorContents(original, loaded)).toBe(true);
    const other = structuredClone(original);
    other.content![0].attrs!.attachmentId = "other.png";
    expect(areEquivalentEditorContents(original, other)).toBe(false);
  });

  it("preserves meaningful empty paragraphs", () => {
    const original: JSONContent = {
      type: "doc",
      content: [
        { type: "paragraph", content: [{ type: "text", text: "Body" }] },
        { type: "paragraph" },
        { type: "paragraph" },
      ],
    };
    const loaded = md2json(json2md(original));
    expect(areEquivalentEditorContents(original, loaded)).toBe(true);
    expect(
      areEquivalentEditorContents(original, {
        ...original,
        content: original.content!.slice(0, 1),
      }),
    ).toBe(false);
  });
});
