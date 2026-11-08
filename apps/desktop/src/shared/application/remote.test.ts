import { beforeEach, describe, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  remoteConnect: vi.fn(),
  remoteClaimNoteWindow: vi.fn(),
  remoteOpenNoteWindow: vi.fn(),
  remoteLogin: vi.fn(),
  remoteCancelLogin: vi.fn(),
  remoteRequest: vi.fn(),
  remoteCancelRequest: vi.fn(),
  remoteSubscribe: vi.fn(),
  remoteSubscribeCaptureLive: vi.fn(),
  remoteUnsubscribe: vi.fn(),
  remoteDisconnect: vi.fn(),
  remoteLogout: vi.fn(),
  remoteAudioOpen: vi.fn(),
  remoteAudioClose: vi.fn(),
  remoteAudioPeaks: vi.fn(),
  remoteAttachmentOpen: vi.fn(),
  remoteAttachmentClose: vi.fn(),
  remoteAttachmentCopy: vi.fn(),
  remoteUploadAttachmentSource: vi.fn(),
  remoteMediaRequest: vi.fn(),
  remoteMediaSubmit: vi.fn(),
  remoteMediaRecover: vi.fn(),
  remoteMediaCancel: vi.fn(),
  remoteMediaResumeSource: vi.fn(),
  remoteMediaSubmissions: vi.fn(),
  remoteMediaExportSource: vi.fn(),
  attachmentTargets: new Map<string, string>(),
}));
vi.mock("~/types/tauri.gen", () => ({ commands: native }));
vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    onmessage?: (event: unknown) => void;
  },
}));

import { ApplicationRequestError } from "./errors";
import { NativeRemoteConnection } from "./remote";

const me = {
  member: { id: "member", name: "Member" },
  personal_vault_id: "private",
};
const info = {
  connection_id: "connection",
  server: "https://loofah.example/",
  capabilities: {
    api_major: 1,
    application_version: "test",
    features: ["notes"],
  },
};
const error = {
  code: "unauthenticated",
  message: "Sign in",
  retryable: false,
  request_id: null,
  current_revision: null,
  mutation_outcome: null,
};

it("requires original-source capability before native dispatch", async () => {
  const client = await NativeRemoteConnection.connect(info.server);
  await expect(
    client.exportMediaSource("vault", "original"),
  ).rejects.toMatchObject({ detail: { code: "incompatible_version" } });
  expect(native.remoteMediaExportSource).not.toHaveBeenCalled();
});

it("keeps original source save routing and cancellation on the original request", async () => {
  native.remoteConnect.mockResolvedValueOnce({
    status: "ok",
    data: {
      ...info,
      capabilities: {
        ...info.capabilities,
        features: ["media_jobs", "media_source_export"],
      },
    },
  });
  const client = await NativeRemoteConnection.connect(info.server);
  let finish!: (value: unknown) => void;
  native.remoteMediaExportSource.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const abort = new AbortController();
  const saving = client.exportMediaSource(
    "original-vault",
    "original-request",
    { signal: abort.signal },
  );
  const rejected = expect(saving).rejects.toMatchObject({
    detail: { code: "cancelled", request_id: "original-request" },
  });
  expect(native.remoteMediaExportSource).toHaveBeenCalledExactlyOnceWith(
    "connection",
    "original-vault",
    "original-request",
  );
  abort.abort();
  expect(native.remoteCancelRequest).toHaveBeenCalledWith(
    "connection",
    "original-request",
  );
  finish({ status: "ok", data: { path: "/downloads/Loofah/source.wav" } });
  await rejected;
  expect(native.remoteMediaSubmit).not.toHaveBeenCalled();
});

