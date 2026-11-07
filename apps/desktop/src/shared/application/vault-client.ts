import {
  type AttachmentPresentation,
  legacyAttachmentPresentation,
  scopedAttachmentPresentation,
} from "@hypr/editor/attachment-presentation";

import {
  openLocalAttachment,
  openRemoteAttachment,
  type AttachmentResource,
} from "./attachments";
import {
  prepareDeviceAttachmentSource,
  type PreparedAttachmentUpload,
} from "./device-attachment-source";
import { applicationResult, ApplicationRequestError } from "./errors";
import {
  openLocalRecording,
  openRemoteRecording,
  type PlaybackRecording,
} from "./playback";
import type {
  ApplicationError,
  AttachmentPage,
  AttachmentUploadStatus,
  UploadAttachmentRequest,
  DeleteRecordingRequest,
  RemoveAttachmentRequest,
  FileInfo,
  FileMetadata,
  MetadataSnapshot,
  MetadataSaveOutcome,
  SaveMetadataRequest,
  ListMeetingsInput,
  SearchMeetingsInput,
  Meeting,
  MeetingPage,
  MeetingTranscript,
  MeetingExport,
  SearchPage,
  RenderedTranscriptSegment,
  Change,
  CreateSessionRequest,
  DocumentTarget,
  DocumentSnapshot,
  DocumentSaveOutcome,
  SaveDocumentRequest,
  EnhancedDoc,
  NoteSaveOutcome,
  NoteSnapshot,
  NoteEditorSnapshot,
  SaveNoteEditorRequest,
  NoteEditorSaveOutcome,
  PersonItem,
  SaveNoteRequest,
  SessionDocument,
  SessionPage,
  TagItem,
  TaskItem,
  TasksSnapshot,
  TasksSaveOutcome,
  ReplaceTasksRequest,
  TranscriptWithData,
  VaultInfo,
  SummaryJobRequest,
  ListProcessingJobsInput,
  ProcessingJobSnapshot,
  ProcessingJobPage,
  ProcessingJobResult,
  ProcessingPolicy,
  ProcessingSettingsSnapshot,
  ProcessingSettingsSaveOutcome,
  SaveProcessingSettingsRequest,
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
  readonly attachmentPresentation: AttachmentPresentation;
  readonly features: readonly string[];
  processingPolicy: (options?: RequestOptions) => Promise<ProcessingPolicy>;
  processingSettings: (
    options?: RequestOptions,
  ) => Promise<ProcessingSettingsSnapshot>;
  saveProcessingSettings: (
    input: SaveProcessingSettingsRequest,
    options?: RequestOptions,
  ) => Promise<ProcessingSettingsSaveOutcome>;
  submitSummaryJob: (
    input: SummaryJobRequest,
    options: RequestOptions & { requestId: string },
  ) => Promise<ProcessingJobSnapshot>;
  processingJobs: (
    input: ListProcessingJobsInput,
    options?: RequestOptions,
  ) => Promise<ProcessingJobPage>;
  processingJob: (
    jobId: string,
    options?: RequestOptions,
  ) => Promise<ProcessingJobSnapshot>;
  processingJobResult: (
    jobId: string,
    options?: RequestOptions,
  ) => Promise<ProcessingJobResult | null>;
  cancelProcessingJob: (
    jobId: string,
    options?: RequestOptions,
  ) => Promise<ProcessingJobSnapshot>;
  prepareAttachmentSource: typeof prepareDeviceAttachmentSource;
  uploadAttachmentSource: (
    sessionId: string,
    prepared: Pick<PreparedAttachmentUpload, "input" | "source">,
    options: RequestOptions & { requestId: string },
  ) => Promise<FileMetadata>;
  prepareAttachmentUpload: (
    sourcePath: string,
    options?: RequestOptions,
  ) => Promise<UploadAttachmentRequest>;
  uploadAttachmentPath: (
    sessionId: string,
    sourcePath: string,
    input: UploadAttachmentRequest,
    options: RequestOptions & { requestId: string },
  ) => Promise<FileMetadata>;
  attachmentUploadStatus: (
    sessionId: string,
    uploadId: string,
    input: UploadAttachmentRequest,
    options?: RequestOptions,
  ) => Promise<AttachmentUploadStatus>;
  openAttachment: (
    sessionId: string,
    attachmentId: string,
    options?: RequestOptions,
  ) => Promise<AttachmentResource | null>;
  noteWriteQueueKey?: (sessionId: string) => string;
  deleteRecording: (
    sessionId: string,
    input: DeleteRecordingRequest,
    options?: RequestOptions,
  ) => Promise<boolean>;
  openRecording: (
    sessionId: string,
    options?: RequestOptions,
  ) => Promise<PlaybackRecording | null>;
  audioMetadata: (
    sessionId: string,
    options?: RequestOptions,
  ) => Promise<FileMetadata | null>;
  attachmentInfo: (
    sessionId: string,
    attachmentId: string,
    options?: RequestOptions,
  ) => Promise<FileInfo | null>;
  attachmentMetadata: (
    sessionId: string,
    attachmentId: string,
    options?: RequestOptions,
  ) => Promise<FileMetadata | null>;
  removeAttachment: (
    sessionId: string,
    attachmentId: string,
    input: RemoveAttachmentRequest,
    options?: RequestOptions,
  ) => Promise<boolean>;
  attachments: (
    sessionId: string,
    limit: number,
    after?: string,
    options?: RequestOptions,
  ) => Promise<AttachmentPage>;
  readMetadata: (
    sessionId: string,
    options?: RequestOptions,
  ) => Promise<MetadataSnapshot>;
  saveMetadata: (
    sessionId: string,
    input: SaveMetadataRequest,
    options?: RequestOptions,
  ) => Promise<MetadataSaveOutcome>;
  agentList: (
    input: ListMeetingsInput,
    options?: RequestOptions,
  ) => Promise<MeetingPage>;
  agentSearch: (
    input: SearchMeetingsInput,
    options?: RequestOptions,
  ) => Promise<SearchPage>;
  agentMeeting: (
    sessionId: string,
    options?: RequestOptions,
  ) => Promise<Meeting>;
  agentTranscript: (
    sessionId: string,
    options?: RequestOptions,
  ) => Promise<MeetingTranscript>;
  agentExport: (
    sessionId: string,
    options?: RequestOptions,
  ) => Promise<MeetingExport>;
  agentSegments: (
    sessionId: string,
    options?: RequestOptions,
  ) => Promise<RenderedTranscriptSegment[]>;
  documents: (
    sessionId: string,
    options?: RequestOptions,
  ) => Promise<EnhancedDoc[]>;
  readDocument: (
    sessionId: string,
    target: DocumentTarget,
    options?: RequestOptions,
  ) => Promise<DocumentSnapshot>;
  saveDocument: (
    sessionId: string,
    target: DocumentTarget,
    input: SaveDocumentRequest,
    options?: RequestOptions,
  ) => Promise<DocumentSaveOutcome>;
  transcripts: (
    sessionId: string,
    options?: RequestOptions,
  ) => Promise<TranscriptWithData[]>;
  tasks: (sessionId: string, options?: RequestOptions) => Promise<TaskItem[]>;
  tasksSnapshot: (
    sessionId: string,
    options?: RequestOptions,
  ) => Promise<TasksSnapshot>;
  replaceTasks: (
    sessionId: string,
    input: ReplaceTasksRequest,
    options?: RequestOptions,
  ) => Promise<TasksSaveOutcome>;
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
  readNoteEditor: (
    sessionId: string,
    options?: RequestOptions,
  ) => Promise<NoteEditorSnapshot>;
  saveNoteEditor: (
    sessionId: string,
    input: SaveNoteEditorRequest,
    options?: RequestOptions,
  ) => Promise<NoteEditorSaveOutcome>;
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
  const [scope, processingAvailable] = await Promise.all([
    commands.applicationScope().then(applicationResult),
    commands.applicationProcessingAvailable(),
  ]);
  return {
    scope,
    networkMode: "always",
    canEdit: true,
    attachmentPresentation: legacyAttachmentPresentation,
    noteWriteQueueKey: (sessionId) => `session:${sessionId}:note`,
    features: [
      ...(processingAvailable
        ? ["summary_body_jobs", "processing_settings"]
        : []),
      "attachment_upload",
      "audio_playback",
      "recording_delete",
      "notes",
      "note_editor_read",
      "note_editor_write",
      "audio_metadata_read",
      "attachment_info",
      "attachment_list",
      "attachment_metadata",
      "attachment_remove",
      "artifact_details",
      "search",
      "content_read",
      "document_edit",
      "metadata_edit",
      "tasks_edit",
      "agent_read",
    ],
    processingPolicy: (options) =>
      ipc(() => commands.applicationProcessingPolicy(), options),
    processingSettings: (options) =>
      ipc(() => commands.applicationProcessingSettings(), options),
    saveProcessingSettings: (input, options) =>
      ipc(
        () => commands.applicationSaveProcessingSettings(input),
        options,
        true,
      ),
    submitSummaryJob: (input, options) =>
      ipc(
        () => commands.applicationSubmitSummaryJob(options.requestId, input),
        options,
        true,
      ),
    processingJobs: (input, options) =>
      ipc(() => commands.applicationProcessingJobs(input), options),
    processingJob: (jobId, options) =>
      ipc(() => commands.applicationProcessingJob(jobId), options),
    processingJobResult: (jobId, options) =>
      ipc(() => commands.applicationProcessingJobResult(jobId), options),
    cancelProcessingJob: (jobId, options) =>
      ipc(() => commands.applicationCancelProcessingJob(jobId), options, true),
    openRecording: openLocalRecording,
    prepareAttachmentUpload: prepareDeviceAttachmentUpload,
    prepareAttachmentSource: prepareDeviceAttachmentSource,
    uploadAttachmentSource: (sessionId, prepared, options) =>
      ipc(
        () =>
          commands.applicationUploadAttachmentSource(
            options.requestId,
            sessionId,
            prepared.input,
            prepared.source,
          ),
        options,
        true,
      ),
    uploadAttachmentPath: (sessionId, sourcePath, input, options) =>
      ipc(
        () =>
          commands.applicationUploadAttachmentSource(
            options.requestId,
            sessionId,
            input,
            { kind: "path", path: sourcePath },
          ),
        options,
        true,
      ),
    attachmentUploadStatus: (sessionId, uploadId, input, options) =>
      ipc(
        () =>
          commands.applicationAttachmentUploadStatus(
            uploadId,
            sessionId,
            input,
          ),
        options,
      ),
    openAttachment: openLocalAttachment,
    deleteRecording: (sessionId, input, options) =>
      ipc(
        () => commands.applicationDeleteRecording(sessionId, input),
        options,
        true,
      ),
    agentList: (input, options) =>
      ipc(() => commands.applicationAgentList(input), options),
    agentSearch: (input, options) =>
      ipc(() => commands.applicationAgentSearch(input), options),
    agentMeeting: (sessionId, options) =>
      ipc(() => commands.applicationAgentMeeting(sessionId), options),
    agentTranscript: (sessionId, options) =>
      ipc(() => commands.applicationAgentTranscript(sessionId), options),
    agentExport: (sessionId, options) =>
      ipc(() => commands.applicationAgentExport(sessionId), options),
    agentSegments: (sessionId, options) =>
      ipc(() => commands.applicationAgentSegments(sessionId), options),
    readMetadata: (sessionId, options) =>
      ipc(() => commands.applicationReadMetadata(sessionId), options),
    saveMetadata: (sessionId, input, options) =>
      ipc(
        () => commands.applicationSaveMetadata(sessionId, input),
        options,
        true,
      ),
    readDocument: (sessionId, target, options) =>
      ipc(() => commands.applicationReadDocument(sessionId, target), options),
    saveDocument: (sessionId, target, input, options) =>
      ipc(
        () => commands.applicationSaveDocument(sessionId, target, input),
        options,
        true,
      ),
    documents: (sessionId, options) =>
      ipc(() => commands.applicationDocuments(sessionId), options),
    transcripts: (sessionId, options) =>
      ipc(() => commands.applicationTranscripts(sessionId), options),
    audioMetadata: (sessionId, options) =>
      ipc(() => commands.applicationAudioMetadata(sessionId), options),
    attachmentInfo: (sessionId, attachmentId, options) =>
      ipc(
        () => commands.applicationAttachmentInfo(sessionId, attachmentId),
        options,
      ),
    attachmentMetadata: (sessionId, attachmentId, options) =>
      ipc(
        () => commands.applicationAttachmentMetadata(sessionId, attachmentId),
        options,
      ),
    removeAttachment: (sessionId, attachmentId, input, options) => {
      const requestId = options?.requestId ?? crypto.randomUUID();
      return ipc(
        () =>
          commands.applicationRemoveAttachment(
            requestId,
            sessionId,
            attachmentId,
            input,
          ),
        { ...options, requestId },
        true,
      );
    },
    attachments: (sessionId, limit, after, options) =>
      ipc(
        () => commands.applicationAttachments(sessionId, limit, after ?? null),
        options,
      ),
    tasks: (sessionId, options) =>
      ipc(() => commands.applicationTasks(sessionId), options),
    tasksSnapshot: (sessionId, options) =>
      ipc(() => commands.applicationTasksSnapshot(sessionId), options),
    replaceTasks: (sessionId, input, options) =>
      ipc(
        () => commands.applicationReplaceTasks(sessionId, input),
        options,
        true,
      ),
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
    readNoteEditor: (sessionId, options) =>
      ipc(() => commands.applicationReadNoteEditor(sessionId), options),
    saveNoteEditor: (sessionId, input, options) =>
      ipc(
        () => commands.applicationSaveNoteEditor(sessionId, input),
        options,
        true,
      ),
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
    attachmentPresentation: scopedAttachmentPresentation,
    attachments: (sessionId, limit, after, options) =>
      connection.request(
        {
          kind: "attachments",
          vault_id: vaultId,
          session_id: sessionId,
          limit,
          after: after ?? null,
        },
        "attachments",
        options,
      ),
    features: connection.info.capabilities.features,
    processingPolicy: (options) =>
      connection.request(
        { kind: "processing_policy", vault_id: vaultId },
        "processing_policy",
        options,
      ),
    processingSettings: (options) =>
      connection.request(
        { kind: "processing_settings", vault_id: vaultId },
        "processing_settings",
        options,
      ),
    saveProcessingSettings: (input, options) =>
      connection.request(
        { kind: "save_processing_settings", vault_id: vaultId, input },
        "processing_settings_save",
        options,
      ),
    submitSummaryJob: (input, options) =>
      connection.request(
        { kind: "submit_summary_job", vault_id: vaultId, input },
        "processing_job",
        options,
      ),
    processingJobs: (input, options) =>
      connection.request(
        { kind: "processing_jobs", vault_id: vaultId, input },
        "processing_jobs",
        options,
      ),
    processingJob: (jobId, options) =>
      connection.request(
        { kind: "processing_job", vault_id: vaultId, job_id: jobId },
        "processing_job",
        options,
      ),
    processingJobResult: (jobId, options) =>
      connection.request(
        { kind: "processing_job_result", vault_id: vaultId, job_id: jobId },
        "processing_job_result",
        options,
      ),
    cancelProcessingJob: (jobId, options) =>
      connection.request(
        { kind: "cancel_processing_job", vault_id: vaultId, job_id: jobId },
        "processing_job",
        options,
      ),
    prepareAttachmentUpload: prepareDeviceAttachmentUpload,
    prepareAttachmentSource: prepareDeviceAttachmentSource,
    uploadAttachmentSource: (sessionId, prepared, options) =>
      connection.uploadAttachmentSource(
        vaultId,
        sessionId,
        prepared.source,
        prepared.input,
        options,
      ),
    uploadAttachmentPath: (sessionId, sourcePath, input, options) =>
      connection.uploadAttachmentPath(
        vaultId,
        sessionId,
        sourcePath,
        input,
        options,
      ),
    attachmentUploadStatus: (sessionId, uploadId, input, options) =>
      connection.request(
        {
          kind: "attachment_upload_status",
          upload: {
            vault_id: vaultId,
            session_id: sessionId,
            request_id: uploadId,
            input,
          },
        },
        "attachment_upload_status",
        options,
      ),
    attachmentMetadata: (sessionId, attachmentId, options) =>
      connection.request(
        {
          kind: "attachment_metadata",
          vault_id: vaultId,
          session_id: sessionId,
          attachment_id: attachmentId,
        },
        "attachment_metadata",
        options,
      ),
    removeAttachment: (sessionId, attachmentId, input, options) =>
      connection.request(
        {
          kind: "remove_attachment",
          vault_id: vaultId,
          session_id: sessionId,
          attachment_id: attachmentId,
          input,
        },
        "attachment_removed",
        options,
      ),
    deleteRecording: (sessionId, input, options) =>
      connection.request(
        {
          kind: "delete_recording",
          vault_id: vaultId,
          session_id: sessionId,
          input,
        },
        "recording_deleted",
        options,
      ),
    openRecording: (sessionId, options) =>
      openRemoteRecording(connection, vaultId, sessionId, options),
    openAttachment: (sessionId, attachmentId, options) =>
      openRemoteAttachment(
        connection,
        vaultId,
        sessionId,
        attachmentId,
        options,
      ),
    readMetadata: (sessionId, options) =>
      connection.request(
        { kind: "metadata", vault_id: vaultId, session_id: sessionId },
        "metadata",
        options,
      ),
    saveMetadata: (sessionId, input, options) =>
      connection.request(
        {
          kind: "save_metadata",
          vault_id: vaultId,
          session_id: sessionId,
          input,
        },
        "metadata_save",
        options,
      ),
    agentList: (input, options) =>
      connection.request(
        { kind: "agent_list", vault_id: vaultId, input },
        "agent_list",
        options,
      ),
    agentSearch: (input, options) =>
      connection.request(
        { kind: "agent_search", vault_id: vaultId, input },
        "agent_search",
        options,
      ),
    agentMeeting: (sessionId, options) =>
      connection.request(
        { kind: "agent_meeting", vault_id: vaultId, session_id: sessionId },
        "agent_meeting",
        options,
      ),
    agentTranscript: (sessionId, options) =>
      connection.request(
        { kind: "agent_transcript", vault_id: vaultId, session_id: sessionId },
        "agent_transcript",
        options,
      ),
    agentExport: (sessionId, options) =>
      connection.request(
        { kind: "agent_export", vault_id: vaultId, session_id: sessionId },
        "agent_export",
        options,
      ),
    agentSegments: (sessionId, options) =>
      connection.request(
        { kind: "agent_segments", vault_id: vaultId, session_id: sessionId },
        "agent_segments",
        options,
      ),
    readDocument: (sessionId, target, options) =>
      connection.request(
        { kind: "document", vault_id: vaultId, session_id: sessionId, target },
        "document",
        options,
      ),
    saveDocument: (sessionId, target, input, options) =>
      connection.request(
        {
          kind: "save_document",
          vault_id: vaultId,
          session_id: sessionId,
          target,
          input,
        },
        "document_save",
        options,
      ),
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
    tasksSnapshot: (sessionId, options) =>
      connection.request(
        { kind: "tasks_snapshot", vault_id: vaultId, session_id: sessionId },
        "tasks_snapshot",
        options,
      ),
    audioMetadata: (sessionId, options) =>
      connection.request(
        { kind: "audio_metadata", vault_id: vaultId, session_id: sessionId },
        "audio_metadata",
        options,
      ),
    attachmentInfo: (sessionId, attachmentId, options) =>
      connection.request(
        {
          kind: "attachment_info",
          vault_id: vaultId,
          session_id: sessionId,
          attachment_id: attachmentId,
        },
        "attachment_info",
        options,
      ),
    replaceTasks: (sessionId, input, options) =>
      connection.request(
        {
          kind: "replace_tasks",
          vault_id: vaultId,
          session_id: sessionId,
          input,
        },
        "tasks_save",
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
    readNoteEditor: (sessionId, options) =>
      connection.request(
        { kind: "note_editor", vault_id: vaultId, session_id: sessionId },
        "note_editor",
        options,
      ),
    saveNoteEditor: (sessionId, input, options) =>
      connection.request(
        {
          kind: "save_note_editor",
          vault_id: vaultId,
          session_id: sessionId,
          input,
        },
        "note_editor_save",
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

function prepareDeviceAttachmentUpload(
  sourcePath: string,
  options?: RequestOptions,
) {
  return ipc(() => commands.devicePrepareAttachmentUpload(sourcePath), options);
}
