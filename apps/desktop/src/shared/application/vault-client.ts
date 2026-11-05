import { applicationResult, ApplicationRequestError } from "./errors";
import type {
  ApplicationError,
  Change,
  CreateSessionRequest,
  EnhancedDoc,
  NoteSaveOutcome,
  NoteSnapshot,
  PersonItem,
  SaveNoteRequest,
  SessionDocument,
  SessionPage,
  TagItem,
  TaskItem,
  TranscriptWithData,
  VaultInfo,
} from "./protocol.gen";
import type { NativeRemoteConnection } from "./remote";

import {
  commands,
  events,
  type ConnectionState,
  type SearchRequest,
  type SearchResult,
} from "~/types/tauri.gen";

export type RequestOptions = { signal?: AbortSignal; requestId?: string };
export type VaultClientEvent =
  | { kind: "change"; change: Change }
  | {
      kind: "connection";
      state: ConnectionState;
      error: ApplicationError | null;
    };

export type VaultClient = {
  readonly scope: string;
  readonly networkMode: "always" | "online";
  readonly canEdit: boolean;
  readonly features: readonly string[];
  documents: (
    sessionId: string,
    options?: RequestOptions,
  ) => Promise<EnhancedDoc[]>;
  transcripts: (
    sessionId: string,
    options?: RequestOptions,
  ) => Promise<TranscriptWithData[]>;
  tasks: (sessionId: string, options?: RequestOptions) => Promise<TaskItem[]>;
  people: (options?: RequestOptions) => Promise<PersonItem[]>;
  tags: (options?: RequestOptions) => Promise<TagItem[]>;
  sessions: (
    limit: number,
    after?: string | null,
    options?: RequestOptions,
  ) => Promise<SessionPage>;
  session: (
    sessionId: string,
    options?: RequestOptions,
  ) => Promise<SessionDocument>;
  createSession: (
    input: CreateSessionRequest,
    options?: RequestOptions,
  ) => Promise<SessionDocument>;
  readNote: (
    sessionId: string,
    options?: RequestOptions,
  ) => Promise<NoteSnapshot>;
  saveNote: (
    sessionId: string,
    input: SaveNoteRequest,
    options?: RequestOptions,
  ) => Promise<NoteSaveOutcome>;
  search: (
    input: SearchRequest,
    options?: RequestOptions,
  ) => Promise<SearchResult>;
  subscribe: (
    listener: (event: VaultClientEvent) => void,
  ) => Promise<() => void>;
};

async function ipc<T>(
  operation: () => Promise<
    { status: "ok"; data: T } | { status: "error"; error: ApplicationError }
  >,
  options?: RequestOptions,
  mutation = false,
): Promise<T> {
  options?.signal?.throwIfAborted();
  const result = applicationResult(await operation());
  if (options?.signal?.aborted) {
    throw new ApplicationRequestError({
      code: "cancelled",
      message: "The request was cancelled after being sent",
      request_id: options.requestId ?? null,
      retryable: false,
      current_revision: null,
      mutation_outcome: mutation ? "unknown" : null,
    });
  }
  return result;
}

export async function connectLocalVault(): Promise<VaultClient> {
  const scope = applicationResult(await commands.applicationScope());
  return {
    scope,
    networkMode: "always",
    canEdit: true,
    features: ["notes", "search", "content_read"],
    documents: (sessionId, options) =>
      ipc(() => commands.applicationDocuments(sessionId), options),
    transcripts: (sessionId, options) =>
      ipc(() => commands.applicationTranscripts(sessionId), options),
    tasks: (sessionId, options) =>
      ipc(() => commands.applicationTasks(sessionId), options),
    people: (options) => ipc(() => commands.applicationPeople(), options),
    tags: (options) => ipc(() => commands.applicationTags(), options),
    sessions: (limit, after, options) =>
      ipc(() => commands.applicationSessions(limit, after ?? null), options),
    session: (sessionId, options) =>
      ipc(() => commands.applicationSession(sessionId), options),
    createSession: (input, options) => {
      const requestId = options?.requestId ?? crypto.randomUUID();
      return ipc(
        () => commands.applicationCreateSession(requestId, input),
        { ...options, requestId },
        true,
      );
    },
    readNote: (sessionId, options) =>
      ipc(() => commands.applicationReadNote(sessionId), options),
    saveNote: (sessionId, input, options) =>
      ipc(() => commands.applicationSaveNote(sessionId, input), options, true),
    search: (input, options) =>
      ipc(() => commands.applicationSearch(input), options),
    subscribe: async (listener) => {
      const unsubscribe = await events.applicationChanged.listen(
        ({ payload }) => {
          if (payload.event.vault_id === scope)
            listener({ kind: "change", change: payload.event.change });
        },
      );
      listener({ kind: "connection", state: "connected", error: null });
      listener({ kind: "change", change: { kind: "resync_required" } });
      return unsubscribe;
    },
  };
}

export function connectRemoteVault(
  connection: NativeRemoteConnection,
  memberId: string,
  vault: Pick<VaultInfo, "id" | "role">,
): VaultClient {
  const vaultId = vault.id;
  return {
    scope: JSON.stringify([
      "remote",
      connection.info.server,
      memberId,
      vaultId,
    ]),
    networkMode: "online",
    canEdit: vault.role !== "reader",
    features: connection.info.capabilities.features,
    documents: (sessionId, options) =>
      connection.request(
        { kind: "documents", vault_id: vaultId, session_id: sessionId },
        "documents",
        options,
      ),
    transcripts: (sessionId, options) =>
      connection.request(
        { kind: "transcripts", vault_id: vaultId, session_id: sessionId },
        "transcripts",
        options,
      ),
    tasks: (sessionId, options) =>
      connection.request(
        { kind: "tasks", vault_id: vaultId, session_id: sessionId },
        "tasks",
        options,
      ),
    people: (options) =>
      connection.request(
        { kind: "people", vault_id: vaultId },
        "people",
        options,
      ),
    tags: (options) =>
      connection.request({ kind: "tags", vault_id: vaultId }, "tags", options),
    sessions: (limit, after, options) =>
      connection.request(
        { kind: "sessions", vault_id: vaultId, limit, after: after ?? null },
        "sessions",
        options,
      ),
    session: (sessionId, options) =>
      connection.request(
        { kind: "session", vault_id: vaultId, session_id: sessionId },
        "session",
        options,
      ),
    createSession: (input, options) =>
      connection.request(
        { kind: "create_session", vault_id: vaultId, input },
        "session",
        options,
      ),
    readNote: (sessionId, options) =>
      connection.request(
        { kind: "note", vault_id: vaultId, session_id: sessionId },
        "note",
        options,
      ),
    saveNote: (sessionId, input, options) =>
      connection.request(
        { kind: "save_note", vault_id: vaultId, session_id: sessionId, input },
        "note_save",
        options,
      ),
    search: (input, options) =>
      connection.request(
        { kind: "search", vault_id: vaultId, input },
        "search",
        options,
      ),
    subscribe: (listener) =>
      connection.subscribe(vaultId, (event) => {
        if (event.kind === "change")
          listener({ kind: "change", change: event.event.change });
        else listener(event);
      }),
  };
}
