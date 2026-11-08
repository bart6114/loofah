import { Channel } from "@tauri-apps/api/core";

import {
  applicationResult,
  ApplicationRequestError,
  connectionClosed,
} from "./errors";
import type {
  ApplicationError,
  FileMetadata,
  UploadAttachmentRequest,
  MediaDeclaration,
  MediaRequestStatus,
} from "./protocol.gen";
import type { RequestOptions } from "./vault-client";

import {
  commands,
  type Me,
  type RemoteConnectionInfo,
  type RemoteEvent,
  type RemoteOperation,
  type RemoteReply,
  type NativeRecording,
  type NativeAttachment,
  type NativeAttachmentCopy,
  type AttachmentCopyAction,
  type RecordingWaveform,
  type DeviceAttachmentSource,
  type CaptureLiveBinding,
  type CaptureLiveEvent,
  type MediaQuery,
  type MediaReply,
  type MediaSubmission,
} from "~/types/tauri.gen";

type Reply<K extends RemoteReply["kind"]> = Extract<
  RemoteReply,
  { kind: K }
>["value"];

type MediaValue<K extends MediaReply["kind"]> = Extract<
  MediaReply,
  { kind: K }
>["value"];

export class NativeRemoteConnection {
  private closed = false;
  private signedOut = false;
  private disconnectPromise?: Promise<void>;
  private readonly subscriptions = new Set<() => Promise<void>>();
  private readonly recordings = new Set<string>();
  private readonly closingRecordings = new Map<string, Promise<void>>();
  private readonly attachments = new Map<
    string,
    { vaultId: string; sessionId: string; attachmentId: string }
  >();
  private readonly closingAttachments = new Map<string, Promise<void>>();

  private constructor(readonly info: RemoteConnectionInfo) {}

  static async connect(server: string): Promise<NativeRemoteConnection> {
    return new NativeRemoteConnection(
      applicationResult(await commands.remoteConnect(server)),
    );
  }

  static async connectDesktopProfile(
    name: string,
    profileId: string,
  ): Promise<NativeRemoteConnection> {
    return new NativeRemoteConnection(
      applicationResult(
        await commands.remoteConnectDesktopProfile(name, profileId),
      ),
    );
  }

  async signIn(): Promise<Me> {
    this.assertOpen();
    return applicationResult(
      await commands.remoteLogin(this.info.connection_id),
    );
  }

  async cancelSignIn(): Promise<void> {
    await commands.remoteCancelLogin(this.info.connection_id);
  }

  restore(): Promise<Me> {
    return this.request({ kind: "provision" }, "me");
  }

  async uploadAttachmentPath(
    vaultId: string,
    sessionId: string,
    sourcePath: string,
    input: UploadAttachmentRequest,
    options: RequestOptions & { requestId: string },
  ): Promise<FileMetadata> {
    return this.uploadAttachmentSource(
      vaultId,
      sessionId,
      { kind: "path", path: sourcePath },
      input,
      options,
    );
  }

  async uploadAttachmentSource(
    vaultId: string,
    sessionId: string,
    source: DeviceAttachmentSource,
    input: UploadAttachmentRequest,
    options: RequestOptions & { requestId: string },
  ): Promise<FileMetadata> {
    this.assertOpen();
    const { signal, requestId } = options;
    signal?.throwIfAborted();
    const cancel = () => {
      void commands
        .remoteCancelRequest(this.info.connection_id, requestId)
        .catch(() => {});
    };
    signal?.addEventListener("abort", cancel, { once: true });
    try {
      const metadata = applicationResult(
        await commands.remoteUploadAttachmentSource(
          this.info.connection_id,
          {
            vault_id: vaultId,
            session_id: sessionId,
            request_id: requestId,
            input,
          },
          source,
        ),
      );
      if (this.closed || signal?.aborted) {
        throw new ApplicationRequestError({
          code: "cancelled",
          message:
            "The upload was cancelled after being sent; check its status before retrying",
          request_id: requestId,
          retryable: false,
          current_revision: null,
          mutation_outcome: "unknown",
        });
      }
      return metadata;
    } finally {
      signal?.removeEventListener("abort", cancel);
    }
  }

