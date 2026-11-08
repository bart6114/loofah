import { beforeEach, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  commands: {
    deviceRecordingStart: vi.fn(),
    deviceRecordingAppend: vi.fn(),
    deviceRecordingAppendPending: vi.fn(),
    deviceRecordingInventory: vi.fn(),
    deviceRecordingRecoveryInventory: vi.fn(),
    deviceRecordingStatus: vi.fn(),
    deviceRecordingStop: vi.fn(),
    deviceRecordingFinishRecovery: vi.fn(),
    deviceRecordingExport: vi.fn(),
    deviceRecordingResumeUpload: vi.fn(),
    deviceRecordingRetire: vi.fn(),
    deviceRecordingSubscribe: vi.fn(),
    deviceRecordingUnsubscribe: vi.fn(),
  },
  channels: [] as { onmessage?: (value: number) => void }[],
}));
vi.mock("~/types/tauri.gen", () => ({ commands: native.commands }));
vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    onmessage?: (value: number) => void;
    constructor() {
      native.channels.push(this);
    }
  },
}));

import {
  createDeviceRecordingClient,
  createDeviceRecordingMonitor,
} from "./device-recordings";

const scope = {
  server: "https://company.example",
  member_id: "member",
  vault_id: "vault",
};
const input = {
  request_id: "eeb6b370-8019-484d-9fba-6944756aa5c4",
  target: {
    kind: "existing_session" as const,
    session_id: "note",
    expected_audio_sha256: null,
  },
  format: {
    encoding: "pcm_f32_le" as const,
    sample_rate: 16000,
    channels: ["mixed" as const],
  },
};
const identity = {
  scope,
  capture_id: "capture",
  request_id: input.request_id,
  target: input.target,
};

beforeEach(() => {
  vi.resetAllMocks();
  native.channels.length = 0;
  native.commands.deviceRecordingUnsubscribe.mockResolvedValue(undefined);
  native.commands.deviceRecordingStop.mockResolvedValue({
    status: "ok",
    data: true,
  });
});

it("recovers and exports an original device identity without a server connection", async () => {
  const monitor = createDeviceRecordingMonitor();
  native.commands.deviceRecordingFinishRecovery.mockResolvedValue({
    status: "ok",
    data: { identity, sealed: true },
  });
  native.commands.deviceRecordingExport.mockResolvedValue({
    status: "ok",
    data: null,
  });
  await expect(monitor.finishRecovery(identity)).resolves.toMatchObject({
    identity,
    sealed: true,
  });
  await expect(monitor.exportAudio(identity)).resolves.toBeNull();
  expect(
    native.commands.deviceRecordingFinishRecovery,
  ).toHaveBeenCalledExactlyOnceWith(identity);
  expect(native.commands.deviceRecordingExport).toHaveBeenCalledExactlyOnceWith(
    identity,
  );
  expect(native.commands.deviceRecordingStart).not.toHaveBeenCalled();
  expect(native.commands.deviceRecordingResumeUpload).not.toHaveBeenCalled();
});

it("does no startup work and refuses an aborted start before native admission", async () => {
  const client = createDeviceRecordingClient({ scope });
  expect(native.commands.deviceRecordingStatus).not.toHaveBeenCalled();
  expect(native.commands.deviceRecordingInventory).not.toHaveBeenCalled();
  const abort = new AbortController();
  abort.abort();
  await expect(client.start(input, { signal: abort.signal })).rejects.toThrow();
  expect(native.commands.deviceRecordingStart).not.toHaveBeenCalled();
});

it("retains the original destination and request UUID when a start reply is lost", async () => {
  const destination = { ...scope };
  const client = createDeviceRecordingClient({ scope: destination });
  destination.vault_id = "other";
  native.commands.deviceRecordingStart.mockRejectedValue(
    new Error("lost IPC reply"),
  );
  await expect(client.start(input)).rejects.toMatchObject({
    detail: {
      request_id: input.request_id,
      retryable: false,
      mutation_outcome: "unknown",
    },
  });
  expect(native.commands.deviceRecordingStart).toHaveBeenCalledWith(null, {
    ...input,
    scope,
  });
  expect(native.commands.deviceRecordingStop).not.toHaveBeenCalled();
});

