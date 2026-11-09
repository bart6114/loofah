import { describe, expect, it } from "vitest";

import { json2mdStrict, md2json } from "@hypr/editor/markdown";
import type { JSONContent } from "@hypr/editor/note";
import { normalizeTaskContent } from "@hypr/editor/tasks";

import {
  captureNoteEditorContent,
  noteEditorContent,
  editorContent,
  captureEditorContent,
} from "./note-editor-content";
import { taskItemToRecord } from "./task-records";

import type { EditorText } from "~/shared/application/note-editor-draft";

function draft(): EditorText {
  return {
    title: "Title",
    markdown:
      "# Title\n\n- [ ] Follow up\n\n| Keep | Table |\n| --- | --- |\n| Rich | Content |",
    tasks: [
      {
        id: "stable-task",
        source_order: 0,
        status: "in_progress",
        text: "Follow up",
        body: [
          { type: "paragraph", content: [{ type: "text", text: "Follow up" }] },
        ],
        due_at: "2027-01-01",
      },
    ],
  };
}
function taskNodes(content: JSONContent): JSONContent[] {
  return (content.type === "taskItem" ? [content] : []).concat(
    (content.content ?? []).flatMap(taskNodes),
  );
}

describe("transport independent main-editor content bridge", () => {
  it("hydrates stable task intent and preserves a renamed task's identity, status and date across Markdown saves", () => {
    const input = draft();
    const original = structuredClone(input);
    const { content, tasks } = noteEditorContent("note", input);
    expect(input).toEqual(original);
    const node = taskNodes(content)[0];
    expect(node.attrs).toMatchObject({
      taskId: "stable-task",
      status: "in_progress",
    });
    node.content![0].content![0].text = "Renamed follow up";
    const saved = captureNoteEditorContent("note", content, tasks);
    expect(saved.tasks).toEqual([
      {
        ...input.tasks[0],
        text: "Renamed follow up",
        body: [
          {
            type: "paragraph",
            content: [{ type: "text", text: "Renamed follow up" }],
          },
        ],
      },
    ]);
    expect(saved.markdown).toContain("- [ ] Renamed follow up");
    expect(saved.markdown).toContain("| Keep | Table |");
    expect(saved.title).toBe("Title");
    const reopened = noteEditorContent("note", saved);
    expect(taskNodes(reopened.content)[0].attrs).toMatchObject({
      taskId: "stable-task",
      status: "in_progress",
    });
    expect(reopened.tasks[0].dueDate).toBe("2027-01-01");
  });

  it("captures due-date-only changes from the scoped task store and explicit checkbox changes from the document", () => {
    const { content, tasks } = noteEditorContent("note", draft());
    const before = captureNoteEditorContent("note", content, tasks);
    tasks[0].dueDate = "2027-02-01";
    const next = captureNoteEditorContent("note", content, tasks);
    expect(next.markdown).toBe(before.markdown);
    expect(next.tasks[0]).toMatchObject({
      id: "stable-task",
      status: "in_progress",
      due_at: "2027-02-01",
    });
    taskNodes(content)[0].attrs = {
      ...taskNodes(content)[0].attrs,
      status: "todo",
      checked: false,
    };
    expect(
      captureNoteEditorContent("note", content, tasks).tasks[0],
    ).toMatchObject({ status: "todo", due_at: "2027-02-01" });
  });

  it("serializes the current rich editor nodes and keeps attachment references portable", () => {
    const content = normalizeTaskContent({
      type: "doc",
      content: [
        {
          type: "heading",
          attrs: { level: 1 },
          content: [{ type: "text", text: "Rich title" }],
        },
        {
          type: "paragraph",
          content: [
            {
              type: "mention-@",
              attrs: {
                id: "related-session",
                type: "session",
                label: "Related session",
              },
            },
          ],
        },
        {
          type: "paragraph",
          content: [
            {
              type: "text",
              text: "Related note",
              marks: [
                {
                  type: "link",
                  attrs: { href: "loofah://sessions/related-note" },
                },
              ],
            },
          ],
        },
        {
          type: "image",
          attrs: {
            attachmentId: "photo (1).png",
            src: "asset://localhost/private/photo.png",
            path: "/private/photo.png",
            alt: "Photo",
          },
        },
        {
          type: "fileAttachment",
          attrs: {
            attachmentId: "report.pdf",
            name: "Report",
            mimeType: "application/pdf",
            src: "file:///private/report.pdf",
            path: "/private/report.pdf",
          },
        },
        { type: "paragraph" },
      ],
    })!;
    const original = structuredClone(content);
    const saved = captureNoteEditorContent("note", content, []);
    expect(content).toEqual(original);
    expect(saved.markdown).toContain("related-session");
    expect(saved.markdown).toContain("related-note");
    expect(saved.markdown).toContain("attachments/photo%20%281%29.png");
    expect(saved.markdown).toContain("attachments/report.pdf");
    expect(saved.markdown).not.toContain("/private/");
    expect(saved.markdown).not.toContain("asset:");
    expect(saved.markdown).not.toContain("file:");
    expect(saved.title).toBe("Rich title");
    expect(saved.tasks).toEqual([]);
    const reopened = noteEditorContent("note", saved);
    expect(json2mdStrict(reopened.content)).toBe(saved.markdown);
  });

  it("assigns new identities only when opening untracked Markdown tasks and refuses unnormalized saves", () => {
    const input = { ...draft(), tasks: [] };
    const opened = noteEditorContent("note", input);
    const id = taskNodes(opened.content)[0].attrs?.taskId;
    expect(id).toEqual(expect.any(String));
    const saved = captureNoteEditorContent("note", opened.content, []);
    expect(saved.tasks[0].id).toBe(id);
    expect(
      taskNodes(noteEditorContent("note", saved).content)[0].attrs?.taskId,
    ).toBe(id);
    expect(() =>
      captureNoteEditorContent("note", md2json("- [ ] New task"), []),
    ).toThrow("normalized");
    const repeated = structuredClone(opened.content);
    repeated.content!.push(
      structuredClone(
        repeated.content!.find((node) => node.type === "taskList")!,
      ),
    );
    expect(() => captureNoteEditorContent("note", repeated, [])).toThrow(
      "normalized",
    );
  });

  it("preserves current legacy task-body fallback and rejects unsupported task state without dropping it", () => {
    const input = draft();
    input.tasks[0].body = "legacy body";
    const opened = noteEditorContent("note", input);
    expect(opened.tasks[0].body).toEqual([
      { type: "paragraph", content: [{ type: "text", text: "Follow up" }] },
    ]);
    const unknown = draft();
    unknown.tasks[0].status = "future-status";
    expect(() => noteEditorContent("note", unknown)).toThrow(
      "cannot safely edit",
    );
    expect(unknown.tasks).toHaveLength(1);
    unknown.tasks[0].status = "todo";
    unknown.tasks.push({ ...unknown.tasks[0] });
    expect(() => noteEditorContent("note", unknown)).toThrow("repeated task");
    const badBody = draft();
    badBody.markdown = "# Title";
    badBody.tasks[0].body = [{ type: "futureBlock" }];
    expect(() => noteEditorContent("note", badBody)).toThrow(
      "cannot safely edit",
    );
  });

  it("rejects task state from another note and retains legacy task mapping behavior", () => {
    const input = draft();
    const { content, tasks } = noteEditorContent("note", input);
    expect(() => captureNoteEditorContent("another", content, tasks)).toThrow(
      "different note",
    );
    const item = {
      ...input.tasks[0],
      source_id: "note",
      source_type: "session_raw_note",
      assignee: "",
      created_at: "",
      updated_at: "",
    };
    expect(taskItemToRecord(item)).toEqual(tasks[0]);
    expect(taskItemToRecord({ ...item, source_id: "" })).toBeNull();
    expect(taskItemToRecord({ ...item, status: "unknown" })).toBeNull();
  });

  it("fails explicitly for the current serializer's unsupported session and app-link nodes", () => {
    for (const node of [
      {
        type: "session",
        attrs: { sessionId: "related" },
        content: [{ type: "paragraph" }],
      },
      {
        type: "paragraph",
        content: [
          {
            type: "appLink",
            attrs: {
              provider: "github",
              url: "https://github.com/bart6114/loofah",
            },
          },
        ],
      },
    ]) {
      const content = { type: "doc", content: [node] };
      const before = structuredClone(content);
      expect(() => captureNoteEditorContent("note", content, [])).toThrow(
        "cannot be safely serialized",
      );
      expect(content).toEqual(before);
    }
  });
});

it.each([
  { type: "session_summary", id: "note" },
  { type: "enhanced_note", id: "manual-output" },
] as const)(
  "captures rich document task intent only for the supplied $type source",
  (source) => {
    const input = draft();
    const rendered = editorContent(source, input);
    expect(rendered.tasks[0]).toMatchObject({
      sourceType: source.type,
      sourceId: source.id,
      dueDate: "2027-01-01",
      status: "in_progress",
    });
    const node = taskNodes(rendered.content)[0];
    node.content![0].content![0].text = "Document task changed";
    const saved = captureEditorContent(
      source,
      rendered.content,
      rendered.tasks,
    );
    expect(saved.title).toBe("Title");
    expect(saved.tasks[0]).toMatchObject({
      id: "stable-task",
      text: "Document task changed",
      due_at: "2027-01-01",
      status: "in_progress",
    });
    expect(() =>
      captureEditorContent(
        { type: "enhanced_note", id: "foreign" },
        rendered.content,
        rendered.tasks,
      ),
    ).toThrow("different note");
  },
);
