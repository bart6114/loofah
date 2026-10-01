import { history, undo } from "prosemirror-history";
import {
  EditorState,
  Selection,
  TextSelection,
  type Transaction,
} from "prosemirror-state";
import type { EditorView } from "prosemirror-view";
import { describe, expect, it } from "vitest";

import { buildInputRules, buildKeymap } from "./keymap";
import { schema } from "./schema";
import { titleHeadingPlugin } from "./title-layout";

describe("buildInputRules", () => {
  it("creates an unchecked task item when typing [] followed by space", () => {
    const inputRules = buildInputRules();
    const doc = schema.node("doc", null, [
      schema.node("paragraph", null, [schema.text("[]")]),
    ]);
    let state = EditorState.create({
      schema,
      doc,
      selection: Selection.atEnd(doc),
      plugins: [inputRules],
    });

    const view = {
      composing: false,
      get state() {
        return state;
      },
      dispatch(tr: Transaction) {
        state = state.apply(tr);
      },
    } as Pick<EditorView, "composing" | "dispatch" | "state"> as EditorView;

    const handleTextInput = inputRules.props.handleTextInput as
      | ((
          view: EditorView,
          from: number,
          to: number,
          text: string,
          deflt: () => Transaction,
        ) => boolean | void)
      | undefined;

    const handled = handleTextInput?.(
      view,
      state.selection.from,
      state.selection.to,
      " ",
      () => state.tr.insertText(" ", state.selection.from, state.selection.to),
    );

    expect(handled).toBe(true);
    expect(state.doc.toJSON()).toMatchObject({
      type: "doc",
      content: [
        {
          type: "taskList",
          content: [
            {
              type: "taskItem",
              attrs: {
                status: "todo",
                checked: false,
                taskId: expect.any(String),
                taskItemId: expect.any(String),
              },
              content: [{ type: "paragraph" }],
            },
          ],
        },
      ],
    });
  });

  it("replaces typed arrow shorthand with an arrow symbol", () => {
    const doc = schema.node("doc", null, [
      schema.node("paragraph", null, [schema.text("-")]),
    ]);
    const { handled, state } = runTextInput(doc, ">");

    expect(handled).toBe(true);
    expect(state.doc.toJSON()).toEqual({
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [{ type: "text", text: "→" }],
        },
      ],
    });
  });

  it("replaces typed copyright shorthand with a copyright symbol", () => {
    const doc = schema.node("doc", null, [
      schema.node("paragraph", null, [schema.text("(c")]),
    ]);
    const { handled, state } = runTextInput(doc, ")");

    expect(handled).toBe(true);
    expect(state.doc.toJSON()).toEqual({
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [{ type: "text", text: "©" }],
        },
      ],
    });
  });

  it("keeps replacement shorthands literal in code blocks", () => {
    const doc = schema.node("doc", null, [
      schema.node("codeBlock", null, [schema.text("-")]),
    ]);
    const { handled, state } = runTextInput(doc, ">");

    expect(handled).not.toBe(true);
    expect(state.doc.toJSON()).toEqual(doc.toJSON());
  });
});

