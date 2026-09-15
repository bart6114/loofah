import type { EditorView } from "~/store/zustand/tabs/schema";

export function computeCurrentNoteTab(
  tabView: EditorView | null,
  isLiveSessionActive: boolean,
  enhancedNoteIds: readonly string[],
  canShowTranscript: boolean,
  defaultEnhancedNoteId: string | null,
): EditorView {
  const firstEnhancedNoteId = enhancedNoteIds[0];
  const hasEnhancedNote = (id: string) => enhancedNoteIds.includes(id);

  if (tabView?.type === "summary") {
    return firstEnhancedNoteId
      ? { type: "enhanced", id: firstEnhancedNoteId }
      : tabView;
  }

  if (isLiveSessionActive) {
    if (tabView?.type === "raw") {
      return tabView;
    }
    if (tabView?.type === "transcript" && canShowTranscript) {
      return tabView;
    }
    if (tabView?.type === "enhanced" && hasEnhancedNote(tabView.id)) {
      return tabView;
    }
    if (tabView?.type === "attachments") {
      return tabView;
    }
    return { type: "raw" };
  }

  if (tabView) {
    if (tabView.type === "raw") {
      return tabView;
    }
    if (tabView.type === "enhanced") {
      return hasEnhancedNote(tabView.id)
        ? tabView
        : firstEnhancedNoteId
          ? { type: "enhanced", id: firstEnhancedNoteId }
          : { type: "raw" };
    }
    if (tabView.type === "transcript" && canShowTranscript) {
      return tabView;
    }
    if (tabView.type === "attachments") {
      return tabView;
    }

    return { type: "raw" };
  }

  if (
    defaultEnhancedNoteId &&
    enhancedNoteIds.includes(defaultEnhancedNoteId)
  ) {
    return { type: "enhanced", id: defaultEnhancedNoteId };
  }

  return { type: "raw" };
}
