import type { Node as PMNode } from "prosemirror-model";
import { Plugin, PluginKey, type Transaction } from "prosemirror-state";

import { createTaskId, createTaskItemId } from "../tasks";

export function taskIdentityPlugin() {
  const key = new PluginKey<boolean>("taskIdentityValidation");
  return new Plugin({
    key,
    state: {
      init: (_, state) => hasInvalidTaskIdentity(state.doc),
      apply: (transaction, needsValidation) =>
        transaction.getMeta(key) === false
          ? false
          : needsValidation || taskIdentitiesChanged(transaction),
    },
    appendTransaction(transactions, _oldState, newState) {
      if (!transactions.some((transaction) => transaction.docChanged)) {
        return null;
      }
      if (!key.getState(newState)) return null;

      const seenTaskIds = new Set<string>();
      const seenTaskItemIds = new Set<string>();
      const updates: {
        pos: number;
        taskId: string;
        taskItemId: string;
      }[] = [];

      newState.doc.descendants((node, pos) => {
        if (node.type.name !== "taskItem") {
          return;
        }

        let taskId =
          typeof node.attrs.taskId === "string" && node.attrs.taskId.trim()
            ? node.attrs.taskId
            : "";

        while (!taskId || seenTaskIds.has(taskId)) {
          taskId = createTaskId();
        }

        let taskItemId =
          typeof node.attrs.taskItemId === "string" &&
          node.attrs.taskItemId.trim()
            ? node.attrs.taskItemId
            : "";

        while (!taskItemId || seenTaskItemIds.has(taskItemId)) {
          taskItemId = createTaskItemId();
        }

        seenTaskIds.add(taskId);
        seenTaskItemIds.add(taskItemId);

        if (
          node.attrs.taskId !== taskId ||
          node.attrs.taskItemId !== taskItemId
        ) {
          updates.push({ pos, taskId, taskItemId });
        }
      });

      let tr = newState.tr.setMeta(key, false);
      updates.forEach(({ pos, taskId, taskItemId }) => {
        const node = tr.doc.nodeAt(pos);
        if (!node) {
          return;
        }

        tr = tr.setNodeMarkup(
          pos,
          undefined,
          { ...node.attrs, taskId, taskItemId },
          node.marks,
        );
      });

      return tr;
    },
  });
}

function hasInvalidTaskIdentity(doc: PMNode) {
  const seenTaskIds = new Set<string>();
  const seenTaskItemIds = new Set<string>();
  let invalid = false;

  doc.descendants((node) => {
    if (invalid || node.type.name !== "taskItem") {
      return !invalid;
    }

    const taskId =
      typeof node.attrs.taskId === "string" && node.attrs.taskId.trim()
        ? node.attrs.taskId
        : "";
    const taskItemId =
      typeof node.attrs.taskItemId === "string" && node.attrs.taskItemId.trim()
        ? node.attrs.taskItemId
        : "";

    invalid =
      !taskId ||
      !taskItemId ||
      seenTaskIds.has(taskId) ||
      seenTaskItemIds.has(taskItemId);
    seenTaskIds.add(taskId);
    seenTaskItemIds.add(taskItemId);

    return !invalid;
  });

  return invalid;
}

function taskIdentitiesChanged(transaction: Transaction): boolean {
  return transaction.steps.some((step, index) => {
    const before = transaction.docs[index]!;
    const after = transaction.docs[index + 1] ?? transaction.doc;
    let changed = false;
    let hasRanges = false;
    step.getMap().forEach((oldStart, oldEnd, newStart, newEnd) => {
      hasRanges = true;
      if (
        JSON.stringify(identitiesInRange(before, oldStart, oldEnd)) !==
        JSON.stringify(identitiesInRange(after, newStart, newEnd))
      )
        changed = true;
    });
    // Attribute steps have empty maps; checking their target also handles task
    // identity edits without treating text changes as identity changes.
    if (!hasRanges) {
      const pos = (step as unknown as { pos?: number }).pos;
      if (typeof pos === "number") {
        changed =
          JSON.stringify(identitiesInRange(before, pos, pos)) !==
          JSON.stringify(identitiesInRange(after, pos, pos));
      }
    }
    return changed;
  });
}

function identitiesInRange(doc: PMNode, from: number, to: number) {
  const identities = new Map<number, unknown>();
  const add = (node: PMNode, pos: number) => {
    if (node.type.name === "taskItem")
      identities.set(pos, [node.attrs.taskId, node.attrs.taskItemId]);
  };
  for (const pos of [from, to]) {
    const resolved = doc.resolve(pos);
    for (let depth = 1; depth <= resolved.depth; depth++)
      add(resolved.node(depth), resolved.before(depth));
    if (resolved.nodeAfter) add(resolved.nodeAfter, pos);
  }
  if (to > from)
    doc.nodesBetween(from, to, (node, pos) => {
      add(node, pos);
    });
  return [...identities.values()];
}
