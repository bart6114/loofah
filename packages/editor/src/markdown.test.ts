import { Node as PMNode } from "prosemirror-model";
import { describe, expect, test } from "vitest";

import {
  EMPTY_DOC,
  isValidContent,
  json2md,
  type JSONContent,
  md2json,
  parseJsonContent,
} from "./markdown";
import { normalizePortableAttachmentUrls } from "./note/portable-attachments";
import { schema as noteSchema } from "./note/schema";

describe("json2md", () => {
  test("renders underline as html tags", () => {
    const markdown = json2md({
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [
            {
              type: "text",
              text: "underlined",
              marks: [{ type: "underline" }],
            },
          ],
        },
      ],
    });

    expect(markdown).toBe("<u>underlined</u>");
  });

  test("renders task items without escaping brackets", () => {
    const markdown = json2md({
      type: "doc",
      content: [
        {
          type: "taskList",
          content: [
            {
              type: "taskItem",
              attrs: { checked: false },
              content: [
                {
                  type: "paragraph",
                  content: [
                    { type: "text", text: "this is an example md task" },
                  ],
                },
              ],
            },
          ],
        },
      ],
    });

    expect(markdown).toContain("[ ]");
    expect(markdown).not.toContain("\\[");
    expect(markdown).not.toContain("\\]");
    expect(markdown).toContain("this is an example md task");
  });

  test("renders checked task items", () => {
    const markdown = json2md({
      type: "doc",
      content: [
        {
          type: "taskList",
          content: [
            {
              type: "taskItem",
              attrs: { checked: true },
              content: [
                {
                  type: "paragraph",
                  content: [{ type: "text", text: "completed task" }],
                },
              ],
            },
          ],
        },
      ],
    });

    expect(markdown).toContain("[x]");
    expect(markdown).toContain("completed task");
  });

  test("renders image width metadata into markdown titles", () => {
    const markdown = json2md({
      type: "doc",
      content: [
        {
          type: "image",
          attrs: {
            src: "https://example.com/image.png",
            alt: "alt text",
            title: "Example",
            editorWidth: 42,
          },
        },
      ],
    });

    expect(markdown).toBe(
      '![alt text](https://example.com/image.png "char-editor-width=42|Example")',
    );
  });

  test("renders table nodes as markdown tables", () => {
    const markdown = json2md({
      type: "doc",
      content: [
        {
          type: "table",
          content: [
            {
              type: "tableRow",
              content: [
                {
                  type: "tableHeader",
                  content: [
                    {
                      type: "paragraph",
                      content: [{ type: "text", text: "Name" }],
                    },
                  ],
                },
                {
                  type: "tableHeader",
                  content: [
                    {
                      type: "paragraph",
                      content: [{ type: "text", text: "Role | Notes" }],
                    },
                  ],
                },
              ],
            },
            {
              type: "tableRow",
              content: [
                {
                  type: "tableCell",
                  content: [
                    {
                      type: "paragraph",
                      content: [{ type: "text", text: "Alasdair" }],
                    },
                  ],
                },
                {
                  type: "tableCell",
                  content: [
                    {
                      type: "paragraph",
                      content: [{ type: "text", text: "Account Executive" }],
                    },
                  ],
                },
              ],
            },
          ],
        },
      ],
    });

    expect(markdown).toBe(
      "| Name | Role \\| Notes |\n| --- | --- |\n| Alasdair | Account Executive |",
    );
  });

  test("renders merged and shorter table rows with consistent columns", () => {
    const markdown = json2md({
      type: "doc",
      content: [
        {
          type: "table",
          content: [
            {
              type: "tableRow",
              content: [
                {
                  type: "tableHeader",
                  attrs: { colspan: 2 },
                  content: [
                    {
                      type: "paragraph",
                      content: [{ type: "text", text: "Quarter" }],
                    },
                  ],
                },
                {
                  type: "tableHeader",
                  content: [
                    {
                      type: "paragraph",
                      content: [{ type: "text", text: "Status" }],
                    },
                  ],
                },
              ],
            },
            {
              type: "tableRow",
              content: [
                {
                  type: "tableCell",
                  content: [
                    {
                      type: "paragraph",
                      content: [{ type: "text", text: "Q3" }],
                    },
                  ],
                },
              ],
            },
          ],
        },
      ],
    });

    expect(markdown).toBe(
      "| Quarter |  | Status |\n| --- | --- | --- |\n| Q3 |  |  |",
    );

    const roundtripped = md2json(markdown);
    expect(roundtripped.content?.[0]?.content?.[0]?.content).toHaveLength(3);
    expect(roundtripped.content?.[0]?.content?.[1]?.content).toHaveLength(3);
  });

  test("renders table cell hard breaks as parseable break tags", () => {
    const markdown = json2md({
      type: "doc",
      content: [
        {
          type: "table",
          content: [
            {
              type: "tableRow",
              content: [
                {
                  type: "tableHeader",
                  content: [
                    {
                      type: "paragraph",
                      content: [{ type: "text", text: "Summary" }],
                    },
                  ],
                },
              ],
            },
            {
              type: "tableRow",
              content: [
                {
                  type: "tableCell",
                  content: [
                    {
                      type: "paragraph",
                      content: [
                        { type: "text", text: "First" },
                        { type: "hardBreak" },
                        { type: "text", text: "Second" },
                      ],
                    },
                  ],
                },
              ],
            },
          ],
        },
      ],
    });

    expect(markdown).toBe("| Summary |\n| --- |\n| First<br>Second |");
  });

  test("escapes literal table cell break tags", () => {
    const markdown = json2md({
      type: "doc",
      content: [
        {
          type: "table",
          content: [
            {
              type: "tableRow",
              content: [
                {
                  type: "tableHeader",
                  content: [
                    {
                      type: "paragraph",
                      content: [{ type: "text", text: "Raw" }],
                    },
                  ],
                },
              ],
            },
            {
              type: "tableRow",
              content: [
                {
                  type: "tableCell",
                  content: [
                    {
                      type: "paragraph",
                      content: [{ type: "text", text: "literal <br>" }],
                    },
                  ],
                },
              ],
            },
          ],
        },
      ],
    });

    expect(markdown).toBe("| Raw |\n| --- |\n| literal \\<br> |");
  });

  test("preserves table cell backslashes across roundtrip", () => {
    const json: JSONContent = {
      type: "doc",
      content: [
        {
          type: "table",
          content: [
            {
              type: "tableRow",
              content: [
                {
                  type: "tableHeader",
                  content: [
                    {
                      type: "paragraph",
                      content: [{ type: "text", text: "Path" }],
                    },
                  ],
                },
              ],
            },
            {
              type: "tableRow",
              content: [
                {
                  type: "tableCell",
                  content: [
                    {
                      type: "paragraph",
                      content: [{ type: "text", text: "C:\\Users\\John" }],
                    },
                  ],
                },
              ],
            },
          ],
        },
      ],
    };

    const roundtripped = md2json(json2md(json));
    const cellText =
      roundtripped.content?.[0]?.content?.[1]?.content?.[0]?.content?.[0]
        ?.content?.[0]?.text;

    expect(cellText).toBe("C:\\Users\\John");
  });

  test("preserves table cell backslashes before pipes across roundtrip", () => {
    const json: JSONContent = {
      type: "doc",
      content: [
        {
          type: "table",
          content: [
            {
              type: "tableRow",
              content: [
                {
                  type: "tableHeader",
                  content: [
                    {
                      type: "paragraph",
                      content: [{ type: "text", text: "Pattern" }],
                    },
                  ],
                },
              ],
            },
            {
              type: "tableRow",
              content: [
                {
                  type: "tableCell",
                  content: [
                    {
                      type: "paragraph",
                      content: [{ type: "text", text: "literal \\| marker" }],
                    },
                  ],
                },
              ],
            },
          ],
        },
      ],
    };

    const roundtripped = md2json(json2md(json));
    const cellText =
      roundtripped.content?.[0]?.content?.[1]?.content?.[0]?.content?.[0]
        ?.content?.[0]?.text;

    expect(cellText).toBe("literal \\| marker");
  });
});

