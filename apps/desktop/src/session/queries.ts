import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useSyncExternalStore,
} from "react";

import {
  json2md,
  json2mdStrict,
  md2json,
  parseJsonContent,
} from "@hypr/editor/markdown";
import { hydrateTaskContent } from "@hypr/editor/tasks";

import { taskItemToRecord } from "~/editor-bridge/task-records";
import { waitForPendingSoftDelete } from "~/session/pending-soft-deletes";
import { useSessionSummaries } from "~/session/summary-queries";
import { ensureFirstLineTitle } from "~/session/title-content";
import { getApplicationClient } from "~/shared/application/client";
import { documentEditorBaseline } from "~/shared/application/document-editor-draft";
import type {
  DocumentTarget,
  MetadataSnapshot,
  NoteEditorSnapshot,
} from "~/shared/application/protocol.gen";
import { useVaultReadScope } from "~/shared/application/use-vault-read-scope";
import { useVaultHost } from "~/shared/application/vault-provider";
import { DEFAULT_USER_ID, id } from "~/shared/utils";
import { enqueueDatabaseWrite } from "~/shared/write-queue";
import type { DeletedSessionData } from "~/store/zustand/undo-delete";
import { useTranscriptOverview } from "~/stt/transcript-overview";
import {
  type EnhancedDoc,
  type SessionMetaPatch,
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

function projectRawNote(
  sessionId: string,
  snapshot: NoteEditorSnapshot,
): string | null {
  const source = { type: "session_raw_note", id: sessionId };
  if (
    snapshot.tasks.tasks.some(
      (task) => task.body !== null && !Array.isArray(task.body),
    )
  )
    return null;
  const tasks = snapshot.tasks.tasks.map(taskItemToRecord);
  if (
    tasks.some(
      (task) =>
        !task || task.sourceType !== source.type || task.sourceId !== source.id,
    )
  )
    return null;
  if (!snapshot.note.markdown) return "";
  const owned = tasks.filter((task) => task !== null);
  const byId = new Map(owned.map((task) => [task.taskId, task]));
  return JSON.stringify(
    hydrateTaskContent({
      content: md2json(snapshot.note.markdown),
      sourceTasks: owned,
      getTask: (id) => byId.get(id) ?? null,
    }),
  );
}

export function useSessionMetadata(sessionId: string): MetadataSnapshot | null {
  const owner = useVaultReadScope();
  const query = useQuery(
    {
      ...owner.scope.metadataOptions(sessionId),
      enabled:
        !!sessionId &&
        owner.readable &&
        owner.scope.client.features.includes("content_read"),
    },
    owner.scope.queryClient,
  );
  return sessionId && owner.readable && query.data?.meta.id === sessionId
    ? query.data
    : null;
}

function useRawNoteSource(sessionId: string) {
  const owner = useVaultReadScope();
  const select = useCallback(
    (snapshot: NoteEditorSnapshot) => projectRawNote(sessionId, snapshot),
    [sessionId],
  );
  const query = useQuery(
    {
      ...owner.scope.noteEditorOptions(sessionId),
      enabled:
        !!sessionId &&
        owner.readable &&
        owner.scope.client.features.includes("note_editor_read"),
      refetchOnMount: "always",
      select,
    },
    owner.scope.queryClient,
  );
  return sessionId && owner.readable ? (query.data ?? null) : null;
}

export function useSession(sessionId: string): SessionRecord | null {
  const metadata = useSessionMetadata(sessionId);
  const rawMd = useRawNoteSource(sessionId);
  return useMemo(
    () =>
      metadata && rawMd !== null
        ? {
            id: metadata.meta.id,
            user_id: DEFAULT_USER_ID,
            created_at: metadata.meta.created_at,
            folder_id: metadata.meta.folder ?? "",
            title: metadata.meta.title,
            raw_md: rawMd,
            tags: metadata.meta.tags,
            author: metadata.meta.author ?? null,
            tag_suggestions: metadata.meta.tag_suggestions ?? null,
          }
        : null,
    [metadata, rawMd],
  );
}

export function useSessionRawMd(sessionId: string): string | null {
  return useRawNoteSource(sessionId);
}

export function useSessionSummary(
  sessionId: string,
): SessionSummaryRecord | null {
  const { scope } = useVaultHost();
  if (!scope)
    throw new Error("Session headers require an immutable vault host");
  const { data } = useSessionSummaries(
    scope,
    { kind: "ids", ids: sessionId ? [sessionId] : [] },
    Boolean(sessionId) && scope.client.features.includes("session_summaries"),
  );
  return sessionId && scope.getSnapshot().state === "connected"
    ? (data?.find((header) => header.id === sessionId) ?? null)
    : null;
}

export function useUpdateSession(
  sessionId: string,
  assertCurrent?: () => void,
) {
  return useCallback(
    (changes: SessionChanges) =>
      updateSession(sessionId, changes, assertCurrent),
    [sessionId, assertCurrent],
  );
}

export function useSessionHasTranscript(sessionId: string): boolean {
  return useTranscriptOverview(sessionId).data?.has_words ?? false;
}

const terminalDocumentStates = new Set([
  "closed",
  "access_denied",
  "login_required",
  "incompatible",
]);
function useDocumentScope() {
  const { scope } = useVaultHost();
  if (!scope) throw new Error("Documents require an immutable vault host");
  const owner = useRef({
    scope,
    retired: terminalDocumentStates.has(scope.getSnapshot().state),
  }).current;
  if (owner.scope !== scope) owner.retired = true;
  const subscribe = useCallback(
    (changed: () => void) =>
      owner.scope.subscribe(() => {
        if (terminalDocumentStates.has(owner.scope.getSnapshot().state))
          owner.retired = true;
        changed();
      }),
    [owner],
  );
  const connection = useSyncExternalStore(subscribe, owner.scope.getSnapshot);
  if (terminalDocumentStates.has(connection.state)) owner.retired = true;
  return {
    scope: owner.scope,
    readable:
      !owner.retired &&
      (connection.state === "connected" || connection.state === "reconnecting"),
  };
}
function useSessionDocuments(sessionId: string) {
  const owner = useDocumentScope();
  const query = useQuery(
    {
      ...owner.scope.documentsOptions(sessionId),
      enabled: !!sessionId && owner.readable,
    },
    owner.scope.queryClient,
  );
  return {
    ...owner,
    data: sessionId && owner.readable ? query.data : undefined,
  };
}
export function useEnhancedNoteRecords(
  sessionId: string,
): EnhancedNoteRecord[] {
  const { data } = useSessionDocuments(sessionId);
  return useMemo(
    () => data?.map(mapEnhancedDoc) ?? EMPTY_ENHANCED_NOTES,
    [data],
  );
}
export function useEnhancedNoteTitle(
  sessionId: string,
  enhancedNoteId: string,
) {
  const { data } = useSessionDocuments(sessionId);
  return useMemo(() => {
    const matches = data?.filter(
      (doc) => doc.id === enhancedNoteId && doc.session_id === sessionId,
    );
    return matches?.length === 1 ? matches[0].title : undefined;
  }, [data, sessionId, enhancedNoteId]);
}
export function useEnhancedDocumentTarget(
  sessionId: string,
  enhancedNoteId: string,
): DocumentTarget | null | undefined {
  const { data } = useSessionDocuments(sessionId);
  return useMemo(() => {
    if (!data) return undefined;
    if (!enhancedNoteId) return null;
    const matches = data.filter(
      (doc) => doc.id === enhancedNoteId && doc.session_id === sessionId,
    );
    if (matches.length !== 1) return null;
    const doc = matches[0];
    if (doc.kind === "template_output" && doc.id === sessionId) return null;
    return doc.kind === "summary"
      ? { kind: "summary" }
      : { kind: "template_output", document_id: doc.id };
  }, [data, sessionId, enhancedNoteId]);
}
export function useEnhancedNote(
  enhancedNoteId: string,
  generationId: string | undefined,
  sessionId: string,
): EnhancedNoteRecord | null {
  const { scope, readable } = useDocumentScope();
  const target = useEnhancedDocumentTarget(sessionId, enhancedNoteId);
  const options = useMemo(
    () => scope.documentEditorOptions(sessionId, target ?? { kind: "summary" }),
    [scope, sessionId, target],
  );
  const supported = scope.client.features.includes("document_editor_read");
  const query = useQuery(
    { ...options, enabled: !!sessionId && !!target && readable && supported },
    scope.queryClient,
  );
  useEffect(() => {
    if (generationId && target && readable && supported)
      void scope.queryClient.invalidateQueries({
        queryKey: options.queryKey,
        exact: true,
      });
  }, [scope, generationId, target, readable, supported, options.queryKey]);
  return useMemo(() => {
    if (
      !target ||
      !readable ||
      !supported ||
      query.error ||
      !query.data?.document.document
    )
      return null;
    try {
      documentEditorBaseline(sessionId, target, query.data);
    } catch {
      return null;
    }
    const note = mapEnhancedDoc(query.data.document.document);
    const tasks = query.data.tasks.tasks.map(taskItemToRecord);
    if (tasks.some((task) => task === null)) return null;
    const records = tasks.filter((task) => task !== null);
    const byId = new Map(records.map((task) => [task.taskId, task]));
    if (note.content)
      note.content = JSON.stringify(
        hydrateTaskContent({
          content: parseJsonContent(note.content),
          sourceTasks: records,
          getTask: (id) => byId.get(id) ?? null,
        }),
      );
    return note;
  }, [sessionId, target, readable, supported, query.error, query.data]);
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
        ? await getApplicationClient().store.sessionSaveSummary(
            sessionId,
            markdown,
            null,
            sessionTitle ?? null,
          )
        : await getApplicationClient().store.sessionSaveEnhancedDoc(
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
    const result = await getApplicationClient().store.sessionSaveNote(
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
  assertCurrent?: () => void,
): Promise<void> {
  assertCurrent?.();
  const backend = getApplicationClient().store;
  return enqueueDatabaseWrite(`enhanced-note:${enhancedNoteId}`, async () => {
    assertCurrent?.();
    const result =
      enhancedNoteId === sessionId
        ? await backend.sessionDeleteSummary(sessionId)
        : await backend.sessionDeleteEnhancedDoc(sessionId, enhancedNoteId);
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
  assertCurrent?: () => void,
): Promise<void> {
  assertCurrent?.();
  const backend = getApplicationClient().store;
  return enqueueDatabaseWrite(`session:${sessionId}`, async () => {
    assertCurrent?.();
    const patch: SessionMetaPatch = {};
    if (changes.title !== undefined) patch.title = changes.title;
    if (changes.created_at !== undefined) patch.created_at = changes.created_at;
    if (changes.folder_id !== undefined) patch.folder = changes.folder_id;
    if (changes.tags !== undefined) patch.tags = changes.tags;

    if (Object.keys(patch).length === 0) return;

    const result = await backend.sessionUpdateMeta(sessionId, patch);
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

  const metaWrite = await getApplicationClient().store.sessionWriteMeta({
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
      const noteWrite = await getApplicationClient().store.sessionWriteNote(
        sessionId,
        markdown,
      );
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
  assertCurrent?: () => void,
): Promise<DeletedSessionData | null> {
  // Read title before deleting: session_delete removes the index entry outright (nothing
  // left to read back from), so anything the undo toast needs to display has to be
  // captured up front.
  assertCurrent?.();
  const backend = getApplicationClient().store;
  const preRead = await backend.sessionGet(sessionId);
  assertCurrent?.();
  if (preRead.status === "error") {
    throw new Error(`session_get failed: ${preRead.error}`);
  }
  const session = preRead.data;
  if (!session) return null;

  const result = await backend.sessionDelete(sessionId);
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
  const result = await getApplicationClient().store.sessionIsEmpty(sessionId);
  if (result.status === "error") {
    throw new Error(result.error);
  }
  return result.data;
}

export function captureSessionRestore(scope: string) {
  const backend = getApplicationClient().store;
  return (sessionId: string, assertCurrent: () => void): Promise<void> =>
    enqueueDatabaseWrite(
      `undo-restore:${JSON.stringify([scope, sessionId])}`,
      async () => {
        assertCurrent();
        await waitForPendingSoftDelete(scope, sessionId);
        assertCurrent();
        const result = await backend.sessionRestore(sessionId);
        if (result.status === "error")
          throw new Error(
            `Failed to restore session ${sessionId}: ${result.error}`,
          );
        if (!result.data)
          throw new Error(`Session ${sessionId} was never soft-deleted`);
      },
    );
}

// There is deliberately no "finalize the deletion" step: `session_delete` moves the whole
// session folder into `.trash/<date>/` atomically and `session_restore` reverses it, so a
// delete is already complete when `softDeleteSession` resolves. The old finalize called
// fs-sync's `delete_session_folder` (a plain `remove_dir_all`, no trash, no undo) on a 5s
// timer -- on an already-trashed session that was a no-op, but anything that recreated the
// path inside that window (a sync client pulling the folder back from another device, a
// late transcript flush) was destroyed irrecoverably. Hard-deleting on a synced vault is
// not something this app does.

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
