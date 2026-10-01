import { useQueryClient } from "@tanstack/react-query";
import { useCallback } from "react";

import {
  json2md,
  json2mdStrict,
  md2json,
  parseJsonContent,
} from "@hypr/editor/markdown";
import { useTaskStorageOptional } from "@hypr/editor/task-storage";
import { hydrateTaskContent } from "@hypr/editor/tasks";

import { waitForPendingSoftDelete } from "~/session/pending-soft-deletes";
import { ensureFirstLineTitle } from "~/session/title-content";
import { useIndexQuery } from "~/shared/index-query";
import { DEFAULT_USER_ID, id } from "~/shared/utils";
import { enqueueDatabaseWrite } from "~/shared/write-queue";
import type { DeletedSessionData } from "~/store/zustand/undo-delete";
import {
  commands,
  type EnhancedDoc,
  type SessionMetaPatch,
  type SessionRecord as StoreSessionRecord,
  type TagSuggestionState,
} from "~/types/tauri.gen";

export type SessionRecord = {
  id: string;
  user_id: string;
  created_at: string;
  folder_id: string;
  title: string;
  raw_md: string;
  tags: string[];
  author: string | null;
  tag_suggestions: TagSuggestionState | null;
};

// Note content is saved together with its title through sessionSaveNote.
export type SessionChanges = Partial<
  Pick<SessionRecord, "created_at" | "folder_id" | "title" | "tags">
>;

export type SessionSummaryRecord = {
  id: string;
  title: string;
  created_at: string;
};

export type EnhancedNoteRecord = {
  id: string;
  sessionId: string;
  title: string;
  content: string;
  templateId: string;
  position: number;
};

const EMPTY_ENHANCED_NOTES: EnhancedNoteRecord[] = [];
const EMPTY_SESSION_SUMMARIES: SessionSummaryRecord[] = [];

export function useSession(sessionId: string): SessionRecord | null {
  const taskStorage = useTaskStorageOptional();
  const { data = null } = useIndexQuery({
    // Editable content revalidates on mount in case a write or watcher event is
    // still in flight when the user switches back to the note.
    refetchOnMount: "always",
    entity: ["sessions", "tasks"],
    ids: [sessionId],
    queryKey: ["session", sessionId],
    queryFn: async () => {
      const result = await commands.sessionGet(sessionId);
      if (result.status === "error") {
        throw new Error(result.error);
      }
      if (!result.data) return null;
      const note = mapSessionRecord(result.data);
      if (taskStorage && note.raw_md) {
        const source = { type: "session_raw_note", id: sessionId };
        const loadedTasks = await taskStorage.loadSource?.(source);
        note.raw_md = JSON.stringify(
          hydrateTaskContent({
            content: JSON.parse(note.raw_md),
            sourceTasks: loadedTasks ?? taskStorage.getTasksForSource(source),
            getTask: (id) =>
              loadedTasks?.find((task) => task.taskId === id) ??
              taskStorage.getTask(id),
          }),
        );
      }
      return note;
    },
    enabled: Boolean(sessionId),
  });
  return sessionId ? data : null;
}

/**
 * Note content for the editor, as the stringified prosemirror doc consumers expect.
 * Returns `null` while the session itself hasn't loaded -- callers should keep showing a
 * loading state for that, same as `useSession` returning `null`.
 *
 * This is `useSession`'s `raw_md` and nothing else, deliberately: the index entry's
 * `note_markdown` *is* `sessions/<id>/notes.md` (seeded by the rescan's own `read_note`,
 * kept current by `session_write_note`'s write-through and by the vault watcher's
 * `refresh_session` on external edits), and it rides the `sessions` half of the
 * `index-changed` bus. A second, separately-cached `session_read_note` here used to shadow
 * it; nothing ever invalidated that cache, so a remount inside the cache window (tab switch,
 * second window, an Obsidian edit) handed the editor pre-edit content that the next
 * keystroke's `persistChange` then wrote back over `notes.md`.
 *
 * NoteEditor preserves local edits until saving and reloading confirms their persistence,
 * and defers external changes while the editor is focused.
 */
