import { Channel } from "@tauri-apps/api/core";

import {
  applicationResult,
  ApplicationRequestError,
  connectionClosed,
} from "./errors";
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
} from "~/types/tauri.gen";

type Reply<K extends RemoteReply["kind"]> = Extract<
  RemoteReply,
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

  async subscribe(
    vaultId: string,
    listener: (event: RemoteEvent) => void,
  ): Promise<() => Promise<void>> {
    this.assertOpen();
    const subscriptionId = crypto.randomUUID();
    const channel = new Channel<RemoteEvent>();
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
      applicationResult(
        await commands.remoteSubscribe(
          this.info.connection_id,
          vaultId,
          subscriptionId,
          channel,
        ),
      );
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