  async mediaRequest<K extends MediaReply["kind"]>(
    vaultId: string,
    query: MediaQuery,
    expected: K,
    options?: RequestOptions,
  ): Promise<MediaValue<K>> {
    const requestId = options?.requestId ?? crypto.randomUUID();
    const reply = await this.mediaAction(requestId, options, false, async () =>
      applicationResult(
        await commands.remoteMediaRequest(
          this.info.connection_id,
          vaultId,
          requestId,
          query,
        ),
      ),
    );
    if (reply.kind !== expected)
      throw new ApplicationRequestError({
        code: "incompatible_version",
        message: "The native host returned an unexpected media reply",
        request_id: requestId,
        retryable: false,
        current_revision: null,
        mutation_outcome: null,
      });
    return reply.value as MediaValue<K>;
  }

  submitMedia(
    vaultId: string,
    declaration: MediaDeclaration,
    source: DeviceAttachmentSource | null,
    options: RequestOptions & { requestId: string },
  ): Promise<MediaRequestStatus> {
    if (declaration.request_id !== options.requestId)
      return Promise.reject(
        new ApplicationRequestError({
          code: "validation",
          message: "The original media request ID changed",
          request_id: options.requestId,
          retryable: false,
          current_revision: null,
          mutation_outcome: "not_committed",
        }),
      );
    return this.mediaAction(options.requestId, options, true, async () =>
      applicationResult(
        await commands.remoteMediaSubmit(
          this.info.connection_id,
          vaultId,
          declaration,
          source,
        ),
      ),
    );
  }

  recoverMedia(
    vaultId: string,
    requestId: string,
    options?: RequestOptions,
  ): Promise<MediaRequestStatus> {
    return this.mediaAction(requestId, options, true, async () =>
      applicationResult(
        await commands.remoteMediaRecover(
          this.info.connection_id,
          vaultId,
          requestId,
        ),
      ),
    );
  }

  cancelMedia(
    vaultId: string,
    requestId: string,
    options?: RequestOptions,
  ): Promise<MediaRequestStatus> {
    return this.mediaAction(requestId, options, true, async () =>
      applicationResult(
        await commands.remoteMediaCancel(
          this.info.connection_id,
          vaultId,
          requestId,
        ),
      ),
    );
  }

  resumeMediaSource(
    vaultId: string,
    requestId: string,
    source: DeviceAttachmentSource,
    options?: RequestOptions,
  ): Promise<MediaRequestStatus> {
    return this.mediaAction(requestId, options, true, async () =>
      applicationResult(
        await commands.remoteMediaResumeSource(
          this.info.connection_id,
          vaultId,
          requestId,
          source,
        ),
      ),
    );
  }

  mediaSubmissions(
    vaultId: string,
    options?: RequestOptions,
  ): Promise<MediaSubmission[]> {
    return this.mediaAction(
      options?.requestId ?? crypto.randomUUID(),
      options,
      false,
      async () =>
        applicationResult(
          await commands.remoteMediaSubmissions(
            this.info.connection_id,
            vaultId,
          ),
        ),
    );
  }

  exportMediaSource(
    vaultId: string,
    requestId: string,
    options?: RequestOptions,
  ) {
    if (!this.info.capabilities.features.includes("media_source_export"))
      return Promise.reject(
        new ApplicationRequestError({
          code: "incompatible_version",
          message: "This server does not support original media source export",
          request_id: requestId,
          retryable: false,
          current_revision: null,
          mutation_outcome: null,
        }),
      );
    return this.mediaAction(requestId, options, false, async () =>
      applicationResult(
        await commands.remoteMediaExportSource(
          this.info.connection_id,
          vaultId,
          requestId,
        ),
      ),
    );
  }

