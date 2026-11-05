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