it("keeps upload paths native, preserves the original request ID, and cancels an in-flight upload", async () => {
  const connection = await NativeRemoteConnection.connect(info.server);
  const input = { filename: "file.pdf", size_bytes: 7, sha256: "1".repeat(64) };
  const metadata = {
    filename: "file 1.pdf",
    sizeBytes: 7,
    sha256: input.sha256,
    contentType: "application/pdf",
  };
  const requestId = crypto.randomUUID();
  let finish!: (result: unknown) => void;
  native.remoteUploadAttachmentSource.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const controller = new AbortController();
  const uploading = connection.uploadAttachmentPath(
    "vault",
    "note",
    "/device/file.pdf",
    input,
    { requestId, signal: controller.signal },
  );
  expect(native.remoteUploadAttachmentSource).toHaveBeenCalledWith(
    info.connection_id,
    { vault_id: "vault", session_id: "note", request_id: requestId, input },
    { kind: "path", path: "/device/file.pdf" },
  );
  controller.abort();
  expect(native.remoteCancelRequest).toHaveBeenCalledWith(
    info.connection_id,
    requestId,
  );
  finish({ status: "ok", data: metadata });
  await expect(uploading).rejects.toMatchObject({
    detail: {
      code: "cancelled",
      request_id: requestId,
      mutation_outcome: "unknown",
    },
  });
  expect(native.remoteUploadAttachmentSource).toHaveBeenCalledTimes(1);
  await connection.disconnect();
});

beforeEach(() => {
  vi.resetAllMocks();
  native.remoteCancelRequest.mockResolvedValue(undefined);
  native.remoteUnsubscribe.mockResolvedValue(undefined);
  native.remoteDisconnect.mockResolvedValue(undefined);
  native.remoteConnect.mockResolvedValue({ status: "ok", data: info });
  native.remoteRequest.mockResolvedValue({
    status: "ok",
    data: { kind: "me", value: me },
  });
  native.remoteSubscribe.mockResolvedValue({ status: "ok", data: undefined });
  native.remoteSubscribeCaptureLive.mockResolvedValue({
    status: "ok",
    data: null,
  });
  native.remoteLogout.mockResolvedValue({ status: "ok", data: undefined });
  native.remoteAudioClose.mockResolvedValue(undefined);
  native.remoteAudioOpen.mockImplementation(async (_connection, id) => ({
    status: "ok",
    data: {
      id,
      metadata: {
        filename: "audio.wav",
        contentType: "audio/wav",
        sizeBytes: 100,
        sha256: "sha",
      },
    },
  }));
  native.remoteAudioPeaks.mockResolvedValue({
    status: "ok",
    data: {
      recording_sha256: "sha",
      peaks: { durationSec: 1, channels: [[0, 1]] },
    },
  });
  native.attachmentTargets.clear();
  native.remoteAttachmentClose.mockResolvedValue(undefined);
  native.remoteAttachmentOpen.mockImplementation(
    async (_connection, id, target) => {
      native.attachmentTargets.set(id, target.attachment_id);
      return {
        status: "ok",
        data: { id, preview_limit_bytes: 32 * 1024 * 1024 },
      };
    },
  );
  native.remoteAttachmentCopy.mockImplementation(async (_connection, id) => ({
    status: "ok",
    data: {
      copied_filename: "downloaded-copy.svg",
      metadata: {
        filename: native.attachmentTargets.get(id),
        contentType: "image/svg+xml",
        sizeBytes: 42,
        sha256: "a".repeat(64),
      },
    },
  }));
});

