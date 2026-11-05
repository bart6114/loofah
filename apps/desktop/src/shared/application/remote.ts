import { Channel } from "@tauri-apps/api/core";

import {
  applicationResult,
  ApplicationRequestError,
  connectionClosed,
} from "./errors";

import {
  commands,
  type Me,
  type RemoteConnectionInfo,
  type RemoteEvent,
  type RemoteOperation,
  type RemoteReply,
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
    await Promise.all(
      [...this.subscriptions].map((unsubscribe) => unsubscribe()),
    );
  }

  private assertOpen() {
    if (this.closed) throw connectionClosed();
  }
}
