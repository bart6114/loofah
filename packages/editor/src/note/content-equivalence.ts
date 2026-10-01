import { json2mdStrict } from "../markdown";
import { normalizeTaskStatus } from "../tasks";
import type { JSONContent } from "./index";
import { normalizePortableAttachmentUrls } from "./portable-attachments";

export function areEquivalentEditorContents(
  left: JSONContent,
  right: JSONContent,
): boolean {
  if (left === right || JSON.stringify(left) === JSON.stringify(right)) {
    return true;
  }

  try {
    return (
      json2mdStrict(normalizePortableAttachmentUrls(left)) ===
        json2mdStrict(normalizePortableAttachmentUrls(right)) &&
      JSON.stringify(taskStatuses(left)) === JSON.stringify(taskStatuses(right))
    );
  } catch {
    return false;
  }
}

function taskStatuses(content: JSONContent): string[] {
  const statuses: string[] = [];
  const visit = (node: JSONContent) => {
    if (node.type === "taskItem") {
      // Markdown and hydration recreate task identities, but in-progress is
      // meaningful state that an unchecked Markdown checkbox cannot represent.
      statuses.push(
        normalizeTaskStatus(node.attrs?.status, node.attrs?.checked),
      );
    }
    node.content?.forEach(visit);
  };
  visit(content);
  return statuses;
}
