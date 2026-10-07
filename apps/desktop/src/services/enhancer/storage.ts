import {
  loadSessionContentSnapshot,
  type SessionContentSnapshot,
} from "~/session/content-queries";
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

    const result = await commands.sessionEnsureSummary(sessionId);
    if (result.status === "error") {
      throw new Error(
        `Failed to create summary document for session ${sessionId}: ${result.error}`,
      );
    }

    return {
      id: sessionId,
      title: "Summary",
      markdown: result.data,
      content: result.data,
      contentFormat: "md",
      templateId: "",
      kind: "summary",
      position: 0,
    };
  });
}

export function selectSummaryDocument(
  notes: EnhancerNote[],
): EnhancerNote | undefined {
  const sorted = [...notes].sort(
    (a, b) => a.position - b.position || a.id.localeCompare(b.id),
  );
  return sorted.find((note) => note.kind === "summary");
}