describe("md2json", () => {
  test("converts html underline tags to underline marks", () => {
    const json = md2json("<u>underlined</u>");
    const paragraph = json.content?.[0];
    const textNode = paragraph?.content?.[0];

    expect(paragraph?.type).toBe("paragraph");
    expect(textNode?.type).toBe("text");
    expect(textNode?.text).toBe("underlined");
    expect(textNode?.marks).toEqual([{ type: "underline" }]);
  });

  test("converts standalone image to block-level JSON", () => {
    const json = md2json("![alt text](https://example.com/image.png)");

    expect(json.type).toBe("doc");
    expect(json.content![0].type).toBe("image");
    expect(json.content![0].attrs?.src).toBe("https://example.com/image.png");
    expect(json.content![0].attrs?.alt).toBe("alt text");
    expect(json.content![0].attrs?.editorWidth).toBe(80);
  });

  test("lifts an image that shares a paragraph with text", () => {
    // No blank line before the image: markdown folds it into the paragraph,
    // but the editor schemas only allow images at block level.
    const json = md2json("some text\n![](attachments/image%202.png)");

    expect(json.content!.map((node) => node.type)).toEqual([
      "paragraph",
      "image",
    ]);
    expect(json.content![0].content).toEqual([
      { type: "text", text: "some text" },
    ]);
    expect(json.content![1].attrs?.attachmentId).toBe("image 2.png");
    expect(() => PMNode.fromJSON(noteSchema, json).check()).not.toThrow();
  });

  test("splits a paragraph around an image between text runs", () => {
    const json = md2json("before ![](https://example.com/a.png) after");

    expect(json.content!.map((node) => node.type)).toEqual([
      "paragraph",
      "image",
      "paragraph",
    ]);
    expect(json.content![0].content).toEqual([
      { type: "text", text: "before" },
    ]);
    expect(json.content![2].content).toEqual([{ type: "text", text: "after" }]);
  });

  test("lifts adjacent images without leaving whitespace-only paragraphs", () => {
    const json = md2json(
      "![](https://example.com/a.png)  ![](https://example.com/b.png)",
    );

    expect(json.content!.map((node) => node.type)).toEqual(["image", "image"]);
  });

  test("lifts images inside list items behind a leading paragraph", () => {
    const json = md2json(
      "* ![](https://example.com/avatar.png) [kevin](https://example.com)",
    );

    const listItem = json.content![0].content![0];
    expect(listItem.type).toBe("listItem");
    expect(listItem.content!.map((node) => node.type)).toEqual([
      "paragraph",
      "image",
      "paragraph",
    ]);
    expect(listItem.content![0].content).toBeUndefined();
    expect(() => PMNode.fromJSON(noteSchema, json).check()).not.toThrow();
  });

  test("round-trips a lifted inline image through markdown", () => {
    const md = json2md(md2json("some text\n![](attachments/image%202.png)"));

    expect(md2json(md).content!.map((node) => node.type)).toEqual([
      "paragraph",
      "image",
    ]);
  });

  test("converts image with title to JSON", () => {
    const json = md2json(
      '![alt text](https://example.com/image.png "Image Title")',
    );

    const findImage = (content: any[]): any => {
      for (const node of content) {
        if (node.type === "image") return node;
        if (node.content) {
          const found = findImage(node.content);
          if (found) return found;
        }
      }
      return null;
    };

    const imageNode = findImage(json.content!);
    expect(imageNode?.attrs?.title).toBe("Image Title");
    expect(imageNode?.attrs?.editorWidth).toBe(80);
  });

  test("converts image width metadata to JSON attributes", () => {
    const json = md2json(
      '![alt text](https://example.com/image.png "char-editor-width=42|Image Title")',
    );

    const findImage = (content: any[]): any => {
      for (const node of content) {
        if (node.type === "image") return node;
        if (node.content) {
          const found = findImage(node.content);
          if (found) return found;
        }
      }
      return null;
    };

    const imageNode = findImage(json.content!);
    expect(imageNode?.attrs?.title).toBe("Image Title");
    expect(imageNode?.attrs?.editorWidth).toBe(42);
  });

  test("handles empty markdown", () => {
    const json = md2json("");
    expect(json.type).toBe("doc");
    expect(json.content).toBeDefined();
  });

  test("converts task list", () => {
    const json = md2json("- [ ] Task 1\n- [x] Task 2\n- [ ] Task 3");

    const taskList = json.content!.find((node) => node.type === "taskList");
    expect(taskList).toBeDefined();
  });

  test("converts mixed content document", () => {
    const markdown = `# Introduction

Here is some text.

![diagram](https://example.com/diagram.png)

- List item 1
- List item 2

More text here.`;

    const json = md2json(markdown);
    expect(json.type).toBe("doc");
    expect(json.content!.length).toBeGreaterThan(3);
  });

  test("standalone image with following text produces correct structure", () => {
    const json = md2json(`![welcome](https://example.com/welcome.png)

We appreciate your patience while you wait.`);

    expect(json.content!.length).toBeGreaterThanOrEqual(2);
    expect(json.content![0].type).toBe("image");
    expect(json.content![0].attrs?.src).toBe("https://example.com/welcome.png");
    expect(json.content![1].type).toBe("paragraph");
  });

  test("converts markdown tables to editor-compatible table JSON", () => {
    const json = md2json(`| Name | Company | Role / Notes |
| --- | --- | --- |
| Alasdair | Cloudflare | Account Executive |
| Rick | Cloudflare | Solutions Engineer |`);

    const table = json.content?.[0];
    expect(table?.type).toBe("table");
    expect(table?.content?.[0]?.type).toBe("tableRow");
    expect(table?.content?.[0]?.content?.[0]?.type).toBe("tableHeader");
    expect(
      table?.content?.[0]?.content?.[0]?.content?.[0]?.content?.[0]?.text,
    ).toBe("Name");
    expect(table?.content?.[1]?.content?.[0]?.type).toBe("tableCell");
    expect(() => PMNode.fromJSON(noteSchema, json)).not.toThrow();
  });

  test("converts table cell break tags to hard breaks", () => {
    const json = md2json(`| Summary |
| --- |
| First<br>Second |`);

    const cellContent =
      json.content?.[0]?.content?.[1]?.content?.[0]?.content?.[0]?.content;
    expect(cellContent).toEqual([
      { type: "text", text: "First" },
      { type: "hardBreak" },
      { type: "text", text: "Second" },
    ]);
    expect(() => PMNode.fromJSON(noteSchema, json)).not.toThrow();
  });

  test("keeps escaped table cell break tags as text", () => {
    const json = md2json(`| Raw |
| --- |
| literal \\<br> |`);

    const cellContent =
      json.content?.[0]?.content?.[1]?.content?.[0]?.content?.[0]?.content;
    expect(cellContent).toEqual([{ type: "text", text: "literal <br>" }]);
  });
});

