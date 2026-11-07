import { json2mdStrict, md2json } from "@hypr/editor/markdown";
import {
  normalizePortableAttachmentUrls,
  schema,
  type JSONContent,
} from "@hypr/editor/note";
import {
  extractTasksFromContent,
  hydrateTaskContent,
  normalizeTaskContent,
  type TaskRecord,
} from "@hypr/editor/tasks";

import { taskInputToRecord, taskRecordToInput } from "./task-records";

import {
  ensureFirstLineTitle,
  extractFirstLineTitle,
} from "~/session/title-content";
import { ApplicationRequestError } from "~/shared/application/errors";
import type { EditorText } from "~/shared/application/note-editor-draft";

export function noteEditorContent(sessionId: string, draft: EditorText) {
  const source = { type: "session_raw_note", id: sessionId };
  const records = draft.tasks.map((task) => taskInputToRecord(source, task));
  if (records.some((record) => record === null))
    throw unsupported(
      "This note contains task data the editor cannot safely edit",
    );
  const tasks = records as TaskRecord[];
  if (new Set(tasks.map((task) => task.taskId)).size !== tasks.length)
    throw unsupported("This note contains repeated task identities");
  const tasksById = new Map(tasks.map((task) => [task.taskId, task]));
  const content = normalizeTaskContent(
    hydrateTaskContent({
      content: ensureFirstLineTitle(md2json(draft.markdown), draft.title),
      sourceTasks: tasks,
      getTask: (id) => tasksById.get(id) ?? null,
    }),
  )!;
  try {
    schema.nodeFromJSON(content).check();
  } catch {
    throw unsupported(
      "This note contains content the editor cannot safely edit",
    );
  }
  return { content, tasks };
}

export function captureNoteEditorContent(
  sessionId: string,
  content: JSONContent,
  tasks: readonly TaskRecord[],
): EditorText {
  const source = { type: "session_raw_note", id: sessionId };
  if (
    tasks.some(
      (task) => task.sourceType !== source.type || task.sourceId !== source.id,
    )
  )
    throw unsupported("This editor task state belongs to a different note");
  const portable = normalizePortableAttachmentUrls(content);
  const seen = new Set<string>();
  const inspect = (node: JSONContent) => {
    if (node.type === "taskItem") {
      const id = node.attrs?.taskId;
      if (typeof id !== "string" || !id.trim() || seen.has(id))
        throw unsupported(
          "Task identities must be normalized before saving this editor",
        );
      seen.add(id);
    }
    node.content?.forEach(inspect);
  };
  inspect(portable);
  const previous = new Map(tasks.map((task) => [task.taskId, task]));
  let markdown: string;
  try {
    markdown = json2mdStrict(portable);
  } catch {
    throw unsupported(
      "This content cannot be safely serialized by the current editor",
    );
  }
  return {
    markdown,
    title: extractFirstLineTitle(portable) ?? "",
    tasks: extractTasksFromContent(portable, source, previous).map(
      taskRecordToInput,
    ),
  };
}

function unsupported(message: string) {
  return new ApplicationRequestError({
    code: "incompatible_version",
    message,
    request_id: null,
    retryable: false,
    current_revision: null,
    mutation_outcome: "not_committed",
  });
}