  private async mediaAction<T>(
    requestId: string,
    options: RequestOptions | undefined,
    mutation: boolean,
    run: () => Promise<T>,
  ): Promise<T> {
    this.assertOpen();
    if (!this.info.capabilities.features.includes("media_jobs"))
      throw new ApplicationRequestError({
        code: "incompatible_version",
        message: "This server does not support media jobs",
        request_id: requestId,
        retryable: false,
        current_revision: null,
        mutation_outcome: mutation ? "not_committed" : null,
      });
    const signal = options?.signal;
    signal?.throwIfAborted();
    const cancel = () => {
      void commands
        .remoteCancelRequest(this.info.connection_id, requestId)
        .catch(() => {});
    };
    signal?.addEventListener("abort", cancel, { once: true });
    try {
      const result = await run();
      if (this.closed || signal?.aborted)
        throw new ApplicationRequestError({
          code: "cancelled",
          message: mutation
            ? "Media request may have been admitted; inspect or explicitly recover its original request"
            : "Media request cancelled",
          request_id: requestId,
          retryable: false,
          current_revision: null,
          mutation_outcome: mutation ? "unknown" : null,
        });
      return result;
    } finally {
      signal?.removeEventListener("abort", cancel);
    }
  }

  async openAttachment(
    vaultId: string,
    sessionId: string,
    attachmentId: string,
    options?: RequestOptions,
  ): Promise<NativeAttachment> {
    this.assertOpen();
    const signal = options?.signal;
    signal?.throwIfAborted();
    const id = crypto.randomUUID();
    this.attachments.set(id, { vaultId, sessionId, attachmentId });
    const cancel = () => {
      void this.closeAttachment(id).catch(() => {});
    };
    signal?.addEventListener("abort", cancel, { once: true });
    try {
      const resource = applicationResult(
        await commands.remoteAttachmentOpen(this.info.connection_id, id, {
          vault_id: vaultId,
          session_id: sessionId,
          attachment_id: attachmentId,
        }),
      );
      if (
        this.closed ||
        signal?.aborted ||
        !this.attachments.has(id) ||
        this.closingAttachments.has(id)
      )
        throw connectionClosed();
      if (
        resource.id !== id ||
        !Number.isSafeInteger(resource.preview_limit_bytes) ||
        resource.preview_limit_bytes <= 0
      ) {
        throw new ApplicationRequestError({
          code: "incompatible_version",
          message: "The native host returned an unexpected attachment",
          request_id: null,
          retryable: false,
          current_revision: null,
          mutation_outcome: null,
        });
      }
      return resource;
    } catch (error) {
      await this.closeAttachment(id).catch(() => {});
      throw error;
    } finally {
      signal?.removeEventListener("abort", cancel);
    }
  }

  async copyAttachment(
    id: string,
    action: AttachmentCopyAction,
    options?: RequestOptions,
  ): Promise<NativeAttachmentCopy> {
    this.assertOpen();
    const target = this.attachments.get(id);
    if (!target || this.closingAttachments.has(id)) throw connectionClosed();
    const signal = options?.signal;
    signal?.throwIfAborted();
    const requestId = options?.requestId ?? crypto.randomUUID();
    const cancel = async () => {
      try {
        await commands.remoteCancelRequest(this.info.connection_id, requestId);
      } catch {}
    };
    signal?.addEventListener("abort", cancel, { once: true });
    try {
      const copy = applicationResult(
        await commands.remoteAttachmentCopy(
          this.info.connection_id,
          id,
          requestId,
          action,
        ),
      );
      if (
        this.closed ||
        signal?.aborted ||
        !this.attachments.has(id) ||
        this.closingAttachments.has(id)
      ) {
        throw new ApplicationRequestError({
          code: "cancelled",
          message: "The attachment action was cancelled after being sent",
          request_id: requestId,
          retryable: false,
          current_revision: null,
          mutation_outcome: "unknown",
        });
      }
      if (
        copy.metadata.filename !== target.attachmentId ||
        !copy.copied_filename ||
        /[/\\]/.test(copy.copied_filename) ||
        copy.copied_filename.includes("\0")
      ) {
        throw new ApplicationRequestError({
          code: "incompatible_version",
          message: "The native host returned an unexpected attachment copy",
          request_id: requestId,
          retryable: false,
          current_revision: null,
          mutation_outcome: "unknown",
        });
      }
      return copy;
    } finally {
      signal?.removeEventListener("abort", cancel);
    }
  }