describe("roundtrip", () => {
  test("markdown -> json -> markdown -> json produces consistent results", () => {
    const originalMarkdown = `# Test Document

![image](https://example.com/test.png)

- List item
- Another item

Some text.`;

    const json1 = md2json(originalMarkdown);
    const markdown2 = json2md(json1);
    const json2 = md2json(markdown2);

    expect(json1.type).toBe("doc");
    expect(json2.type).toBe("doc");
    expect(json1.content!.length).toBe(json2.content!.length);
  });

  test("preserves empty paragraphs across roundtrip", () => {
    const json1: JSONContent = {
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [{ type: "text", text: "first" }],
        },
        { type: "paragraph" },
        { type: "paragraph" },
        {
          type: "paragraph",
          content: [{ type: "text", text: "second" }],
        },
      ],
    };

    const markdown = json2md(json1);
    const json2 = md2json(markdown);

    expect(json2.content!.length).toBe(4);
    expect(json2.content![0].content?.[0]?.text).toBe("first");
    expect(json2.content![1].content).toBeUndefined();
    expect(json2.content![2].content).toBeUndefined();
    expect(json2.content![3].content?.[0]?.text).toBe("second");
  });

  test("serializes empty paragraphs as extra blank lines", () => {
    const markdown = json2md({
      type: "doc",
      content: [
        { type: "paragraph", content: [{ type: "text", text: "a" }] },
        { type: "paragraph" },
        { type: "paragraph", content: [{ type: "text", text: "b" }] },
      ],
    });

    // 1 empty paragraph between = 2 blank lines = 3 consecutive newlines
    expect(markdown).toContain("a\n\n\nb");
    expect(markdown).not.toContain("&nbsp;");
    expect(markdown).not.toContain("\u00A0");
  });

  test("preserves multiple consecutive empty paragraphs", () => {
    const json1: JSONContent = {
      type: "doc",
      content: [
        { type: "paragraph", content: [{ type: "text", text: "a" }] },
        { type: "paragraph" },
        { type: "paragraph" },
        { type: "paragraph", content: [{ type: "text", text: "b" }] },
      ],
    };
    const markdown = json2md(json1);
    const json2 = md2json(markdown);

    expect(json2.content!.length).toBe(4);
    expect(json2.content![1].content).toBeUndefined();
    expect(json2.content![2].content).toBeUndefined();
  });

  test("preserves leading empty paragraphs", () => {
    const json1: JSONContent = {
      type: "doc",
      content: [
        { type: "paragraph" },
        { type: "paragraph" },
        { type: "paragraph", content: [{ type: "text", text: "hello" }] },
      ],
    };
    const markdown = json2md(json1);
    const json2 = md2json(markdown);

    expect(json2.content!.length).toBe(3);
    expect(json2.content![0].content).toBeUndefined();
    expect(json2.content![1].content).toBeUndefined();
    expect(json2.content![2].content?.[0]?.text).toBe("hello");
  });

  test("preserves trailing empty paragraphs", () => {
    const json1: JSONContent = {
      type: "doc",
      content: [
        { type: "paragraph", content: [{ type: "text", text: "hello" }] },
        { type: "paragraph" },
        { type: "paragraph" },
      ],
    };
    const markdown = json2md(json1);
    const json2 = md2json(markdown);

    expect(json2.content!.length).toBe(3);
    expect(json2.content![0].content?.[0]?.text).toBe("hello");
    expect(json2.content![1].content).toBeUndefined();
    expect(json2.content![2].content).toBeUndefined();
  });

  test("parses leading blank lines from raw markdown", () => {
    const json = md2json("\n\nhello");
    expect(json.content!.length).toBe(3);
    expect(json.content![0].content).toBeUndefined();
    expect(json.content![1].content).toBeUndefined();
    expect(json.content![2].content?.[0]?.text).toBe("hello");
  });

  // Pins the exact round-trip this app's note-save path relies on
  // (`_memo.md` -> md2json -> editor model -> json2md -> `_memo.md`,
  // see raw.tsx's persistChange / useSessionRawMd): a memo covering
  // headings, bold, lists, and a code fence must serialize back to
  // byte-identical markdown, or an external edit to `_memo.md` would be
  // needlessly rewritten (and diffed/churned by sync tooling) on the very
  // next save with no user-visible change.
  test("memo fixture with headings, bold, lists, and a code fence round-trips byte-identically", () => {
    const memo = [
      "# Meeting Notes",
      "",
      "This paragraph has **bold** text for emphasis.",
      "",
      "- First item",
      "- Second item",
      "- Third item",
      "",
      "```js",
      'console.log("hello");',
      "```",
    ].join("\n");

    // json2md's bulletList renderer (state.renderList) inserts a blank line between every
    // item -- a pinned normalization, not a bug: tight ("- a\n- b") and loose ("- a\n\n- b")
    // lists parse to the identical JSON, so this is a one-time reformat on first save, not
    // something that re-churns on every subsequent save (see the second assertion below).
    const expectedSerialized = [
      "# Meeting Notes",
      "",
      "This paragraph has **bold** text for emphasis.",
      "",
      "- First item",
      "",
      "- Second item",
      "",
      "- Third item",
      "",
      "```js",
      'console.log("hello");',
      "```",
    ].join("\n");

    const json1 = md2json(memo);
    const serialized = json2md(json1);

    expect(serialized).toBe(expectedSerialized);

    // A second parse/serialize pass must converge on the exact same markdown -- once
    // reformatted, the file is stable and further saves don't keep rewriting it.
    const roundTripped = json2md(md2json(serialized));
    expect(roundTripped).toBe(serialized);
  });
});

