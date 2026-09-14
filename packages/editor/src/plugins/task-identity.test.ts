import { Node as PMNode } from "prosemirror-model";
import { EditorState } from "prosemirror-state";
import { describe, expect, it, vi } from "vitest";

import { schema } from "../note/schema";
import { taskIdentityPlugin } from "./task-identity";

describe("taskIdentityPlugin", () => {
  it("repairs duplicate task identities after unrelated document edits", () => {
    const duplicateAttrs = {
      status: "todo",
      checked: false,
      taskId: "task-1",
      taskItemId: "task-item-1",
    };
    let state = EditorState.create({
      schema,
      doc: schema.node("doc", null, [
        schema.node("paragraph", null, [schema.text("intro")]),
        schema.node("taskList", null, [
          schema.node("taskItem", duplicateAttrs, [schema.node("paragraph")]),
          schema.node("taskItem", duplicateAttrs, [schema.node("paragraph")]),
        ]),
      ]),
      plugins: [taskIdentityPlugin()],
    });

    state = state.applyTransaction(state.tr.insertText("!", 1)).state;

    const taskIds: string[] = [];
    const taskItemIds: string[] = [];
    state.doc.descendants((node) => {
      if (node.type.name !== "taskItem") {
        return true;
      }

      taskIds.push(node.attrs.taskId);
      taskItemIds.push(node.attrs.taskItemId);
      return false;
    });

    expect(new Set(taskIds).size).toBe(2);
    expect(new Set(taskItemIds).size).toBe(2);
  });
});

it("does not traverse the document for ordinary typing after initial validation", () => {
  const state = EditorState.create({
    schema,
    doc: schema.node(
      "doc",
      null,
      Array.from({ length: 10000 }, () =>
        schema.node("paragraph", null, [schema.text("text")]),
      ),
    ),
    plugins: [taskIdentityPlugin()],
  });
  const walk = vi.spyOn(PMNode.prototype, "descendants");
  try {
    const next = state.applyTransaction(state.tr.insertText("hello", 2)).state;
    expect(next.doc.firstChild!.textContent).toBe("thelloext");
    expect(walk).not.toHaveBeenCalled();
  } finally {
    walk.mockRestore();
  }
});

it("repairs pasted duplicates after a valid document and preserves IDs on typing", () => {
  const item = schema.node(
    "taskItem",
    { taskId: "task", taskItemId: "item", status: "todo", checked: false },
    [schema.node("paragraph", null, [schema.text("first")])],
  );
  let state = EditorState.create({
    schema,
    doc: schema.node("doc", null, [schema.node("taskList", null, [item])]),
    plugins: [taskIdentityPlugin()],
  });
  state = state.applyTransaction(
    state.tr.insert(state.doc.content.size - 1, item),
  ).state;
  const tasks = () => {
    const values: string[] = [];
    state.doc.descendants((node) => {
      if (node.type.name === "taskItem") values.push(node.attrs.taskId);
    });
    return values;
  };
  expect(new Set(tasks()).size).toBe(2);
  const ids = tasks();
  state = state.applyTransaction(state.tr.insertText("!", 3)).state;
  expect(tasks()).toEqual(ids);
});