it("closing UI during start preserves native recording and still permits explicit original stop", async () => {
  let resolve!: (value: unknown) => void;
  native.commands.deviceRecordingStart.mockImplementation(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  const client = createDeviceRecordingClient({
    scope,
    connectionId: "original",
  });
  const pending = client.start(input);
  client.dispose();
  resolve({ status: "ok", data: { identity } });
  await expect(pending).rejects.toMatchObject({
    detail: { mutation_outcome: "unknown", request_id: input.request_id },
  });
  expect(native.commands.deviceRecordingStop).not.toHaveBeenCalled();
  expect(await client.stop(identity)).toBe(true);
  expect(native.commands.deviceRecordingStop).toHaveBeenCalledWith(identity);
});

it("fences callbacks and unsubscribes again after late native subscription admission", async () => {
  let resolve!: (value: unknown) => void;
  native.commands.deviceRecordingSubscribe.mockImplementation(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  const client = createDeviceRecordingClient({ scope });
  const changed = vi.fn();
  const pending = client.subscribe(changed);
  client.dispose();
  native.channels[0].onmessage?.(1);
  resolve({ status: "ok", data: null });
  const close = await pending;
  native.channels[0].onmessage?.(2);
  close();
  expect(changed).not.toHaveBeenCalled();
  const id = native.commands.deviceRecordingSubscribe.mock.calls[0][0];
  expect(native.commands.deviceRecordingUnsubscribe.mock.calls).toEqual([
    [id],
    [id],
  ]);
  expect(native.commands.deviceRecordingStop).not.toHaveBeenCalled();
});

it("lists retained device captures without selecting a signed-in destination", async () => {
  const monitor = createDeviceRecordingMonitor();
  native.commands.deviceRecordingRecoveryInventory.mockResolvedValue({
    status: "ok",
    data: [{ identity }],
  });
  expect(await monitor.recoveryInventory()).toEqual([{ identity }]);
  expect(native.commands.deviceRecordingRecoveryInventory).toHaveBeenCalledWith(
    false,
  );
  expect(native.commands.deviceRecordingStart).not.toHaveBeenCalled();
  monitor.dispose();
});

it("retirement is absent from recovery-only monitors and refuses offline or foreign scope before dispatch", async () => {
  expect("retire" in createDeviceRecordingMonitor()).toBe(false);
  const offline = createDeviceRecordingClient({ scope });
  await expect(
    offline.retire(identity, "cleanup", () => true),
  ).rejects.toThrow();
  const client = createDeviceRecordingClient({
    scope,
    connectionId: "original",
  });
  await expect(
    client.retire(identity, "cleanup", () => false),
  ).rejects.toThrow();
  await expect(
    client.retire(
      { ...identity, scope: { ...scope, vault_id: "other" } },
      "cleanup",
      () => true,
    ),
  ).rejects.toMatchObject({ detail: { mutation_outcome: "not_committed" } });
  expect(native.commands.deviceRecordingRetire).not.toHaveBeenCalled();
});
it("retirement keeps the original connection and request after unknown IPC completion and fences late owner results", async () => {
  const client = createDeviceRecordingClient({
    scope,
    connectionId: "original",
  });
  native.commands.deviceRecordingRetire.mockRejectedValueOnce(
    new Error("lost reply"),
  );
  await expect(
    client.retire(identity, "same-cleanup", () => true),
  ).rejects.toMatchObject({
    detail: { request_id: "same-cleanup", mutation_outcome: "unknown" },
  });
  let finish!: (value: unknown) => void;
  native.commands.deviceRecordingRetire.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  let current = true;
  const pending = client.retire(identity, "same-cleanup", () => current);
  current = false;
  finish({
    status: "ok",
    data: { identity, retirement: { state: "retired" } },
  });
  await expect(pending).rejects.toMatchObject({
    detail: { request_id: "same-cleanup", mutation_outcome: "unknown" },
  });
  expect(native.commands.deviceRecordingRetire.mock.calls).toEqual([
    ["original", identity, "same-cleanup"],
    ["original", identity, "same-cleanup"],
  ]);
  client.dispose();
  await expect(
    client.retire(identity, "same-cleanup", () => true),
  ).rejects.toThrow();
  expect(native.commands.deviceRecordingRetire).toHaveBeenCalledTimes(2);
});

it("never appends from an offline monitor or a retired original scope", async () => {
  const monitor = createDeviceRecordingMonitor();
  expect("append" in monitor).toBe(false);
  const request = {
    session_id: "note",
    request_id: "request",
    capture_id: "capture",
    admission_id: "admission",
    processing: null,
  };
  const offline = createDeviceRecordingClient({ scope });
  await expect(offline.append(request, () => true)).rejects.toThrow();
  const connected = createDeviceRecordingClient({
    scope,
    connectionId: "original",
  });
  await expect(connected.append(request, () => false)).rejects.toThrow();
  await expect(connected.pendingAppendRequests(() => false)).rejects.toThrow();
  expect(native.commands.deviceRecordingAppend).not.toHaveBeenCalled();
  expect(native.commands.deviceRecordingAppendPending).not.toHaveBeenCalled();
});

it("keeps all append IDs and options after a lost reply and fences late results", async () => {
  const request = {
    session_id: "note",
    request_id: "request",
    capture_id: "capture",
    admission_id: "admission",
    processing: null,
  };
  const client = createDeviceRecordingClient({
    scope,
    connectionId: "original",
  });
  native.commands.deviceRecordingAppend.mockRejectedValueOnce(
    new Error("lost"),
  );
  await expect(client.append(request, () => true)).rejects.toMatchObject({
    detail: { mutation_outcome: "unknown" },
  });
  native.commands.deviceRecordingAppend.mockResolvedValueOnce({
    status: "ok",
    data: { identity },
  });
  await client.append(request, () => true);
  expect(native.commands.deviceRecordingAppend.mock.calls[0]).toEqual(
    native.commands.deviceRecordingAppend.mock.calls[1],
  );
  expect(native.commands.deviceRecordingAppend).toHaveBeenLastCalledWith(
    "original",
    { ...request, scope },
  );
  let resolve!: (value: unknown) => void;
  let current = true;
  native.commands.deviceRecordingAppend.mockImplementationOnce(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  const pending = client.append(request, () => current);
  current = false;
  resolve({ status: "ok", data: { identity } });
  await expect(pending).rejects.toThrow();
  expect(native.commands.deviceRecordingStart).not.toHaveBeenCalled();
});