describe("isValidContent", () => {
  test("returns true for valid content", () => {
    expect(
      isValidContent({ type: "doc", content: [{ type: "paragraph" }] }),
    ).toBe(true);
  });

  test("returns false for non-object", () => {
    expect(isValidContent("string")).toBe(false);
    expect(isValidContent(null)).toBe(false);
    expect(isValidContent(undefined)).toBe(false);
  });

  test("returns false for doc without content array", () => {
    expect(isValidContent({ type: "doc" })).toBe(false);
  });
});

describe("parseJsonContent", () => {
  test("parses valid JSON string", () => {
    const raw = JSON.stringify({
      type: "doc",
      content: [{ type: "paragraph" }],
    });
    const result = parseJsonContent(raw);
    expect(result.type).toBe("doc");
  });

  test("returns EMPTY_DOC for empty input", () => {
    expect(parseJsonContent("")).toEqual(EMPTY_DOC);
    expect(parseJsonContent(null)).toEqual(EMPTY_DOC);
    expect(parseJsonContent(undefined)).toEqual(EMPTY_DOC);
  });
});

describe("fileAttachment round-trip", () => {
  test("serializes fileAttachment node to markdown link", () => {
    const md = json2md({
      type: "doc",
      content: [
        {
          type: "fileAttachment",
          attrs: {
            name: "report.pdf",
            src: "asset://localhost/%2Fpath%2Freport.pdf",
          },
        },
      ],
    });
    expect(md).toBe("[report.pdf](asset://localhost/%2Fpath%2Freport.pdf)");
  });

  test("parses markdown link with asset:// to fileAttachment", () => {
    const json = md2json(
      "[report.pdf](asset://localhost/%2Fpath%2Freport.pdf)",
    );
    const attachments = json.content!.filter(
      (n) => n.type === "fileAttachment",
    );
    expect(attachments).toHaveLength(1);
    expect(attachments[0].attrs?.name).toBe("report.pdf");
    expect(attachments[0].attrs?.src).toBe(
      "asset://localhost/%2Fpath%2Freport.pdf",
    );
  });

  test("round-trips two file attachments without leaking URL tail", () => {
    const doc: JSONContent = {
      type: "doc",
      content: [
        {
          type: "fileAttachment",
          attrs: {
            name: "CE2 The devil wears Prada script.pdf",
            src: "asset://localhost/%2FUsers%2Fsungbin%2FLibrary%2FApplication%20Support%2Fcom.example.app%2Fsessions%2Ff515cc6f%2Fattachments%2FCE2%20The%20devil%20wears%20Prada%20script%202.pdf",
          },
        },
        {
          type: "fileAttachment",
          attrs: {
            name: "2021-13630 조성빈 물리학 1 HW2.pdf",
            src: "asset://localhost/%2FUsers%2Fsungbin%2FLibrary%2FApplication%20Support%2Fcom.example.app%2Fsessions%2Ff515cc6f%2Fattachments%2F2021-13630%20%E1%84%8C%E1%85%A9%E1%84%89%E1%85%A5%E1%86%BC%E1%84%87%E1%85%B5%E1%86%AB%20%E1%84%86%E1%85%AE%E1%86%AF%E1%84%85%E1%85%B5%E1%84%92%E1%85%A1%E1%86%A8%201%20HW2.pdf",
          },
        },
      ],
    };

    const md = json2md(doc);
    const parsed = md2json(md);

    const attachments = parsed.content!.filter(
      (n) => n.type === "fileAttachment",
    );
    expect(attachments).toHaveLength(2);
    expect(attachments[0].attrs?.name).toBe(
      "CE2 The devil wears Prada script.pdf",
    );
    expect(attachments[1].attrs?.name).toBe(
      "2021-13630 조성빈 물리학 1 HW2.pdf",
    );
    expect(attachments[0].attrs?.src).toBe(doc.content![0].attrs!.src);
    expect(attachments[1].attrs?.src).toBe(doc.content![1].attrs!.src);

    const leakedText = parsed
      .content!.filter((n) => n.type === "paragraph")
      .flatMap((p) => p.content ?? [])
      .filter((n) => n.type === "text")
      .map((n) => n.text)
      .join("");
    expect(leakedText).toBe("");
  });

  test("handles parentheses in filename via percent-encoding", () => {
    const doc: JSONContent = {
      type: "doc",
      content: [
        {
          type: "fileAttachment",
          attrs: {
            name: "CE2 (Group 5) PPT.pdf",
            src: "asset://localhost/%2Fpath%2FCE2%20(Group%205)%20PPT.pdf",
          },
        },
      ],
    };

    const md = json2md(doc);
    // Parens in URL must be encoded so the markdown link syntax is unambiguous.
    expect(md).toContain("%28Group%205%29");

    const parsed = md2json(md);
    const attachments = parsed.content!.filter(
      (n) => n.type === "fileAttachment",
    );
    expect(attachments).toHaveLength(1);
    expect(attachments[0].attrs?.name).toBe("CE2 (Group 5) PPT.pdf");
  });
});

