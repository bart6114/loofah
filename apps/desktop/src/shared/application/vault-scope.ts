import {
  QueryClient,
  hashKey,
  queryOptions,
  infiniteQueryOptions,
} from "@tanstack/react-query";

import { artifactInvalidation } from "./artifact-invalidation";
import type { AttachmentResource } from "./attachments";
import type { PreparedAttachmentUpload } from "./device-attachment-source";
import { documentDraftClient, documentDraftId } from "./document-draft";
import { ApplicationRequestError, connectionClosed } from "./errors";
import type { PlaybackRecording } from "./playback";
import type {
  ApplicationError,
  Change,
  NoteSnapshot,
  NoteEditorSnapshot,
  DocumentTarget,
  ListProcessingJobsInput,
  ProcessingSettingsSnapshot,
} from "./protocol.gen";
import type { VaultClient, VaultClientEvent } from "./vault-client";

import type { ConnectionState, SearchRequest } from "~/types/tauri.gen";

export class VaultScope {
  readonly queryClient: QueryClient;
  private snapshot: { state: ConnectionState; error: ApplicationError | null } =
    { state: "connecting", error: null };
  private readonly listeners = new Set<() => void>();
  private readonly changeListeners = new Set<(change: Change) => void>();
  private startPromise?: Promise<void>;
  private unsubscribe?: () => void;
  private disposed = false;
  private readonly uploadSources = new Set<PreparedAttachmentUpload>();
  private readonly uploadCaches = new Set<QueryClient>();
  private readonly resources = new Map<
    string,
    PlaybackRecording | AttachmentResource
  >();
  private readonly stopResourceCache: () => void;

  constructor(readonly client: VaultClient) {
    this.queryClient = new QueryClient({
      defaultOptions: {
        // Native requests report connectivity; pausing here would silently queue edits.
        mutations: { networkMode: "always" },
        queries: {
          networkMode: client.networkMode,
          staleTime: Infinity,
          retry: (count, error) =>
            count < 2 &&
            (!(error instanceof ApplicationRequestError) ||
              error.detail.retryable),
        },
      },
    });
    this.stopResourceCache = this.queryClient
      .getQueryCache()
      .subscribe((event) => {
        const [prefix, scope, kind] = event.query.queryKey;
        if (
          prefix !== "vault" ||
          scope !== client.scope ||
          (kind !== "audio_resource" && kind !== "attachment_resource")
        )
          return;
        if (
          event.type !== "removed" &&
          !(event.type === "updated" && event.action.type === "success")
        )
          return;
        const previous = this.resources.get(event.query.queryHash);
        const next =
          event.type === "removed"
            ? null
            : (event.query.state.data as
                | PlaybackRecording
                | AttachmentResource
                | null);
        if (previous === next) return;
        this.resources.delete(event.query.queryHash);
        if (next) this.resources.set(event.query.queryHash, next);
        if (previous) void previous.close().catch(() => {});
      });
  }