describe("native attachment resource lifetime", () => {
  it("binds opaque resources to coordinates and delegates copy actions without exposing a path", async () => {
    const connection = await NativeRemoteConnection.connect(info.server);
    const controller = new AbortController();
    const resource = await connection.openAttachment("vault", "note", "a.svg", {
      signal: controller.signal,
    });
    expect(native.remoteAttachmentOpen).toHaveBeenCalledWith(
      info.connection_id,
      resource.id,
      { vault_id: "vault", session_id: "note", attachment_id: "a.svg" },
    );
    controller.abort();
    expect(native.remoteAttachmentClose).not.toHaveBeenCalled();
    const requestId = crypto.randomUUID();
    const copy = await connection.copyAttachment(resource.id, "open", {
      requestId,
    });
    expect(native.remoteAttachmentCopy).toHaveBeenCalledWith(
      info.connection_id,
      resource.id,
      requestId,
      "open",
    );
    expect(copy).not.toHaveProperty("path");
    await Promise.all([
      connection.closeAttachment(resource.id),
      connection.closeAttachment(resource.id),
    ]);
    expect(native.remoteAttachmentClose).toHaveBeenCalledTimes(1);
    await expect(
      connection.copyAttachment(resource.id, "reveal"),
    ).rejects.toMatchObject({ detail: { code: "cancelled" } });
  });

  it("closes before late registration and ignores returned handles after an aborted open", async () => {
    const connection = await NativeRemoteConnection.connect(info.server);
    let complete!: (value: unknown) => void;
    native.remoteAttachmentOpen.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          complete = resolve;
        }),
    );
    const controller = new AbortController();
    const opening = connection.openAttachment("vault", "note", "a.svg", {
      signal: controller.signal,
    });
    const id = native.remoteAttachmentOpen.mock.calls[0][1];
    controller.abort();
    complete({
      status: "ok",
      data: { id, preview_limit_bytes: 32 * 1024 * 1024 },
    });
    await expect(opening).rejects.toMatchObject({
      detail: { code: "cancelled" },
    });
    expect(native.remoteAttachmentClose).toHaveBeenCalledWith(
      info.connection_id,
      id,
    );
    expect(native.remoteAttachmentClose).toHaveBeenCalledTimes(1);
  });

  it("cancels copy requests and treats late side effects as uncertain without automatically retrying", async () => {
    const connection = await NativeRemoteConnection.connect(info.server);
    const resource = await connection.openAttachment("vault", "note", "a.svg");
    let complete!: (value: unknown) => void;
    native.remoteAttachmentCopy.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          complete = resolve;
        }),
    );
    const controller = new AbortController();
    const requestId = crypto.randomUUID();
    const copying = connection.copyAttachment(resource.id, "save", {
      signal: controller.signal,
      requestId,
    });
    controller.abort();
    expect(native.remoteCancelRequest).toHaveBeenCalledWith(
      info.connection_id,
      requestId,
    );
    complete({
      status: "ok",
      data: { copied_filename: "a.svg", metadata: { filename: "a.svg" } },
    });
    await expect(copying).rejects.toMatchObject({
      detail: { code: "cancelled", mutation_outcome: "unknown" },
    });
    expect(native.remoteAttachmentCopy).toHaveBeenCalledTimes(1);
    await expect(
      connection.copyAttachment(resource.id, "reveal"),
    ).resolves.toHaveProperty("copied_filename");
  });

  it("closes descriptors during disconnect and refuses late copy delivery", async () => {
    const connection = await NativeRemoteConnection.connect(info.server);
    const resource = await connection.openAttachment("vault", "note", "a.svg");
    let complete!: (value: unknown) => void;
    native.remoteAttachmentCopy.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          complete = resolve;
        }),
    );
    const copying = connection.copyAttachment(resource.id, "open");
    await connection.disconnect();
    complete({
      status: "ok",
      data: { copied_filename: "a.svg", metadata: { filename: "a.svg" } },
    });
    await expect(copying).rejects.toMatchObject({
      detail: { code: "cancelled", mutation_outcome: "unknown" },
    });
    expect(native.remoteAttachmentClose).toHaveBeenCalledWith(
      info.connection_id,
      resource.id,
    );
    expect(native.remoteDisconnect).toHaveBeenCalledTimes(1);
  });

  it("closes only the revoked vault's attachments and all attachments after sign-out", async () => {
    const connection = await NativeRemoteConnection.connect(info.server);
    const first = await connection.openAttachment("first", "note", "a.svg");
    const second = await connection.openAttachment("second", "note", "a.svg");
    await connection.subscribe("first", vi.fn());
    const channel = native.remoteSubscribe.mock.calls[0][3];
    channel.onmessage({ kind: "connection", state: "access_denied", error });
    await vi.waitFor(() =>
      expect(native.remoteAttachmentClose).toHaveBeenCalledWith(
        info.connection_id,
        first.id,
      ),
    );
    expect(native.remoteAttachmentClose).not.toHaveBeenCalledWith(
      info.connection_id,
      second.id,
    );
    await expect(
      connection.copyAttachment(second.id, "reveal"),
    ).resolves.toHaveProperty("copied_filename");
    channel.onmessage({ kind: "connection", state: "login_required", error });
    await vi.waitFor(() =>
      expect(native.remoteAttachmentClose).toHaveBeenCalledWith(
        info.connection_id,
        second.id,
      ),
    );
  });

  it("rejects mismatched native handles and metadata without accepting a device path", async () => {
    const connection = await NativeRemoteConnection.connect(info.server);
    native.remoteAttachmentOpen.mockResolvedValueOnce({
      status: "ok",
      data: { id: "foreign", preview_limit_bytes: 32 * 1024 * 1024 },
    });
    await expect(
      connection.openAttachment("vault", "note", "a.svg"),
    ).rejects.toMatchObject({ detail: { code: "incompatible_version" } });
    const resource = await connection.openAttachment("vault", "note", "a.svg");
    native.remoteAttachmentCopy.mockResolvedValueOnce({
      status: "ok",
      data: {
        copied_filename: "../bad.svg",
        metadata: { filename: "other.svg" },
      },
    });
    await expect(
      connection.copyAttachment(resource.id, "save"),
    ).rejects.toMatchObject({ detail: { code: "incompatible_version" } });
  });
});