describe("buildKeymap", () => {
  it("does not handle Shift+Enter as a hard break shortcut", () => {
    const doc = schema.node("doc", null, [
      schema.node("paragraph", null, [schema.text("hello")]),
    ]);
    const { handled, state } = runKeyDownAtEnd(doc, "Enter", {
      shiftKey: true,
    });

    expect(handled).not.toBe(true);
    expect(state.doc.toJSON()).toEqual(doc.toJSON());
  });

  it("merges task item text backward without changing the list structure", () => {
    const doc = schema.node("doc", null, [
      schema.node("taskList", null, [
        schema.node(
          "taskItem",
          {
            status: "todo",
            checked: false,
            taskId: "task-1",
            taskItemId: "task-item-1",
          },
          [schema.node("paragraph", null, [schema.text("one")])],
        ),
        schema.node(
          "taskItem",
          {
            status: "todo",
            checked: false,
            taskId: "task-2",
            taskItemId: "task-item-2",
          },
          [schema.node("paragraph", null, [schema.text("two")])],
        ),
      ]),
    ]);
    const { state } = runBackspaceAtTextStart(doc, "two");

    expect(state.doc.toJSON()).toEqual({
      type: "doc",
      content: [
        {
          type: "taskList",
          content: [
            {
              type: "taskItem",
              attrs: {
                status: "todo",
                checked: false,
                taskId: "task-1",
                taskItemId: "task-item-1",
              },
              content: [
                {
                  type: "paragraph",
                  content: [{ type: "text", text: "onetwo" }],
                },
              ],
            },
          ],
        },
      ],
    });
  });

  it("keeps the first task item separate from a previous bullet list", () => {
    const doc = schema.node("doc", null, [
      schema.node("bulletList", null, [
        schema.node("listItem", null, [
          schema.node("paragraph", null, [schema.text("one")]),
        ]),
      ]),
      schema.node("taskList", null, [
        schema.node(
          "taskItem",
          {
            status: "todo",
            checked: false,
            taskId: "task-1",
            taskItemId: "task-item-1",
          },
          [schema.node("paragraph", null, [schema.text("two")])],
        ),
      ]),
    ]);
    const { state } = runBackspaceAtTextStart(doc, "two");

    expect(state.doc.toJSON()).toEqual(doc.toJSON());
  });

  it("joins later task item paragraphs within the same task item", () => {
    const doc = schema.node("doc", null, [
      schema.node("taskList", null, [
        schema.node(
          "taskItem",
          {
            status: "todo",
            checked: false,
            taskId: "task-1",
            taskItemId: "task-item-1",
          },
          [schema.node("paragraph", null, [schema.text("one")])],
        ),
        schema.node(
          "taskItem",
          {
            status: "todo",
            checked: false,
            taskId: "task-2",
            taskItemId: "task-item-2",
          },
          [
            schema.node("paragraph", null, [schema.text("two")]),
            schema.node("paragraph", null, [schema.text("three")]),
          ],
        ),
      ]),
    ]);
    const { state } = runBackspaceAtTextStart(doc, "three", true);

    expect(state.doc.toJSON()).toEqual({
      type: "doc",
      content: [
        {
          type: "taskList",
          content: [
            {
              type: "taskItem",
              attrs: {
                status: "todo",
                checked: false,
                taskId: "task-1",
                taskItemId: "task-item-1",
              },
              content: [
                {
                  type: "paragraph",
                  content: [{ type: "text", text: "one" }],
                },
              ],
            },
            {
              type: "taskItem",
              attrs: {
                status: "todo",
                checked: false,
                taskId: "task-2",
                taskItemId: "task-item-2",
              },
              content: [
                {
                  type: "paragraph",
                  content: [{ type: "text", text: "twothree" }],
                },
              ],
            },
          ],
        },
      ],
    });
  });
});

