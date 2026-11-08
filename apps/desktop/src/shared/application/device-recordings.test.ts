import { beforeEach, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  commands: {
    deviceRecordingStart: vi.fn(),
    deviceRecordingInventory: vi.fn(),
    deviceRecordingRecoveryInventory: vi.fn(),
    deviceRecordingStatus: vi.fn(),
    deviceRecordingStop: vi.fn(),
    deviceRecordingResumeUpload: vi.fn(),
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