export function useSessionRawMd(sessionId: string): string | null {
  return useSession(sessionId)?.raw_md ?? null;
}

export function useSessionSummary(
  sessionId: string,
): SessionSummaryRecord | null {
  const { data = null } = useIndexQuery({
    entity: "sessions",
    ids: [sessionId],
    queryKey: ["session-summary", sessionId],
    queryFn: async () => {
      const result = await commands.sessionGet(sessionId);
      if (result.status === "error") {
        throw new Error(result.error);
      }
      const meta = result.data?.meta;
      return meta
        ? { id: meta.id, title: meta.title, created_at: meta.created_at }
        : null;
    },
    enabled: Boolean(sessionId),
  });
  return sessionId ? data : null;
}

export function useSessionSummaries(enabled = true): SessionSummaryRecord[] {
  const { data = EMPTY_SESSION_SUMMARIES } = useIndexQuery({
    entity: "session_headers",
    queryKey: ["session-summaries"],
    enabled,
    queryFn: async () => {
      const result = await commands.sessionListHeaders();
      if (result.status === "error") {
        throw new Error(result.error);
      }
      // session_list_headers is (created_at, id) ASC; this list has always been
      // newest-first with id as the ascending tiebreaker.
      return [...result.data]
        .sort(
          (left, right) =>
            right.created_at.localeCompare(left.created_at) ||
            left.id.localeCompare(right.id),
        )
        .map((entry) => ({
          id: entry.id,
          title: entry.title,
          created_at: entry.created_at,
        }));
    },
  });
  return data;
}

export function useUpdateSession(sessionId: string) {
  return useCallback(
    (changes: SessionChanges) => updateSession(sessionId, changes),
    [sessionId],
  );
}

export function useSessionHasTranscript(sessionId: string): boolean {
  const { data = false } = useIndexQuery({
    // Transcript events carry the session id.
    entity: "transcripts",
    ids: [sessionId],
    queryKey: ["session-has-transcript", sessionId],
    queryFn: async () => {
      const result = await commands.sessionHasTranscript(sessionId);
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return result.data;
    },
    enabled: Boolean(sessionId),
  });
  return sessionId ? data : false;
}

export function useEnhancedNoteRecords(
  sessionId: string,
): EnhancedNoteRecord[] {
  const { data = EMPTY_ENHANCED_NOTES } = useIndexQuery({
    // Doc events carry the session id, not the doc id.
    entity: "docs",
    ids: [sessionId],
    queryKey: ["session-enhanced-docs", sessionId],
    queryFn: async () => {
      const result = await commands.sessionEnhancedDocs(sessionId);
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return result.data.map(mapEnhancedDoc);
    },
    enabled: Boolean(sessionId),
  });
  return sessionId ? data : EMPTY_ENHANCED_NOTES;
}

export function useEnhancedNote(
  enhancedNoteId: string,
  generationId?: string,
  sessionId?: string,
): EnhancedNoteRecord | null {
  const taskStorage = useTaskStorageOptional();
  const { data = null } = useIndexQuery({
    entity: ["docs", "tasks"],
    ids: sessionId ? [sessionId] : undefined,
    refetchOnMount: "always",
    queryKey: generationId
      ? ["enhanced-doc", enhancedNoteId, generationId]
      : ["enhanced-doc", enhancedNoteId],
    queryFn: async () => {
      const isSummary = sessionId !== undefined && enhancedNoteId === sessionId;
      let note: EnhancedNoteRecord;
      if (isSummary) {
        const result = await commands.sessionSummaryGet(sessionId);
        if (result.status === "error") throw new Error(result.error);
        if (result.data === null) return null;
        note = mapEnhancedDoc({
          id: sessionId,
          session_id: sessionId,
          kind: "summary",
          title: "Summary",
          template_id: "",
          sort_order: 0,
          markdown: result.data,
        });
      } else {
        const result = await commands.enhancedDocGet(enhancedNoteId);
        if (result.status === "error") throw new Error(result.error);
        if (!result.data) return null;
        note = mapEnhancedDoc(result.data);
      }
      const source = {
        type: note.id === note.sessionId ? "session_summary" : "enhanced_note",
        id: enhancedNoteId,
      };
      const loadedTasks = await taskStorage?.loadSource?.(source);
      if (taskStorage && note.content) {
        note.content = JSON.stringify(
          hydrateTaskContent({
            content: JSON.parse(note.content),
            sourceTasks: loadedTasks ?? taskStorage.getTasksForSource(source),
            getTask: (id) =>
              loadedTasks?.find((task) => task.taskId === id) ??
              taskStorage.getTask(id),
          }),
        );
      }
      return note;
    },
    enabled: Boolean(enhancedNoteId),
  });
  return enhancedNoteId ? data : null;
}

