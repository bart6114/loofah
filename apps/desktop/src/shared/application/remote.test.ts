import { beforeEach, describe, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  remoteConnect: vi.fn(),
  remoteLogin: vi.fn(),
  remoteCancelLogin: vi.fn(),
  remoteRequest: vi.fn(),
  remoteCancelRequest: vi.fn(),
  remoteSubscribe: vi.fn(),
  remoteUnsubscribe: vi.fn(),
  remoteDisconnect: vi.fn(),
  remoteLogout: vi.fn(),
  remoteAudioOpen: vi.fn(),
  remoteAudioClose: vi.fn(),
  remoteAudioPeaks: vi.fn(),
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
