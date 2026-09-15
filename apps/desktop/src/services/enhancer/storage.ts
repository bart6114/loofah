import {
  loadSessionContentSnapshot,
  type SessionContentSnapshot,
} from "~/session/content-queries";
import { id } from "~/shared/utils";
import { enqueueDatabaseWrite } from "~/shared/write-queue";
import { commands } from "~/types/tauri.gen";

export type EnhancerNote = SessionContentSnapshot["enhancedNotes"][number];

export function ensureSummaryDocument(
  sessionId: string,
): Promise<EnhancerNote> {
  return enqueueDatabaseWrite(`session:${sessionId}`, async () => {
    const snapshot = await loadSessionContentSnapshot(sessionId);
    if (!snapshot) {
      throw new Error(`Session ${sessionId} no longer exists`);
    }

    const existing = selectSummaryDocument(snapshot.enhancedNotes);
    if (existing) {
      return existing;
    }

    const noteId = id();
    const position =
      snapshot.enhancedNotes.reduce(
        (highest, note) => Math.max(highest, note.position),
        0,
      ) + 1;
    const result = await commands.sessionWriteEnhancedDoc({
      id: noteId,
      session_id: sessionId,
      kind: "summary",
      title: "Summary",
      template_id: "",
      sort_order: position,
      markdown: "",
    });
    if (result.status === "error") {
      throw new Error(
        `Failed to create summary document for session ${sessionId}: ${result.error}`,
      );
    }

    return {
      id: noteId,
      title: "Summary",
      markdown: "",
      content: "",
      contentFormat: "md",
      templateId: "",
      kind: "summary",
      position,
    };
  });
}

export function selectSummaryDocument(
  notes: EnhancerNote[],
): EnhancerNote | undefined {
  const sorted = [...notes].sort(
    (a, b) => a.position - b.position || a.id.localeCompare(b.id),
  );
  return sorted.find((note) => note.kind === "summary") ?? sorted[0];
}
