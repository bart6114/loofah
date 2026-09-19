import { enqueueDatabaseWrite } from "~/shared/write-queue";
import { ensureTag } from "~/tags/queries";
import { commands } from "~/types/tauri.gen";

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
  tagNames,
}: {
  sessionId: string;
  ownerUserId: string;
  note: SessionDocumentContentUpdate;
  tagNames: string[];
}): Promise<void> {
  return enqueueDatabaseWrite(`session:${sessionId}`, async () => {
    const normalizedTagNames = [...new Set(tagNames)].filter(Boolean);

    // File-first with the same staleness contract the old guarded SQL update had: a stale
    // `currentMarkdown` (reset/regenerate replaced the summary meanwhile) rejects and
    // nothing below runs. A missing doc file (session or doc deleted) rejects too,
    // replacing the old `expectedRowsAffected`/`EXISTS(sessions)` guards.
    const docWrite =
      note.id === sessionId
        ? await commands.sessionUpdateSummary(
            sessionId,
            note.nextMarkdown,
            note.currentMarkdown,
            true,
          )
        : await commands.sessionUpdateEnhancedDoc(sessionId, note.id, {
            markdown: note.nextMarkdown,
            reconcile_tasks: true,
            expected_markdown: note.currentMarkdown,
          });
    if (docWrite.status === "error") {
      throw new Error(
        `Failed to persist generated summary ${note.id}: ${docWrite.error}`,
      );
    }

    // `_meta.json` is the only tag store now (the SQL tag tables have no readers left).
    // Same additive semantics as the old tag/session_tags upserts: union the generated
    // tags into whatever the session already carries, sorted for stable file content.
    // The read-merge-write can't interleave with another tag writer: everything that
    // mutates this session serializes through the `session:<id>` queue key.
    if (normalizedTagNames.length > 0) {
      const sessionRead = await commands.sessionGet(sessionId);
      if (sessionRead.status === "error") {
        throw new Error(
          `Failed to read session ${sessionId} tags: ${sessionRead.error}`,
        );
      }
      const currentTags = sessionRead.data?.meta.tags ?? [];
      const mergedTags = [
        ...new Set([...currentTags, ...normalizedTagNames]),
      ].sort();

      const result = await commands.sessionUpdateMeta(sessionId, {
        tags: mergedTags,
      });
      if (result.status === "error") {
        throw new Error(
          `Failed to write tags into session ${sessionId} meta: ${result.error}`,
        );
      }

      // Best-effort registry sync: the vault-root `tags.json` feeds the typeahead,
      // but a registry failure must never fail the note write itself.
      for (const tagName of mergedTags) {
        void ensureTag(tagName).catch((error) => {
          console.error(
            "[content-mutations] failed to register tag in tags.json",
            tagName,
            error,
          );
        });
      }
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