  closeAttachment(id: string): Promise<void> {
    const closing = this.closingAttachments.get(id);
    if (closing) return closing;
    if (!this.attachments.delete(id)) return Promise.resolve();
    const result = (async () => {
      try {
        await commands.remoteAttachmentClose(this.info.connection_id, id);
      } finally {
        this.closingAttachments.delete(id);
      }
    })();
    this.closingAttachments.set(id, result);
    return result;
  }

  private async closeAttachments(vaultId?: string) {
    await Promise.allSettled([
      ...this.closingAttachments.values(),
      ...[...this.attachments]
        .filter(
          ([, target]) => vaultId === undefined || target.vaultId === vaultId,
        )
        .map(([id]) => this.closeAttachment(id)),
    ]);
  }

  async openRecording(
    vaultId: string,
    sessionId: string,
    options?: RequestOptions,
  ): Promise<NativeRecording | null> {
    this.assertOpen();
    const signal = options?.signal;
    signal?.throwIfAborted();
    const id = crypto.randomUUID();
    this.recordings.add(id);
    const cancel = () => {
      void this.closeRecording(id).catch(() => {});
    };
    signal?.addEventListener("abort", cancel, { once: true });
    try {
      const recording = applicationResult(
        await commands.remoteAudioOpen(
          this.info.connection_id,
          id,
          vaultId,
          sessionId,
        ),
      );
      if (
        this.closed ||
        signal?.aborted ||
        !this.recordings.has(id) ||
        this.closingRecordings.has(id)
      )
        throw connectionClosed();
      if (!recording) {
        await this.closeRecording(id);
        return null;
      }
      if (recording.id !== id)
        throw new ApplicationRequestError({
          code: "incompatible_version",
          message: "The native host returned an unexpected recording",
          request_id: null,
          retryable: false,
          current_revision: null,
          mutation_outcome: null,
        });
      return recording;
    } catch (error) {
      await this.closeRecording(id).catch(() => {});
      throw error;
    } finally {
      signal?.removeEventListener("abort", cancel);
    }
  }

  async recordingWaveform(
    id: string,
    options?: RequestOptions,
  ): Promise<RecordingWaveform> {
    this.assertOpen();
    if (!this.recordings.has(id) || this.closingRecordings.has(id))
      throw connectionClosed();
    const signal = options?.signal;
    signal?.throwIfAborted();
    const requestId = options?.requestId ?? crypto.randomUUID();
    const cancel = async () => {
      try {
        await commands.remoteCancelRequest(this.info.connection_id, requestId);
      } catch {}
    };
    signal?.addEventListener("abort", cancel, { once: true });
    try {
      const result = applicationResult(
        await commands.remoteAudioPeaks(this.info.connection_id, id, requestId),
      );
      if (
        this.closed ||
        signal?.aborted ||
        !this.recordings.has(id) ||
        this.closingRecordings.has(id)
      )
        throw connectionClosed();
      return result;
    } finally {
      signal?.removeEventListener("abort", cancel);
    }
  }

  closeRecording(id: string): Promise<void> {
    if (!this.recordings.has(id)) return Promise.resolve();
    const pending = this.closingRecordings.get(id);
    if (pending) return pending;
    const closing = (async () => {
      await commands.remoteAudioClose(this.info.connection_id, id);
      this.recordings.delete(id);
    })().finally(() => {
      if (this.closingRecordings.get(id) === closing)
        this.closingRecordings.delete(id);
    });
    this.closingRecordings.set(id, closing);
    return closing;
  }