describe("native recording resource lifetime", () => {
  it("owns each descriptor until an idempotent close and removes completed abort listeners", async () => {
    const connection = await NativeRemoteConnection.connect(info.server);
    const abort = new AbortController();
    const recording = await connection.openRecording("vault", "session", {
      signal: abort.signal,
    });
    expect(native.remoteAudioOpen).toHaveBeenCalledWith(
      "connection",
      recording!.id,
      "vault",
      "session",
    );
    abort.abort();
    expect(native.remoteAudioClose).not.toHaveBeenCalled();
    await expect(
      connection.recordingWaveform(recording!.id),
    ).resolves.toMatchObject({ recording_sha256: "sha" });
    await Promise.all([
      connection.closeRecording(recording!.id),
      connection.closeRecording(recording!.id),
    ]);
    expect(native.remoteAudioClose).toHaveBeenCalledExactlyOnceWith(
      "connection",
      recording!.id,
    );
    await expect(connection.recordingWaveform(recording!.id)).rejects.toThrow();
  });

  it.each(["abort", "disconnect"] as const)(
    "closes before delayed native admission and rejects a late open after %s",
    async (action) => {
      const connection = await NativeRemoteConnection.connect(info.server);
      let finish!: (value: unknown) => void;
      native.remoteAudioOpen.mockReturnValueOnce(
        new Promise((resolve) => {
          finish = resolve;
        }),
      );
      const abort = new AbortController();
      const opening = connection.openRecording("vault", "session", {
        signal: abort.signal,
      });
      const id = native.remoteAudioOpen.mock.calls[0][1];
      if (action === "abort") abort.abort();
      else await connection.disconnect();
      expect(native.remoteAudioClose).toHaveBeenCalledWith("connection", id);
      finish({ status: "ok", data: { id, metadata: {} } });
      await expect(opening).rejects.toThrow();
    },
  );

  it("cleans up absent recordings and unexpected native identifiers", async () => {
    const connection = await NativeRemoteConnection.connect(info.server);
    native.remoteAudioOpen.mockResolvedValueOnce({ status: "ok", data: null });
    expect(await connection.openRecording("vault", "missing")).toBeNull();
    native.remoteAudioOpen.mockResolvedValueOnce({
      status: "ok",
      data: { id: "other", metadata: {} },
    });
    await expect(
      connection.openRecording("vault", "session"),
    ).rejects.toMatchObject({ detail: { code: "incompatible_version" } });
    expect(native.remoteAudioClose).toHaveBeenCalledTimes(2);
  });

  it("does not let one JavaScript connection use another connection's resource", async () => {
    const first = await NativeRemoteConnection.connect(info.server);
    const second = await NativeRemoteConnection.connect(info.server);
    const recording = await first.openRecording("vault", "session");
    await expect(second.recordingWaveform(recording!.id)).rejects.toThrow();
    await second.closeRecording(recording!.id);
    expect(native.remoteAudioClose).not.toHaveBeenCalled();
    await first.disconnect();
    expect(native.remoteAudioClose).toHaveBeenCalledWith(
      "connection",
      recording!.id,
    );
  });

  it("cancels a waveform request and rejects a response arriving after resource closure", async () => {
    const connection = await NativeRemoteConnection.connect(info.server);
    const recording = await connection.openRecording("vault", "session");
    let finish!: (value: unknown) => void;
    native.remoteAudioPeaks.mockReturnValueOnce(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    const abort = new AbortController();
    const waveform = connection.recordingWaveform(recording!.id, {
      requestId: "waveform-request",
      signal: abort.signal,
    });
    abort.abort();
    expect(native.remoteCancelRequest).toHaveBeenCalledWith(
      "connection",
      "waveform-request",
    );
    await connection.closeRecording(recording!.id);
    finish({
      status: "ok",
      data: {
        recording_sha256: "sha",
        peaks: { durationSec: 1, channels: [[1]] },
      },
    });
    await expect(waveform).rejects.toThrow();
  });

  it("retries a failed close and still performs authoritative logout when cleanup fails", async () => {
    const connection = await NativeRemoteConnection.connect(info.server);
    const recording = await connection.openRecording("vault", "session");
    native.remoteAudioClose.mockRejectedValueOnce(new Error("IPC failed"));
    await expect(connection.closeRecording(recording!.id)).rejects.toThrow(
      "IPC failed",
    );
    await connection.closeRecording(recording!.id);
    expect(native.remoteAudioClose).toHaveBeenCalledTimes(2);
    await connection.openRecording("vault", "second");
    native.remoteAudioClose.mockRejectedValue(new Error("IPC failed"));
    await connection.signOut();
    expect(native.remoteLogout).toHaveBeenCalledWith("connection");
  });
});

describe("native remote connection", () => {
  it("restores the account through native requests with a stable explicit request ID", async () => {
    const connection = await NativeRemoteConnection.connect(info.server);
    expect(
      await connection.request({ kind: "provision" }, "me", {
        requestId: "retry-id",
      }),
    ).toEqual(me);
    expect(native.remoteRequest).toHaveBeenCalledWith(
      "connection",
      "retry-id",
      { kind: "provision" },
    );
  });

  it("keeps structured authentication errors and rejects a response for a different operation", async () => {
    const connection = await NativeRemoteConnection.connect(info.server);
    native.remoteRequest.mockResolvedValueOnce({ status: "error", error });
    await expect(connection.restore()).rejects.toMatchObject({ detail: error });
    await expect(
      connection.request({ kind: "vaults" }, "vaults"),
    ).rejects.toMatchObject({ detail: { code: "incompatible_version" } });
  });

  it("cancels through the same native connection and removes the abort listener on completion", async () => {
    const connection = await NativeRemoteConnection.connect(info.server);
    const controller = new AbortController();
    let resolve!: (value: unknown) => void;
    native.remoteRequest.mockReturnValueOnce(
      new Promise((done) => {
        resolve = done;
      }),
    );
    const result = connection.request({ kind: "me" }, "me", {
      signal: controller.signal,
      requestId: "request",
    });
    controller.abort();
    expect(native.remoteCancelRequest).toHaveBeenCalledWith(
      "connection",
      "request",
    );
    resolve({ status: "ok", data: { kind: "me", value: me } });
    await expect(result).rejects.toThrow();
    const later = new AbortController();
    await connection.request({ kind: "me" }, "me", { signal: later.signal });
    later.abort();
    expect(native.remoteCancelRequest).toHaveBeenCalledTimes(1);
  });

  it("stops events immediately when unsubscribed and keeps teardown idempotent", async () => {
    const connection = await NativeRemoteConnection.connect(info.server);
    const listener = vi.fn();
    const unsubscribe = await connection.subscribe("private", listener);
    const channel = native.remoteSubscribe.mock.calls[0][3];
    const event = { kind: "connection", state: "connected", error: null };
    channel.onmessage(event);
    expect(listener).toHaveBeenCalledWith(event);
    unsubscribe();
    unsubscribe();
    channel.onmessage(event);
    expect(listener).toHaveBeenCalledTimes(1);
    expect(native.remoteUnsubscribe).toHaveBeenCalledTimes(1);
  });

  it("closes a subscription even if disconnect races native subscription admission", async () => {
    const connection = await NativeRemoteConnection.connect(info.server);
    let resolve!: (value: unknown) => void;
    native.remoteSubscribe.mockReturnValueOnce(
      new Promise((done) => {
        resolve = done;
      }),
    );
    const listener = vi.fn();
    const subscription = connection.subscribe("private", listener);
    await connection.disconnect();
    resolve({ status: "ok", data: undefined });
    await subscription;
    native.remoteSubscribe.mock.calls[0][3].onmessage({
      kind: "connection",
      state: "connected",
    });
    expect(listener).not.toHaveBeenCalled();
    await expect(connection.restore()).rejects.toBeInstanceOf(
      ApplicationRequestError,
    );
  });

  it("subscribes to exact live generation/cursor and retires delivery with its connection", async () => {
    const connection = await NativeRemoteConnection.connect(info.server);
    const binding = {
      capture_id: crypto.randomUUID(),
      generation_id: crypto.randomUUID(),
      capture_fingerprint: "1".repeat(64),
      session_id: "note",
      format: {
        encoding: "pcm_f32_le" as const,
        sample_rate: 16000,
        channels: ["microphone" as const],
      },
    };
    const listener = vi.fn();
    const unsubscribe = await connection.subscribeCaptureLive(
      "vault",
      binding,
      17,
      listener,
    );
    const [id, vault, subscriptionId, actualBinding, after, channel] =
      native.remoteSubscribeCaptureLive.mock.calls[0];
    expect([id, vault, actualBinding, after]).toEqual([
      "connection",
      "vault",
      binding,
      17,
    ]);
    const event = {
      kind: "page",
      page: {
        captions: [],
        snapshot: { generation_id: binding.generation_id },
      },
    };
    channel.onmessage(event);
    expect(listener).toHaveBeenCalledWith(event);
    await connection.disconnect();
    channel.onmessage(event);
    await unsubscribe();
    expect(listener).toHaveBeenCalledTimes(1);
    expect(native.remoteUnsubscribe).toHaveBeenCalledExactlyOnceWith(
      "connection",
      subscriptionId,
    );
  });

  it("cleans live subscription admission after the window connection has retired", async () => {
    const connection = await NativeRemoteConnection.connect(info.server);
    let resolve!: (value: unknown) => void;
    native.remoteSubscribeCaptureLive.mockReturnValueOnce(
      new Promise((done) => {
        resolve = done;
      }),
    );
    const binding = {
      capture_id: crypto.randomUUID(),
      generation_id: crypto.randomUUID(),
      capture_fingerprint: "1".repeat(64),
      session_id: "note",
      format: {
        encoding: "pcm_f32_le" as const,
        sample_rate: 16000,
        channels: ["microphone" as const],
      },
    };
    const listener = vi.fn();
    const pending = connection.subscribeCaptureLive(
      "vault",
      binding,
      null,
      listener,
    );
    await connection.disconnect();
    resolve({ status: "ok", data: null });
    await pending;
    native.remoteSubscribeCaptureLive.mock.calls[0][5].onmessage({
      kind: "connection",
      state: "connected",
      error: null,
    });
    expect(listener).not.toHaveBeenCalled();
    expect(native.remoteUnsubscribe).toHaveBeenCalledTimes(1);
  });

  it("signs out in native code and blocks pending responses from becoming active again", async () => {
    const connection = await NativeRemoteConnection.connect(info.server);
    let resolve!: (value: unknown) => void;
    native.remoteRequest.mockReturnValueOnce(
      new Promise((done) => {
        resolve = done;
      }),
    );
    const request = connection.restore();
    await connection.signOut();
    resolve({ status: "ok", data: { kind: "me", value: me } });
    await expect(request).rejects.toMatchObject({
      detail: { code: "cancelled", mutation_outcome: "unknown" },
    });
    expect(native.remoteLogout).toHaveBeenCalledWith("connection");
  });
  it("allows retrying sign-out when the OS credential store was locked", async () => {
    const connection = await NativeRemoteConnection.connect(info.server);
    native.remoteLogout.mockResolvedValueOnce({
      status: "error",
      error: {
        ...error,
        code: "internal",
        message: "Credential store is locked",
      },
    });
    await expect(connection.signOut()).rejects.toThrow(
      "Credential store is locked",
    );
    await connection.signOut();
    expect(native.remoteLogout).toHaveBeenCalledTimes(2);
  });
  it("can release the native connection after failed credential deletion", async () => {
    const connection = await NativeRemoteConnection.connect(info.server);
    native.remoteLogout.mockResolvedValueOnce({
      status: "error",
      error: {
        ...error,
        code: "internal",
        message: "Credential store is locked",
      },
    });
    await expect(connection.signOut()).rejects.toThrow(
      "Credential store is locked",
    );
    await connection.disconnect();
    await connection.disconnect();
    expect(native.remoteDisconnect).toHaveBeenCalledOnce();
  });
  it("releases its native lease once when two cleanup paths run concurrently", async () => {
    const connection = await NativeRemoteConnection.connect(info.server);
    let finish!: () => void;
    native.remoteDisconnect.mockReturnValueOnce(
      new Promise<void>((resolve) => {
        finish = resolve;
      }),
    );
    const first = connection.disconnect();
    const second = connection.disconnect();
    await vi.waitFor(() =>
      expect(native.remoteDisconnect).toHaveBeenCalledOnce(),
    );
    finish();
    await first;
    await second;
    expect(native.remoteDisconnect).toHaveBeenCalledOnce();
  });
});

describe("scoped media bridge", () => {
  function declaration(requestId: string) {
    return {
      request_id: requestId,
      operation: {
        kind: "import_audio" as const,
        session_id: null,
        expected: null,
        filename: "original.wav",
        content_type: "audio/wav",
        created_at: null,
        transcribe: false,
        source: { sha256: "a".repeat(64), size_bytes: 14 },
      },
      transcription: null,
      summary: null,
    };
  }
  async function connection() {
    native.remoteConnect.mockResolvedValueOnce({
      status: "ok",
      data: {
        ...info,
        capabilities: { ...info.capabilities, features: ["media_jobs"] },
      },
    });
    return NativeRemoteConnection.connect(info.server);
  }
  it("refuses missing capability and mismatched declaration IDs before native dispatch", async () => {
    const client = await NativeRemoteConnection.connect(info.server);
    const id = crypto.randomUUID();
    await expect(
      client.submitMedia(
        "private",
        declaration(id),
        { kind: "path", path: "/device/original.wav" },
        { requestId: id },
      ),
    ).rejects.toMatchObject({
      detail: {
        code: "incompatible_version",
        mutation_outcome: "not_committed",
      },
    });
    const supported = await connection();
    await expect(
      supported.submitMedia("private", declaration(id), null, {
        requestId: crypto.randomUUID(),
      }),
    ).rejects.toMatchObject({
      detail: { code: "validation", mutation_outcome: "not_committed" },
    });
    expect(native.remoteMediaSubmit).not.toHaveBeenCalled();
  });
  it("passes exact original declarations and device sources once, leaving unknown writes for explicit recovery", async () => {
    const client = await connection();
    const id = crypto.randomUUID();
    const input = declaration(id);
    const failure = {
      ...error,
      code: "unavailable",
      request_id: id,
      mutation_outcome: "unknown",
    };
    native.remoteMediaSubmit.mockResolvedValue({
      status: "error",
      error: failure,
    });
    const source = { kind: "staged" as const, id: "original-stage" };
    await expect(
      client.submitMedia("private", input, source, { requestId: id }),
    ).rejects.toMatchObject({ detail: failure });
    expect(native.remoteMediaSubmit).toHaveBeenCalledExactlyOnceWith(
      "connection",
      "private",
      input,
      source,
    );
    expect(native.remoteMediaRecover).not.toHaveBeenCalled();
    expect(native.remoteMediaResumeSource).not.toHaveBeenCalled();
    const status = {
      declaration: input,
      revision: "b".repeat(64),
      phase: "review",
      media: null,
    };
    native.remoteMediaRecover.mockResolvedValue({ status: "ok", data: status });
    await expect(client.recoverMedia("private", id)).resolves.toEqual(status);
    expect(native.remoteMediaRecover).toHaveBeenCalledExactlyOnceWith(
      "connection",
      "private",
      id,
    );
  });
  it("cancels only the original in-flight submission and marks its late success unknown", async () => {
    const client = await connection();
    const id = crypto.randomUUID();
    const input = declaration(id);
    let finish!: (result: unknown) => void;
    native.remoteMediaSubmit.mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const cancel = new AbortController();
    const pending = client.submitMedia(
      "private",
      input,
      { kind: "path", path: "/device/original.wav" },
      { requestId: id, signal: cancel.signal },
    );
    cancel.abort();
    expect(native.remoteCancelRequest).toHaveBeenCalledWith("connection", id);
    finish({
      status: "ok",
      data: {
        declaration: input,
        revision: "b".repeat(64),
        phase: "running",
        media: null,
      },
    });
    await expect(pending).rejects.toMatchObject({
      detail: {
        code: "cancelled",
        request_id: id,
        mutation_outcome: "unknown",
      },
    });
    expect(native.remoteMediaSubmit).toHaveBeenCalledTimes(1);
  });
});

it("claims a standalone note without discovery or login and cancels opening on its original request", async () => {
  native.remoteClaimNoteWindow.mockResolvedValue({
    status: "ok",
    data: {
      connection: info,
      member_id: "member",
      vault: { id: "original", role: "reader" },
      session_id: "note",
    },
  });
  const claimed = await NativeRemoteConnection.claimNoteWindow();
  expect(claimed.connection.standaloneNote).toBe(true);
  expect(claimed.info.vault.id).toBe("original");
  expect(native.remoteConnect).not.toHaveBeenCalled();
  expect(native.remoteLogin).not.toHaveBeenCalled();
  await claimed.connection.disconnect();
  expect(native.remoteDisconnect).toHaveBeenCalledWith(info.connection_id);
  expect(native.remoteLogout).not.toHaveBeenCalled();
  const parent = await NativeRemoteConnection.connect(info.server);
  let complete!: () => void;
  native.remoteOpenNoteWindow.mockImplementation(
    () =>
      new Promise((resolve) => {
        complete = () => resolve({ status: "ok", data: null });
      }),
  );
  const abort = new AbortController();
  const pending = parent.openNoteWindow("original", "note", {
    requestId: "original-request",
    signal: abort.signal,
  });
  abort.abort();
  expect(native.remoteCancelRequest).toHaveBeenCalledWith(
    info.connection_id,
    "original-request",
  );
  complete();
  await expect(pending).rejects.toBeTruthy();
  expect(native.remoteOpenNoteWindow).toHaveBeenCalledWith(
    info.connection_id,
    "original-request",
    "original",
    "note",
  );
});