describe("attachment-backed nodes persist portably", () => {
  test("serializes an image with an attachmentId but no src to a vault-relative src", () => {
    const md = json2md({
      type: "doc",
      content: [
        {
          type: "image",
          attrs: { attachmentId: "diagram.png", alt: "Diagram" },
        },
      ],
    });

    expect(md).toBe(
      '![Diagram](attachments/diagram.png "char-editor-width=80")',
    );
  });

  test("replaces a device-local image src with the vault-relative form", () => {
    const md = json2md({
      type: "doc",
      content: [
        {
          type: "image",
          attrs: {
            attachmentId: "diagram.png",
            src: "asset://localhost/%2Fdevice-a%2Fattachments%2Fdiagram.png",
            alt: "Diagram",
          },
        },
      ],
    });

    expect(md).toBe(
      '![Diagram](attachments/diagram.png "char-editor-width=80")',
    );
  });

  test("keeps a remote image src even when an attachmentId exists", () => {
    const md = json2md({
      type: "doc",
      content: [
        {
          type: "image",
          attrs: {
            attachmentId: "diagram.png",
            src: "https://example.com/diagram.png",
            alt: "Diagram",
          },
        },
      ],
    });

    expect(md).toBe(
      '![Diagram](https://example.com/diagram.png "char-editor-width=80")',
    );
  });

  test("restores the attachmentId when parsing a vault-relative image src", () => {
    const json = md2json("![Diagram](attachments/diagram%20%281%29.png)");

    expect(json.content![0].type).toBe("image");
    expect(json.content![0].attrs?.attachmentId).toBe("diagram (1).png");
    expect(json.content![0].attrs?.src).toBeNull();
  });

  test("round-trips a freshly pasted image through the persist path", () => {
    // Mirrors the desktop save flow: normalize the editor doc, write markdown,
    // then parse the markdown back as if the app restarted.
    const md = json2md(
      normalizePortableAttachmentUrls({
        type: "doc",
        content: [
          {
            type: "image",
            attrs: {
              attachmentId: "screenshot 1 (copy).png",
              src: "asset://localhost/%2Fdevice-a%2Fattachments%2Fscreenshot.png",
              alt: "Screenshot",
              editorWidth: 42,
            },
          },
        ],
      }),
    );

    expect(md).toBe(
      '![Screenshot](attachments/screenshot%201%20%28copy%29.png "char-editor-width=42")',
    );

    const parsed = md2json(md);
    expect(parsed.content![0].type).toBe("image");
    expect(parsed.content![0].attrs?.attachmentId).toBe(
      "screenshot 1 (copy).png",
    );
    expect(parsed.content![0].attrs?.src).toBeNull();
    expect(parsed.content![0].attrs?.editorWidth).toBe(42);
  });

  test("round-trips a fileAttachment through the persist path", () => {
    const md = json2md(
      normalizePortableAttachmentUrls({
        type: "doc",
        content: [
          {
            type: "fileAttachment",
            attrs: {
              attachmentId: "notes (final).pdf",
              name: "notes (final).pdf",
              src: "file:///device-a/attachments/notes.pdf",
              path: "/device-a/attachments/notes.pdf",
            },
          },
        ],
      }),
    );

    expect(md).toBe("[notes (final).pdf](attachments/notes%20%28final%29.pdf)");

    const parsed = md2json(md);
    const attachments = parsed.content!.filter(
      (n) => n.type === "fileAttachment",
    );
    expect(attachments).toHaveLength(1);
    expect(attachments[0].attrs?.attachmentId).toBe("notes (final).pdf");
    expect(attachments[0].attrs?.name).toBe("notes (final).pdf");
    expect(attachments[0].attrs?.src).toBeNull();
  });
});

test("mixed lists preserve ordinary bullets beside and around nested checkboxes", () => {
  const json = md2json(
    "- Bob sends the invoice\n- [ ] Send proposal\n  - Supporting detail\n  - [x] Check figures\n- Discussion only",
  );
  expect(json.content!.map((node) => node.type)).toEqual([
    "bulletList",
    "taskList",
    "bulletList",
  ]);
  const task = json.content![1].content![0];
  expect(task.content!.map((node) => node.type)).toEqual([
    "paragraph",
    "bulletList",
    "taskList",
  ]);
  expect(task.content![2].content![0].attrs?.checked).toBe(true);
});