export function useUpdateEnhancedNoteContent(
  enhancedNoteId: string,
  sessionId: string,
) {
  return useCallback(
    (content: string, sessionTitle?: string) =>
      updateEnhancedNoteContent(
        enhancedNoteId,
        sessionId,
        content,
        sessionTitle,
      ),
    [enhancedNoteId, sessionId],
  );
}

export function updateEnhancedNoteContent(
  enhancedNoteId: string,
  sessionId: string,
  content: string,
  sessionTitle?: string,
): Promise<void> {
  return enqueueDatabaseWrite(`session:${sessionId}`, async () => {
    // The file home is markdown-canonical; the editor hands us prosemirror JSON. Content
    // that doesn't parse is already markdown (defensive -- the enhanced editor always
    // serializes JSON today).
    let markdown = content;
    let parsed;
    try {
      parsed = JSON.parse(content);
    } catch {
      // keep `content` as-is
    }
    if (parsed) markdown = json2mdStrict(parsed);

    const docWrite =
      enhancedNoteId === sessionId
        ? await commands.sessionSaveSummary(
            sessionId,
            markdown,
            null,
            sessionTitle ?? null,
          )
        : await commands.sessionSaveEnhancedDoc(
            sessionId,
            enhancedNoteId,
            {
              markdown,
            },
            sessionTitle ?? null,
          );
    if (docWrite.status === "error") {
      throw new Error(
        `Failed to update summary ${enhancedNoteId}: ${docWrite.error}`,
      );
    }
  });
}

export function saveSessionNote(
  sessionId: string,
  markdown: string,
  title?: string,
): Promise<void> {
  return enqueueDatabaseWrite(`session:${sessionId}:note`, async () => {
    const result = await commands.sessionSaveNote(
      sessionId,
      markdown,
      title ?? null,
    );
    if (result.status === "error") throw new Error(result.error);
  });
}

export function useRefreshSessionNote(sessionId: string) {
  const client = useQueryClient();
  return useCallback(async () => {
    const filter = { queryKey: ["session", sessionId], exact: true };
    // A post-save read also cancels older reads, so an intermediate save echo
    // cannot arrive after the editor releases its local draft.
    await client.cancelQueries(filter);
    await client.refetchQueries(
      { ...filter, type: "all" },
      { throwOnError: true },
    );
    const readContent = () => {
      const session = client.getQueryData<SessionRecord>(filter.queryKey);
      if (!session) throw new Error("The saved note could not be reloaded");
      return ensureFirstLineTitle(
        parseJsonContent(session.raw_md),
        session.title,
      );
    };
    readContent();
    return readContent;
  }, [client, sessionId]);
}

export function useRefreshEnhancedNote(
  enhancedNoteId: string,
  sessionId: string,
  generationId?: string,
) {
  const client = useQueryClient();
  return useCallback(async () => {
    const sessionFilter = { queryKey: ["session", sessionId], exact: true };
    const docFilter = {
      queryKey: generationId
        ? ["enhanced-doc", enhancedNoteId, generationId]
        : ["enhanced-doc", enhancedNoteId],
      exact: true,
    };
    await Promise.all([
      client.cancelQueries(sessionFilter),
      client.cancelQueries(docFilter),
    ]);
    await Promise.all([
      client.refetchQueries(
        { ...sessionFilter, type: "all" },
        { throwOnError: true },
      ),
      client.refetchQueries(
        { ...docFilter, type: "all" },
        { throwOnError: true },
      ),
    ]);
    const readContent = () => {
      const session = client.getQueryData<SessionRecord>(
        sessionFilter.queryKey,
      );
      const note = client.getQueryData<EnhancedNoteRecord>(docFilter.queryKey);
      if (!session || !note)
        throw new Error("The saved summary could not be reloaded");
      return ensureFirstLineTitle(
        parseJsonContent(note.content),
        session.title,
      );
    };
    readContent();
    return readContent;
  }, [client, enhancedNoteId, generationId, sessionId]);
}