  async request<K extends RemoteReply["kind"]>(
    operation: RemoteOperation,
    expected: K,
    options?: { signal?: AbortSignal; requestId?: string },
  ): Promise<Reply<K>> {
    this.assertOpen();
    const requestId = options?.requestId ?? crypto.randomUUID();
    const signal = options?.signal;
    signal?.throwIfAborted();
    const cancel = async () => {
      try {
        await commands.remoteCancelRequest(this.info.connection_id, requestId);
      } catch {}
    };
    signal?.addEventListener("abort", cancel, { once: true });
    try {
      const result = applicationResult(
        await commands.remoteRequest(
          this.info.connection_id,
          requestId,
          operation,
        ),
      );
      if (result.kind !== expected) {
        throw new ApplicationRequestError({
          code: "incompatible_version",
          message: "The server returned an unexpected response",
          retryable: false,
          request_id: requestId,
          current_revision: null,
          mutation_outcome: "unknown",
        });
      }
      if (this.closed || signal?.aborted) {
        throw new ApplicationRequestError({
          code: "cancelled",
          message: "The request was cancelled after being sent",
          retryable: false,
          request_id: requestId,
          current_revision: null,
          mutation_outcome: "unknown",
        });
      }
      return result.value as Reply<K>;
    } finally {
      signal?.removeEventListener("abort", cancel);
    }
  }

  subscribe(
    vaultId: string,
    listener: (event: RemoteEvent) => void,
  ): Promise<() => Promise<void>> {
    return this.subscribeChannel(vaultId, listener, (id, channel) =>
      commands.remoteSubscribe(this.info.connection_id, vaultId, id, channel),
    );
  }

  subscribeCaptureLive(
    vaultId: string,
    binding: CaptureLiveBinding,
    after: number | null,
    listener: (event: CaptureLiveEvent) => void,
  ): Promise<() => Promise<void>> {
    return this.subscribeChannel(vaultId, listener, (id, channel) =>
      commands.remoteSubscribeCaptureLive(
        this.info.connection_id,
        vaultId,
        id,
        binding,
        after,
        channel,
      ),
    );
  }

  private async subscribeChannel<T extends RemoteEvent | CaptureLiveEvent>(
    vaultId: string,
    listener: (event: T) => void,
    register: (
      id: string,
      channel: Channel<T>,
    ) => Promise<
      | { status: "ok"; data: null }
      | { status: "error"; error: ApplicationError }
    >,
  ): Promise<() => Promise<void>> {
    this.assertOpen();
    const subscriptionId = crypto.randomUUID();
    const channel = new Channel<T>();
    let active = true;
    channel.onmessage = (event) => {
      if (
        active &&
        !this.closed &&
        event.kind === "connection" &&
        (event.state === "access_denied" || event.state === "login_required")
      ) {
        void this.closeAttachments(
          event.state === "login_required" ? undefined : vaultId,
        );
      }
      if (active && !this.closed) listener(event);
    };
    const unsubscribe = async () => {
      if (!active) return;
      active = false;
      this.subscriptions.delete(unsubscribe);
      try {
        await commands.remoteUnsubscribe(
          this.info.connection_id,
          subscriptionId,
        );
      } catch {}
    };
    this.subscriptions.add(unsubscribe);
    try {
      applicationResult(await register(subscriptionId, channel));
      if (this.closed) await unsubscribe();
      return unsubscribe;
    } catch (error) {
      await unsubscribe();
      throw error;
    }
  }

  async disconnect(): Promise<void> {
    if (this.signedOut) return;
    return (this.disconnectPromise ??= (async () => {
      await this.closeSubscriptions();
      await commands.remoteDisconnect(this.info.connection_id);
    })());
  }

  async signOut(): Promise<void> {
    if (this.signedOut) return;
    await this.closeSubscriptions();
    applicationResult(await commands.remoteLogout(this.info.connection_id));
    this.signedOut = true;
  }

  private async closeSubscriptions() {
    this.closed = true;
    await Promise.allSettled([
      ...[...this.subscriptions].map((unsubscribe) => unsubscribe()),
      ...[...this.recordings].map((id) => this.closeRecording(id)),
      this.closeAttachments(),
    ]);
  }

  private assertOpen() {
    if (this.closed) throw connectionClosed();
  }
}
