import type { JSONContent } from "@hypr/editor/note";
import type { TaskRecord, TaskSource } from "@hypr/editor/tasks";

import type { TaskInput, TaskItem } from "~/shared/application/protocol.gen";

export function taskRecordToInput(task: TaskRecord): TaskInput {
  return {
    id: task.taskId,
    source_order: task.sourceOrder,
    status: task.status,
    text: task.textPreview,
    body: task.body as TaskInput["body"],
    due_at: task.dueDate ?? "",
  };
}

export function taskItemToRecord(item: TaskItem): TaskRecord | null {
  return taskInputToRecord(
    { type: item.source_type, id: item.source_id },
    item,
  );
}

export function taskInputToRecord(
  source: TaskSource,
  item: TaskInput,
): TaskRecord | null {
  const sourceOrder = Number(item.source_order);
  if (
    !item.id ||
    !source.id ||
    !source.type ||
    !Number.isFinite(sourceOrder) ||
    (item.status !== "todo" &&
      item.status !== "in_progress" &&
      item.status !== "done")
  ) {
    return null;
  }

  const body = parseTaskBody(item.body, item.text);
  return {
    taskId: item.id,
    sourceId: source.id,
    sourceType: source.type,
    sourceOrder,
    status: item.status,
    textPreview: item.text || getTextPreview(body),
    body,
    dueDate: item.due_at || undefined,
  };
}

function parseTaskBody(body: unknown, legacyText: unknown): JSONContent[] {
  if (Array.isArray(body)) {
    return body as JSONContent[];
  }

  if (typeof legacyText === "string" && legacyText) {
    return [
      {
        type: "paragraph",
        content: [{ type: "text", text: legacyText }],
      },
    ];
  }

  return [{ type: "paragraph" }];
}

function getTextPreview(body: JSONContent[]): string {
  const firstParagraph = body.find((node) => node.type === "paragraph");
  return getNodeText(firstParagraph).trim();
}

function getNodeText(node: JSONContent | undefined): string {
  if (!node) {
    return "";
  }

  if (typeof node.text === "string") {
    return node.text;
  }

  return (node.content ?? []).map((child) => getNodeText(child)).join(" ");
}