export function deleteEnhancedNote(
  enhancedNoteId: string,
  sessionId: string,
): Promise<void> {
  return enqueueDatabaseWrite(`enhanced-note:${enhancedNoteId}`, async () => {
    const result =
      enhancedNoteId === sessionId
        ? await commands.sessionDeleteSummary(sessionId)
        : await commands.sessionDeleteEnhancedDoc(sessionId, enhancedNoteId);
    if (result.status === "error") {
      throw new Error(
        `Failed to delete summary ${enhancedNoteId}: ${result.error}`,
      );
    }
  });
}

// File-first: `_meta.json` is canonical for session meta, so every change goes through the
// store's `session_update_meta` (read-modify-write of the file, then the SQL dual-write) --
// a raw sessions-table update here would leave the file stale and the next rebuild would revert
// the change to the old file value.
export function updateSession(
  sessionId: string,
  changes: SessionChanges,
): Promise<void> {
  return enqueueDatabaseWrite(`session:${sessionId}`, async () => {
    const patch: SessionMetaPatch = {};
    if (changes.title !== undefined) patch.title = changes.title;
    if (changes.created_at !== undefined) patch.created_at = changes.created_at;
    if (changes.folder_id !== undefined) patch.folder = changes.folder_id;
    if (changes.tags !== undefined) patch.tags = changes.tags;

    if (Object.keys(patch).length === 0) return;

    const result = await commands.sessionUpdateMeta(sessionId, patch);
    if (result.status === "error") {
      throw new Error(`Failed to update session ${sessionId}: ${result.error}`);
    }
  });
}

export async function createSession(
  title = "",
  _userId = DEFAULT_USER_ID,
  // A session can be *created* with initial content or a tracking marker (the
  // onboarding welcome note) -- that seeds the file-canonical store directly
  // below, never `SessionChanges`/`updateSession`'s path.
  initial?: { tracking_id?: string; raw_md?: string },
): Promise<string> {
  const sessionId = id();
  const now = new Date().toISOString();

  const metaWrite = await commands.sessionWriteMeta({
    id: sessionId,
    title,
    started_at: null,
    ended_at: null,
    created_at: now,
    tags: [],
    tracking_id: initial?.tracking_id ?? null,
    folder: null,
  });
  if (metaWrite.status === "error") {
    throw new Error(
      `Failed to create session ${sessionId}: ${metaWrite.error}`,
    );
  }

  // No SQL placeholder note row anymore: every reader that COALESCE'd onto the old
  // bare-id `session_documents` row (session reads, isSessionEmpty, the content
  // snapshot) now reads the file-backed index, where "no note yet" is simply an
  // absent `notes.md`. Rebuild pruned the file-less placeholder row on every
  // startup/focus rescan anyway, and it shadowed the real `<id>:note` row in the
  // search projection's note lookup -- nothing depends on it.
  if (initial?.raw_md) {
    let markdown = "";
    try {
      markdown = json2md(JSON.parse(initial.raw_md));
    } catch (error) {
      console.error(
        "[session] failed to convert initial note content to markdown",
        error,
      );
    }
    if (markdown) {
      const noteWrite = await commands.sessionWriteNote(sessionId, markdown);
      if (noteWrite.status === "error") {
        console.error(
          "[session] failed to seed session note file",
          noteWrite.error,
        );
      }
    }
  }

  return sessionId;
}

