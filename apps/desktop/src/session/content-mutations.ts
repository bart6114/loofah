import { enqueueDatabaseWrite } from "~/shared/write-queue";
import { commands, type ScoredTagSuggestion } from "~/types/tauri.gen";

// Compare-and-swap rejects generated content if the summary changed while AI was running.
export type SessionDocumentContentUpdate = {
  id: string;
  currentMarkdown: string;
  nextMarkdown: string;
};

export function persistGeneratedEnhancedNote({
  sessionId,
  ownerUserId: _ownerUserId,
  note,
  suggestedTags,
  signal,
}: {
  sessionId: string;
  ownerUserId: string;
  note: SessionDocumentContentUpdate;
  suggestedTags?: ScoredTagSuggestion[];
  signal?: AbortSignal;
}): Promise<void> {
  return enqueueDatabaseWrite(`session:${sessionId}`, async () => {
    signal?.throwIfAborted();
    // The Rust write applies suggestions only after the summary passes its stale-content guard.
    const docWrite =
      note.id === sessionId
        ? await commands.sessionUpdateSummary(
            sessionId,
            note.nextMarkdown,
            note.currentMarkdown,
            true,
            suggestedTags ?? null,
          )
        : await commands.sessionUpdateEnhancedDoc(sessionId, note.id, {
            markdown: note.nextMarkdown,
            reconcile_tasks: true,
            suggested_tags: suggestedTags,
            expected_markdown: note.currentMarkdown,
          });
    if (docWrite.status === "error") {
      throw new Error(
        `Failed to persist generated summary ${note.id}: ${docWrite.error}`,
      );
    }
  });
}

// `documents` here is summary/template_output enhanced notes only -- the raw note is stamped
// separately, file-first, by `applyGeneratedNoteTitle` in title-success.ts (it reads/writes
// through session_read_note/session_write_note, never raw SQL, since the editor reads the file
// as of Task 9's file-canonical note-load path).
export function applyGeneratedSessionTitle({
  sessionId,
  currentTitle,
  nextTitle,
  documents,
}: {
  sessionId: string;
  currentTitle: string;
  nextTitle: string;
  documents: SessionDocumentContentUpdate[];
}): Promise<void> {
  return enqueueDatabaseWrite(`session:${sessionId}`, async () => {
    // Same compare-and-swap the old single-transaction title update gave us, kept honest by
    // the write queue: everything that mutates this session's title serializes through the
    // `session:<id>` queue key, so check-then-write can't interleave with a user edit. A
    // stale generation (user renamed meanwhile) must apply nothing at all.
    const sessionRead = await commands.sessionGet(sessionId);
    if (sessionRead.status === "error") {
      throw new Error(
        `Failed to read session ${sessionId} title: ${sessionRead.error}`,
      );
    }
    const session = sessionRead.data;
    if (!session || session.meta.title !== currentTitle) {
      throw new Error(
        `[content-mutations] session title changed while generating; not applying "${nextTitle}"`,
      );
    }

    // Documents first: a stale document guard (the store's "conflict:" CAS rejection, the
    // file-era equivalent of the old expectedRowsAffected rollback) throws here and the
    // store-canonical title write below never happens.
    for (const document of documents) {
      const docWrite =
        document.id === sessionId
          ? await commands.sessionUpdateSummary(
              sessionId,
              document.nextMarkdown,
              document.currentMarkdown,
              false,
              null,
            )
          : await commands.sessionUpdateEnhancedDoc(sessionId, document.id, {
              markdown: document.nextMarkdown,
              expected_markdown: document.currentMarkdown,
            });
      if (docWrite.status === "error") {
        throw new Error(
          `Failed to stamp title into summary ${document.id}: ${docWrite.error}`,
        );
      }
    }

    // Title last, through the store (file-first + SQL dual-write): `_meta.json` is canonical
    // for session meta, so this must never be a raw sessions-table update.
    const result = await commands.sessionUpdateMeta(sessionId, {
      title: nextTitle,
    });
    if (result.status === "error") {
      throw new Error(
        `Failed to update session ${sessionId} title: ${result.error}`,
      );
    }
  });
}