  getSnapshot = () => this.snapshot;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };
  onChange(listener: (change: Change) => void) {
    this.changeListeners.add(listener);
    return () => {
      this.changeListeners.delete(listener);
    };
  }

  start(): Promise<void> {
    if (this.disposed)
      return Promise.reject(new Error("This vault scope is closed"));
    return (this.startPromise ??= this.client
      .subscribe(this.receive)
      .then((unsubscribe) => {
        if (this.disposed) unsubscribe();
        else {
          this.unsubscribe = unsubscribe;
          window.addEventListener("focus", this.focus);
          document.addEventListener("visibilitychange", this.visible);
        }
      })
      .catch((error: unknown) => {
        this.startPromise = undefined;
        if (!this.disposed) {
          this.snapshot = {
            state: "reconnecting",
            error:
              error instanceof ApplicationRequestError ? error.detail : null,
          };
          for (const listener of this.listeners) listener();
        }
        throw error;
      }));
  }

  async run<T>(operation: () => Promise<T>): Promise<T> {
    if (this.disposed) throw connectionClosed();
    try {
      return await operation();
    } catch (error) {
      if (
        error instanceof ApplicationRequestError &&
        ["unauthenticated", "forbidden"].includes(error.detail.code)
      ) {
        this.receive({
          kind: "connection",
          state:
            error.detail.code === "unauthenticated"
              ? "login_required"
              : "access_denied",
          error: error.detail,
        });
      }
      throw error;
    }
  }

  async prepareAttachmentSource(
    candidate: Parameters<VaultClient["prepareAttachmentSource"]>[0],
  ) {
    return this.runUpload(async (signal) => {
      const prepared = await this.client.prepareAttachmentSource(candidate, {
        signal,
      });
      if (signal.aborted) {
        await prepared.close().catch(() => {});
        throw connectionClosed();
      }
      let closing: Promise<void> | undefined;
      const owned: PreparedAttachmentUpload = {
        ...prepared,
        close: () =>
          (closing ??= (async () => {
            this.uploadSources.delete(owned);
            await prepared.close();
          })()),
      };
      this.uploadSources.add(owned);
      return owned;
    }, false);
  }

  uploadAttachmentSource(
    sessionId: string,
    prepared: PreparedAttachmentUpload,
    requestId: string,
  ) {
    return this.runUpload(
      (signal) =>
        this.client.uploadAttachmentSource(sessionId, prepared, {
          signal,
          requestId,
        }),
      true,
    );
  }

  private async runUpload<T>(
    operation: (signal: AbortSignal) => Promise<T>,
    mutation: boolean,
  ): Promise<T> {
    if (
      this.disposed ||
      this.snapshot.state !== "connected" ||
      !this.client.canEdit ||
      !this.client.features.includes("attachment_upload")
    ) {
      throw new ApplicationRequestError({
        code: "unavailable",
        message:
          "Attachment upload is unavailable; reconnect before trying again",
        request_id: null,
        retryable: false,
        current_revision: null,
        mutation_outcome: "not_committed",
      });
    }
    const controller = new AbortController();
    const stop = this.subscribe(() => {
      if (
        ["access_denied", "login_required", "closed", "incompatible"].includes(
          this.snapshot.state,
        )
      )
        controller.abort();
    });
    try {
      const result = await this.run(() => operation(controller.signal));
      if (controller.signal.aborted)
        throw new ApplicationRequestError({
          code: "cancelled",
          message: "Attachment access ended while the request was running",
          request_id: null,
          retryable: false,
          current_revision: null,
          mutation_outcome: mutation ? "unknown" : "not_committed",
        });
      return result;
    } finally {
      stop();
    }
  }

  private closeUploadSources() {
    for (const source of this.uploadSources)
      void source.close().catch(() => {});
    this.uploadSources.clear();
    for (const client of this.uploadCaches)
      for (const mutation of client.getMutationCache().findAll({
        mutationKey: ["vault", this.client.scope, "attachment_upload"],
      }))
        client.getMutationCache().remove(mutation);
    this.uploadCaches.clear();
  }

  retainUploadCache(client: QueryClient) {
    this.uploadCaches.add(client);
  }

  async saveAndConfirm<T>(
    key: readonly unknown[],
    operation: () => Promise<T>,
    confirm: (value: T) => Promise<void>,
  ): Promise<T> {
    const initial = this.queryClient.getQueryState(key);
    let refresh =
      !initial || initial.isInvalidated || initial.fetchStatus !== "idle";
    const hash = hashKey(key);
    const stop = this.queryClient.getQueryCache().subscribe((event) => {
      if (event.query.queryHash !== hash) return;
      if (
        event.type === "removed" ||
        (event.type === "updated" &&
          (event.action.type === "invalidate" ||
            event.action.type === "fetch" ||
            (event.action.type === "success" && !event.action.manual)))
      )
        refresh = true;
    });
    try {
      const result = await this.run(operation);
      await confirm(result);
      // A newer canonical read/event can precede an older, delayed save reply.
      if (
        refresh &&
        !this.disposed &&
        !["access_denied", "login_required", "closed", "incompatible"].includes(
          this.snapshot.state,
        )
      ) {
        void this.queryClient.invalidateQueries({ queryKey: key, exact: true });
      }
      return result;
    } finally {
      stop();
    }
  }

  metadataOptions(sessionId: string) {
    return queryOptions({
      queryKey: ["vault", this.client.scope, "metadata", sessionId],
      queryFn: ({ signal }) =>
        this.run(() => this.client.readMetadata(sessionId, { signal })),
    });
  }

  noteOptions(sessionId: string) {
    const client = this.client;
    return queryOptions({
      queryKey: ["vault", client.scope, "note", sessionId],
      queryFn: ({ signal }) =>
        this.run(() => client.readNote(sessionId, { signal })),
    });
  }

  noteEditorOptions(sessionId: string) {
    return queryOptions({
      queryKey: ["vault", this.client.scope, "note_editor", sessionId],
      queryFn: ({ signal }) =>
        this.run(() => this.client.readNoteEditor(sessionId, { signal })),
    });
  }
  audioMetadataOptions(sessionId: string) {
    return queryOptions({
      queryKey: ["vault", this.client.scope, "audio_metadata", sessionId],
      queryFn: ({ signal }) =>
        this.run(() => this.client.audioMetadata(sessionId, { signal })),
    });
  }
  attachmentInfoOptions(sessionId: string, attachmentId: string) {
    return queryOptions({
      queryKey: [
        "vault",
        this.client.scope,
        "attachment_info",
        sessionId,
        attachmentId,
      ],
      queryFn: ({ signal }) =>
        this.run(() =>
          this.client.attachmentInfo(sessionId, attachmentId, { signal }),
        ),
    });
  }
  attachmentMetadataOptions(sessionId: string, attachmentId: string) {
    return queryOptions({
      queryKey: [
        "vault",
        this.client.scope,
        "attachment_metadata",
        sessionId,
        attachmentId,
      ],
      queryFn: ({ signal }) =>
        this.run(() =>
          this.client.attachmentMetadata(sessionId, attachmentId, { signal }),
        ),
    });
  }
  attachmentsOptions(sessionId: string) {
    return infiniteQueryOptions({
      queryKey: ["vault", this.client.scope, "attachment_list", sessionId],
      initialPageParam: undefined as string | undefined,
      queryFn: ({ pageParam, signal }) =>
        this.run(() =>
          this.client.attachments(sessionId, 100, pageParam, { signal }),
        ),
      getNextPageParam: (page) => page.next_cursor ?? undefined,
    });
  }
  attachmentOptions(sessionId: string, attachmentId: string) {
    return queryOptions({
      queryKey: [
        "vault",
        this.client.scope,
        "attachment_resource",
        sessionId,
        attachmentId,
      ],
      gcTime: 0,
      structuralSharing: false,
      queryFn: ({ signal }) =>
        this.run(async () => {
          const attachment = await this.client.openAttachment(
            sessionId,
            attachmentId,
            { signal },
          );
          if (signal.aborted || this.disposed) {
            await attachment?.close().catch(() => {});
            throw connectionClosed();
          }
          return attachment;
        }),
    });
  }
  recordingOptions(sessionId: string) {
    return queryOptions({
      queryKey: ["vault", this.client.scope, "audio_resource", sessionId],
      gcTime: 0,
      structuralSharing: false,
      queryFn: ({ signal }) =>
        this.run(async () => {
          const recording = await this.client.openRecording(sessionId, {
            signal,
          });
          if (signal.aborted || this.disposed) {
            await recording?.close().catch(() => {});
            throw connectionClosed();
          }
          return recording;
        }),
    });
  }
  recordingPeaksOptions(
    sessionId: string,
    recording: PlaybackRecording | null,
  ) {
    return queryOptions({
      queryKey: [
        "vault",
        this.client.scope,
        "audio_peaks",
        sessionId,
        recording?.id ?? null,
      ],
      gcTime: 0,
      retry: false,
      queryFn: ({ signal }) =>
        this.run(() =>
          recording ? recording.peaks({ signal }) : Promise.resolve(null),
        ),
    });
  }
  sessionOptions(sessionId: string) {
    const client = this.client;
    return queryOptions({
      queryKey: ["vault", client.scope, "session", sessionId],
      queryFn: ({ signal }) =>
        this.run(() => client.session(sessionId, { signal })),
    });
  }
  documentsOptions(sessionId: string) {
    return queryOptions({
      queryKey: ["vault", this.client.scope, "docs", sessionId],
      queryFn: ({ signal }) =>
        this.run(() => this.client.documents(sessionId, { signal })),
    });
  }
  documentOptions(sessionId: string, target: DocumentTarget) {
    return queryOptions({
      queryKey: ["vault", this.client.scope, "document", sessionId, target],
      queryFn: ({ signal }) =>
        this.run(() => this.client.readDocument(sessionId, target, { signal })),
    });
  }
  documentTextOptions(sessionId: string, target: DocumentTarget) {
    const client = documentDraftClient(this.client, sessionId, target);
    return queryOptions({
      queryKey: [
        "vault",
        this.client.scope,
        "document_text",
        sessionId,
        documentDraftId(sessionId, target),
      ],
      queryFn: ({ signal }) =>
        this.run(() =>
          client.readNote(documentDraftId(sessionId, target), { signal }),
        ),
    });
  }
  transcriptsOptions(sessionId: string) {
    return queryOptions({
      queryKey: ["vault", this.client.scope, "transcripts", sessionId],
      queryFn: ({ signal }) =>
        this.run(() => this.client.transcripts(sessionId, { signal })),
    });
  }
  tasksOptions(sessionId: string) {
    return queryOptions({
      queryKey: ["vault", this.client.scope, "tasks", sessionId],
      queryFn: ({ signal }) =>
        this.run(() => this.client.tasks(sessionId, { signal })),
    });
  }
  tasksSnapshotOptions(sessionId: string) {
    return queryOptions({
      queryKey: ["vault", this.client.scope, "tasks_snapshot", sessionId],
      queryFn: ({ signal }) =>
        this.run(() => this.client.tasksSnapshot(sessionId, { signal })),
    });
  }
  peopleOptions() {
    return queryOptions({
      queryKey: ["vault", this.client.scope, "people"],
      queryFn: ({ signal }) => this.run(() => this.client.people({ signal })),
    });
  }
  tagsOptions() {
    return queryOptions({
      queryKey: ["vault", this.client.scope, "tags"],
      queryFn: ({ signal }) => this.run(() => this.client.tags({ signal })),
    });
  }
  processingJobsOptions(input: ListProcessingJobsInput) {
    return queryOptions({
      queryKey: ["vault", this.client.scope, "processing_jobs", input],
      queryFn: ({ signal }) =>
        this.run(() => this.client.processingJobs(input, { signal })),
    });
  }
  processingPolicyOptions() {
    return queryOptions({
      queryKey: ["vault", this.client.scope, "processing_policy"],
      queryFn: ({ signal }) =>
        this.run(() => this.client.processingPolicy({ signal })),
    });
  }
  processingSettingsOptions() {
    return queryOptions({
      queryKey: ["vault", this.client.scope, "processing_settings"],
      queryFn: ({ signal }) =>
        this.run(() => this.client.processingSettings({ signal })),
    });
  }
  processingJobOptions(jobId: string) {
    return queryOptions({
      queryKey: ["vault", this.client.scope, "processing_job", jobId],
      queryFn: ({ signal }) =>
        this.run(() => this.client.processingJob(jobId, { signal })),
    });
  }
  processingJobResultOptions(jobId: string) {
    return queryOptions({
      queryKey: ["vault", this.client.scope, "processing_job_result", jobId],
      queryFn: ({ signal }) =>
        this.run(() => this.client.processingJobResult(jobId, { signal })),
    });
  }
  pageOptions(limit: number, after: string | null = null) {
    const client = this.client;
    return queryOptions({
      queryKey: ["vault", client.scope, "session_page", limit, after],
      queryFn: ({ signal }) =>
        this.run(() => client.sessions(limit, after, { signal })),
    });
  }
  searchOptions(input: SearchRequest) {
    const client = this.client;
    return queryOptions({
      queryKey: ["vault", client.scope, "search", input],
      queryFn: ({ signal }) => this.run(() => client.search(input, { signal })),
    });
  }

  async confirmNote(sessionId: string, note: NoteSnapshot) {
    if (
      !(await this.confirmSnapshot(this.noteOptions(sessionId).queryKey, note))
    )
      return;
    void this.queryClient.invalidateQueries({
      queryKey: ["vault", this.client.scope, "session_page"],
    });
    void this.queryClient.invalidateQueries({
      queryKey: ["vault", this.client.scope, "session", sessionId],
    });
  }

  async confirmProcessingSettings(snapshot: ProcessingSettingsSnapshot) {
    return this.confirmSnapshot(
      this.processingSettingsOptions().queryKey,
      snapshot,
    );
  }

  async confirmNoteEditor(sessionId: string, snapshot: NoteEditorSnapshot) {
    if (
      !(await this.confirmSnapshot(
        this.noteEditorOptions(sessionId).queryKey,
        snapshot,
      ))
    )
      return;
    for (const kind of [
      "note",
      "metadata",
      "tasks",
      "tasks_snapshot",
      "session",
    ]) {
      void this.queryClient.invalidateQueries({
        queryKey: ["vault", this.client.scope, kind, sessionId],
      });
    }
    void this.queryClient.invalidateQueries({
      queryKey: ["vault", this.client.scope, "session_page"],
    });
  }

  async confirmDocumentText(
    sessionId: string,
    target: DocumentTarget,
    note: NoteSnapshot,
  ) {
    if (
      !(await this.confirmSnapshot(
        this.documentTextOptions(sessionId, target).queryKey,
        note,
      ))
    )
      return;
    await this.queryClient.cancelQueries({
      queryKey: this.documentOptions(sessionId, target).queryKey,
    });
    for (const kind of ["document", "docs", "tasks"]) {
      void this.queryClient.invalidateQueries({
        queryKey: ["vault", this.client.scope, kind, sessionId],
      });
    }
  }

  private async confirmSnapshot<T>(key: readonly unknown[], value: T) {
    await this.queryClient.cancelQueries({ queryKey: key, exact: true });
    if (
      this.disposed ||
      ["access_denied", "login_required", "closed", "incompatible"].includes(
        this.snapshot.state,
      )
    )
      return false;
    this.queryClient.setQueryData(key, value);
    return true;
  }

  resync() {
    if (!this.disposed)
      void this.queryClient.invalidateQueries({
        queryKey: ["vault", this.client.scope],
      });
  }

  dispose() {
    if (this.disposed) return;
    this.disposed = true;
    this.closeUploadSources();
    this.unsubscribe?.();
    window.removeEventListener("focus", this.focus);
    document.removeEventListener("visibilitychange", this.visible);
    void this.queryClient.cancelQueries();
    this.queryClient.clear();
    this.stopResourceCache();
    this.snapshot = { state: "closed", error: null };
    for (const listener of this.listeners) listener();
    this.listeners.clear();
    this.changeListeners.clear();
  }

  private focus = () => {
    if (["connected", "reconnecting"].includes(this.snapshot.state))
      this.resync();
  };
  private visible = () => {
    if (document.visibilityState === "visible") this.focus();
  };

  private receive = (event: VaultClientEvent) => {
    if (this.disposed) return;
    if (event.kind === "connection") {
      const previous = this.snapshot.state;
      this.snapshot = { state: event.state, error: event.error };
      if (
        ["access_denied", "login_required", "closed", "incompatible"].includes(
          event.state,
        )
      ) {
        this.closeUploadSources();
        void this.queryClient.cancelQueries();
        this.queryClient.clear();
      } else if (event.state === "connected" && previous === "reconnecting")
        this.resync();
      for (const listener of this.listeners) listener();
      return;
    }
    const change = event.change;
    for (const listener of this.changeListeners) listener(change);
    if (change.kind === "resync_required" || change.kind === "unknown")
      this.resync();
    else if (change.kind === "processing_settings_changed") {
      void this.queryClient.invalidateQueries({
        queryKey: ["vault", this.client.scope, "processing_settings"],
      });
    } else if (change.kind === "job_changed") {
      void this.queryClient.invalidateQueries({
        predicate: (query) => {
          const [prefix, scope, kind, jobId] = query.queryKey;
          return (
            prefix === "vault" &&
            scope === this.client.scope &&
            (kind === "processing_jobs" ||
              (["processing_job", "processing_job_result"].includes(
                String(kind),
              ) &&
                jobId === change.job_id))
          );
        },
      });
    } else if (change.kind === "search_ready") {
      void this.queryClient.invalidateQueries({
        queryKey: ["vault", this.client.scope, "search"],
      });
    } else if (change.kind === "index_changed") {
      const ids = new Set(change.ids);
      const all = ids.size === 0;
      const assetChanged = artifactInvalidation(change);
      void this.queryClient.invalidateQueries({
        predicate: (query) => {
          const [prefix, scope, kind, id, attachmentId] = query.queryKey;
          if (prefix !== "vault" || scope !== this.client.scope) return false;
          if (
            (change.entity === "people" || change.entity === "tags") &&
            kind === change.entity
          )
            return true;
          if (kind === "session_page")
            return ["sessions", "session_headers"].includes(change.entity);
          if (
            ["audio_metadata", "audio_resource", "audio_peaks"].includes(
              String(kind),
            )
          )
            return assetChanged("recording", String(id));
          if (
            [
              "attachment_info",
              "attachment_metadata",
              "attachment_resource",
              "attachment_list",
            ].includes(String(kind))
          )
            return assetChanged(
              "attachments",
              String(id),
              typeof attachmentId === "string" ? attachmentId : undefined,
            );
          if (kind === "tasks_snapshot")
            return (
              ["tasks", "sessions", "session_headers"].includes(
                change.entity,
              ) &&
              (all || ids.has(String(id)))
            );
          if (kind === "metadata")
            return (
              ["sessions", "session_headers"].includes(change.entity) &&
              (all || ids.has(String(id)))
            );
          if (kind === "note_editor")
            return (
              ["sessions", "tasks"].includes(change.entity) &&
              (all || ids.has(String(id)))
            );
          if (kind === "note" || kind === "session")
            return (
              ["sessions", "session_headers", "tasks"].includes(
                change.entity,
              ) &&
              (all || ids.has(String(id)))
            );
          if (kind === "document" || kind === "document_text")
            return (
              ["docs", "sessions", "session_headers"].includes(change.entity) &&
              (all || ids.has(String(id)))
            );
          return kind === change.entity && (all || ids.has(String(id)));
        },
      });
    }
  };
}