describe("title heading keymap", () => {
  const titleDoc = () =>
    schema.node("doc", null, [
      schema.node("heading", { level: 1 }, [schema.text("Planning")]),
      schema.node("paragraph", null, [schema.text("Follow up")]),
    ]);

  it.each([
    ["Backspace", 1, {}],
    ["Backspace", 11, {}],
    ["Backspace", 11, { shiftKey: true }],
    ["Delete", 9, {}],
    ["Delete", 9, { ctrlKey: true }],
  ])("protects the title boundary for %s at %i", (key, pos, init) => {
    const doc = titleDoc();
    const editor = createTitleEditor(doc, pos);

    expect(editor.press(key, init)).toBe(true);
    expect(editor.state.doc.toJSON()).toEqual(doc.toJSON());
    expect(editor.dispatchCount).toBe(0);
  });

  it("keeps an empty title heading when Backspace is pressed at its start", () => {
    const doc = schema.node("doc", null, [
      schema.node("heading", { level: 1 }),
      schema.node("paragraph"),
    ]);
    const editor = createTitleEditor(doc, 1);

    expect(editor.press("Backspace")).toBe(true);
    expect(editor.state.doc.toJSON()).toEqual(doc.toJSON());
    expect(editor.dispatchCount).toBe(0);
  });

  it("turns Enter in an empty title into a body paragraph", () => {
    const doc = schema.node("doc", null, [
      schema.node("heading", { level: 1 }),
      schema.node("paragraph", null, [schema.text("Body")]),
    ]);
    const editor = createTitleEditor(doc, 1);

    expect(editor.press("Enter")).toBe(true);
    expect(editor.state.doc.child(0).type).toBe(schema.nodes.heading);
    expect(editor.state.doc.child(0).textContent).toBe("");
    expect(editor.state.doc.child(1).type).toBe(schema.nodes.paragraph);
    expect(editor.state.doc.child(2).textContent).toBe("Body");
  });

  it.each([
    [1, "", "Planning"],
    [5, "Plan", "ning"],
    [9, "Planning", ""],
  ])("splits the title at %i into a body paragraph", (pos, title, suffix) => {
    const doc = titleDoc();
    const editor = createTitleEditor(doc, pos);

    expect(editor.press("Enter")).toBe(true);
    expect(editor.state.doc.child(0).type).toBe(schema.nodes.heading);
    expect(editor.state.doc.child(0).attrs.level).toBe(1);
    expect(editor.state.doc.child(0).textContent).toBe(title);
    expect(editor.state.doc.child(1).type).toBe(schema.nodes.paragraph);
    expect(editor.state.doc.child(1).textContent).toBe(suffix);
    expect(editor.state.doc.child(2).textContent).toBe("Follow up");
    expect(editor.state.selection.from).toBe(
      editor.state.doc.child(0).nodeSize + 1,
    );

    expect(editor.undo()).toBe(true);
    expect(editor.state.doc.toJSON()).toEqual(doc.toJSON());
  });

  it("replaces a selection inside the title with a paragraph split", () => {
    const doc = titleDoc();
    const editor = createTitleEditor(doc, 3, 6);

    expect(editor.press("Enter")).toBe(true);
    expect(editor.state.doc.child(0).type).toBe(schema.nodes.heading);
    expect(editor.state.doc.child(0).textContent).toBe("Pl");
    expect(editor.state.doc.child(1).type).toBe(schema.nodes.paragraph);
    expect(editor.state.doc.child(1).textContent).toBe("ing");
    expect(editor.undo()).toBe(true);
    expect(editor.state.doc.toJSON()).toEqual(doc.toJSON());
  });

  it("keeps heading conversion and ordinary body joins", () => {
    const doc = schema.node("doc", null, [
      schema.node("heading", { level: 1 }, [schema.text("Planning")]),
      schema.node("heading", { level: 2 }, [schema.text("Section")]),
      schema.node("paragraph", null, [schema.text("One")]),
      schema.node("paragraph", null, [schema.text("Two")]),
    ]);
    const editor = createTitleEditor(doc, doc.child(0).nodeSize + 1);

    expect(editor.press("Backspace")).toBe(true);
    expect(editor.state.doc.child(1).type).toBe(schema.nodes.paragraph);
    expect(editor.state.doc.child(1).textContent).toBe("Section");

    editor.select(
      editor.state.doc.child(0).nodeSize +
        editor.state.doc.child(1).nodeSize +
        editor.state.doc.child(2).nodeSize +
        1,
    );
    expect(editor.press("Backspace")).toBe(true);
    expect(editor.state.doc.child(2).textContent).toBe("OneTwo");
  });

  it("converts a first body code block to a paragraph before the boundary guard", () => {
    const doc = schema.node("doc", null, [
      schema.node("heading", { level: 1 }, [schema.text("Planning")]),
      schema.node("codeBlock", null, [schema.text("const value = 1")]),
    ]);
    const editor = createTitleEditor(doc, doc.child(0).nodeSize + 1);

    expect(editor.press("Backspace")).toBe(true);
    expect(editor.state.doc.child(1).type).toBe(schema.nodes.paragraph);
    expect(editor.state.doc.child(1).textContent).toBe("const value = 1");
  });

  it("lets an intentional selection delete across the title boundary and undo", () => {
    const doc = titleDoc();
    const editor = createTitleEditor(doc, 5, 12);

    expect(editor.press("Backspace")).toBe(true);
    expect(editor.state.doc.toJSON()).not.toEqual(doc.toJSON());
    expect(editor.undo()).toBe(true);
    expect(editor.state.doc.toJSON()).toEqual(doc.toJSON());
  });

  it("retains generic keymap joins when title enforcement is off", () => {
    const doc = titleDoc();
    const editor = createTitleEditor(doc, 11, undefined, false);

    expect(editor.press("Backspace")).toBe(true);
    expect(editor.state.doc.childCount).toBe(1);
    expect(editor.state.doc.firstChild?.textContent).toBe("PlanningFollow up");
  });

  it("retains generic heading splits when title enforcement is off", () => {
    const editor = createTitleEditor(titleDoc(), 5, undefined, false);

    expect(editor.press("Enter")).toBe(true);
    expect(editor.state.doc.child(1).type).toBe(schema.nodes.heading);
    expect(editor.state.doc.child(1).attrs.level).toBe(1);
  });

  it("leaves ordinary character deletion to the editor", () => {
    const editor = createTitleEditor(titleDoc(), 8);

    expect(editor.press("Backspace")).toBe(false);

    editor.select(editor.state.doc.child(0).nodeSize + 2);
    expect(editor.press("Backspace")).toBe(false);
    expect(editor.dispatchCount).toBe(0);
  });
});

