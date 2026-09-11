import { useMemo } from "react";

import {
  useEnhancedNote as useSqliteEnhancedNote,
  useEnhancedNoteRecords,
} from "~/session/queries";

export function useEnhancedNotes(sessionId: string) {
  const notes = useEnhancedNoteRecords(sessionId);
  return useMemo(() => notes.map((note) => note.id), [notes]);
}

export function useEnhancedNote(enhancedNoteId: string) {
  const note = useSqliteEnhancedNote(enhancedNoteId);

  return {
    title: note?.title,
    content: note?.content,
    position: note?.position,
    templateId: note?.templateId,
  };
}