export async function softDeleteSession(
  sessionId: string,
  tombstone = new Date().toISOString(),
): Promise<DeletedSessionData | null> {
  // Read title before deleting: session_delete removes the index entry outright (nothing
  // left to read back from), so anything the undo toast needs to display has to be
  // captured up front.
  const preRead = await commands.sessionGet(sessionId);
  if (preRead.status === "error") {
    throw new Error(`session_get failed: ${preRead.error}`);
  }
  const session = preRead.data;
  if (!session) return null;

  const result = await commands.sessionDelete(sessionId);
  if (result.status === "error") {
    // Distinct from "already deleted" (the pre-read above finding no session, which is
    // benign and returns null): the session existed and the store call itself failed. Throw so
    // useDeleteSession's existing catch rolls back the optimistic UI and shows an error toast --
    // a genuine command error must never look identical to a no-op idempotent delete.
    throw new Error(`session_delete failed: ${result.error}`);
  }

  return {
    session: { id: session.meta.id, title: session.meta.title },
    tombstone,
    deletedAt: Date.now(),
  };
}

// The emptiness semantics (title, note content after trimming, transcript/
// enhanced-doc/tag counts, and attachments) live on the Rust side now -- see
// `SessionStore::session_is_empty`.
export async function isSessionEmpty(sessionId: string): Promise<boolean> {
  const result = await commands.sessionIsEmpty(sessionId);
  if (result.status === "error") {
    throw new Error(result.error);
  }
  return result.data;
}

export async function restoreDeletedSession(
  data: DeletedSessionData,
): Promise<void> {
  // The undo toast shows before the delete write commits. Wait for the in-flight
  // delete to settle first -- session_restore looks for a folder under *today's*
  // trash dir, which only exists once session_delete has actually moved it there.
  await waitForPendingSoftDelete(data.session.id);

  const result = await commands.sessionRestore(data.session.id);
  if (result.status === "error") {
    throw new Error(
      `Failed to restore session ${data.session.id}: ${result.error}`,
    );
  }
  if (!result.data) {
    throw new Error(`Session ${data.session.id} was never soft-deleted`);
  }
}

// There is deliberately no "finalize the deletion" step: `session_delete` moves the whole
// session folder into `.trash/<date>/` atomically and `session_restore` reverses it, so a
// delete is already complete when `softDeleteSession` resolves. The old finalize called
// fs-sync's `delete_session_folder` (a plain `remove_dir_all`, no trash, no undo) on a 5s
// timer -- on an already-trashed session that was a no-op, but anything that recreated the
// path inside that window (a sync client pulling the folder back from another device, a
// late transcript flush) was destroyed irrecoverably. Hard-deleting on a synced vault is
// not something this app does.

function mapSessionRecord(record: StoreSessionRecord): SessionRecord {
  // `note_markdown` is always markdown (the file-canonical `notes.md`); consumers
  // still expect the stringified prosemirror doc the SQL era handed them.
  let rawMd = record.note_markdown ?? "";
  if (rawMd) {
    try {
      rawMd = JSON.stringify(md2json(rawMd));
    } catch (error) {
      console.error("[session] failed to decode note Markdown", error);
    }
  }

  return {
    id: record.meta.id,
    // The owner concept died with the workspaces removal (D10).
    user_id: DEFAULT_USER_ID,
    created_at: record.meta.created_at,
    folder_id: record.meta.folder ?? "",
    title: record.meta.title,
    raw_md: rawMd,
    tags: record.meta.tags,
    author: record.meta.author ?? null,
    tag_suggestions: record.meta.tag_suggestions ?? null,
  };
}

function mapEnhancedDoc(doc: EnhancedDoc): EnhancedNoteRecord {
  // `markdown` is the file body; consumers expect the stringified prosemirror doc.
  let content = doc.markdown;
  if (content) {
    try {
      content = JSON.stringify(md2json(content));
    } catch (error) {
      console.error("[session] failed to decode summary Markdown", error);
    }
  }

  return {
    id: doc.id,
    sessionId: doc.session_id,
    title: doc.title,
    content,
    templateId: doc.template_id,
    position: Number(doc.sort_order),
  };
}