function createTitleEditor(
  doc: ReturnType<typeof schema.node>,
  from: number,
  to?: number,
  enforceTitleHeading = true,
) {
  const keymap = buildKeymap(undefined, enforceTitleHeading);
  let state = EditorState.create({
    schema,
    doc,
    selection: TextSelection.create(doc, from, to),
    plugins: [history(), titleHeadingPlugin(), keymap],
  });
  let dispatchCount = 0;
  const view = {
    get state() {
      return state;
    },
    dispatch(tr: Transaction) {
      dispatchCount++;
      state = state.apply(tr);
    },
    endOfTextblock: (direction: string) =>
      direction === "backward"
        ? state.selection.$from.parentOffset === 0
        : state.selection.$from.parentOffset ===
          state.selection.$from.parent.content.size,
  } as Pick<EditorView, "dispatch" | "endOfTextblock" | "state"> as EditorView;

  return {
    get state() {
      return state;
    },
    get dispatchCount() {
      return dispatchCount;
    },
    press(key: string, init?: KeyboardEventInit) {
      return keymap.props.handleKeyDown?.(
        view,
        new KeyboardEvent("keydown", { key, ...init }),
      );
    },
    select(pos: number) {
      state = state.apply(
        state.tr.setSelection(TextSelection.create(state.doc, pos)),
      );
    },
    undo() {
      return undo(state, (tr) => {
        state = state.apply(tr);
      });
    },
  };
}

function runKeyDownAtEnd(
  doc: ReturnType<typeof schema.node>,
  key: string,
  init?: KeyboardEventInit,
) {
  const keymap = buildKeymap();
  let state = EditorState.create({
    schema,
    doc,
    selection: Selection.atEnd(doc),
    plugins: [keymap],
  });
  const view = {
    get state() {
      return state;
    },
    dispatch(tr: Transaction) {
      state = state.apply(tr);
    },
    endOfTextblock: () => false,
  } as Pick<EditorView, "dispatch" | "endOfTextblock" | "state"> as EditorView;
  const handleKeyDown = keymap.props.handleKeyDown;

  const handled = handleKeyDown?.(
    view,
    new KeyboardEvent("keydown", {
      key,
      ...init,
    }),
  );

  return { handled, state };
}

function runBackspaceAtTextStart(
  doc: ReturnType<typeof schema.node>,
  text: string,
  isEndOfTextblock = false,
) {
  const keymap = buildKeymap();
  const textPos = getTextStartPos(doc, text);
  let state = EditorState.create({
    schema,
    doc,
    selection: TextSelection.create(doc, textPos),
    plugins: [keymap],
  });
  const view = {
    get state() {
      return state;
    },
    dispatch(tr: Transaction) {
      state = state.apply(tr);
    },
    endOfTextblock: () => isEndOfTextblock,
  } as Pick<EditorView, "dispatch" | "endOfTextblock" | "state"> as EditorView;
  const handleKeyDown = keymap.props.handleKeyDown;

  const handled = handleKeyDown?.(
    view,
    new KeyboardEvent("keydown", { key: "Backspace" }),
  );

  expect(handled).toBe(true);
  return { state };
}

function runTextInput(doc: ReturnType<typeof schema.node>, text: string) {
  const inputRules = buildInputRules();
  let state = EditorState.create({
    schema,
    doc,
    selection: Selection.atEnd(doc),
    plugins: [inputRules],
  });

  const view = {
    composing: false,
    get state() {
      return state;
    },
    dispatch(tr: Transaction) {
      state = state.apply(tr);
    },
  } as Pick<EditorView, "composing" | "dispatch" | "state"> as EditorView;

  const handleTextInput = inputRules.props.handleTextInput as
    | ((
        view: EditorView,
        from: number,
        to: number,
        text: string,
        deflt: () => Transaction,
      ) => boolean | void)
    | undefined;

  const handled = handleTextInput?.(
    view,
    state.selection.from,
    state.selection.to,
    text,
    () => state.tr.insertText(text, state.selection.from, state.selection.to),
  );

  return { handled, state };
}

function getTextStartPos(doc: ReturnType<typeof schema.node>, text: string) {
  let textPos = -1;

  doc.descendants((node, pos) => {
    if (node.isText && node.text === text) {
      textPos = pos;
      return false;
    }

    return undefined;
  });

  if (textPos === -1) {
    throw new Error(`Missing text node: ${text}`);
  }

  return textPos;
}
