import { Channel } from "@tauri-apps/api/core";

import {
  applicationResult,
  ApplicationRequestError,
  connectionClosed,
} from "./errors";

import {
  commands,
  type CaptureIdentity,
  type CaptureScope,
  type DeviceCaptureStartRequest,
} from "~/types/tauri.gen";

export function createDeviceRecordingClient({
  scope,
  connectionId,
}: {
  scope: CaptureScope;
  connectionId?: string;
}) {
  const destination = Object.freeze({ ...scope });
  const subscriptions = new Set<() => void>();
  let disposed = false;
  const assertOpen = () => {
    if (disposed) throw connectionClosed();
  };
  return {
    async start(
      input: Omit<DeviceCaptureStartRequest, "scope">,
      { signal }: { signal?: AbortSignal } = {},
    ) {
      assertOpen();
      signal?.throwIfAborted();
      const request = structuredClone({ ...input, scope: destination });
      const unknown = () =>
        new ApplicationRequestError({
          code: "cancelled",
          message:
            "The recording request was sent. Check device status before retrying.",
          request_id: request.request_id,
          retryable: false,
          current_revision: null,
          mutation_outcome: "unknown",
        });
      const result = await commands
        .deviceRecordingStart(connectionId ?? null, request)
        .catch(() => {
          throw unknown();
        });
      if (disposed || signal?.aborted) throw unknown();
      return applicationResult(result);
    },
    async inventory() {
      assertOpen();
      const result = await commands.deviceRecordingInventory();
      assertOpen();
      return applicationResult(result);
    },
    async status() {
      assertOpen();
      const result = await commands.deviceRecordingStatus();
      assertOpen();
      return applicationResult(result);
    },
    async stop(identity: CaptureIdentity) {
      return applicationResult(await commands.deviceRecordingStop(identity));
    },
    async resumeUpload(identity: CaptureIdentity) {
      assertOpen();
      if (!connectionId) throw connectionClosed();
      return applicationResult(
        await commands.deviceRecordingResumeUpload(connectionId, identity),
      );
    },
    async subscribe(
      changed: () => void,
      { signal }: { signal?: AbortSignal } = {},
    ) {
      assertOpen();
      signal?.throwIfAborted();
      const id = crypto.randomUUID();
      let closed = false;
      const unsubscribe = () => {
        void commands.deviceRecordingUnsubscribe(id).catch(() => {});
      };
      const close = () => {
        if (closed) return;
        closed = true;
        subscriptions.delete(close);
        signal?.removeEventListener("abort", close);
        unsubscribe();
      };
      const channel = new Channel<number>();
      channel.onmessage = () => {
        if (!closed && !disposed) changed();
      };
      subscriptions.add(close);
      signal?.addEventListener("abort", close, { once: true });
      try {
        applicationResult(await commands.deviceRecordingSubscribe(id, channel));
        // Closing may precede native admission. Its late acknowledgment requires
        // another unsubscribe, while callbacks remain fenced throughout.
        if (closed || disposed) unsubscribe();
        signal?.throwIfAborted();
        return close;
      } catch (error) {
        close();
        throw error;
      }
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      for (const close of subscriptions) close();
    },
  };
}
