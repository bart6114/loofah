import { beforeEach, expect, it, vi } from "vitest";

const device = vi.hoisted(() => ({
  audioExist: vi.fn(),
  audioPath: vi.fn(),
  audioPeaks: vi.fn(),
  convert: vi.fn(),
  metadata: vi.fn(),
}));
vi.mock("@hypr/plugin-fs-sync", () => ({ commands: device }));
vi.mock("@tauri-apps/api/core", () => ({ convertFileSrc: device.convert }));
vi.mock("~/types/tauri.gen", () => ({
  commands: { applicationAudioMetadata: device.metadata },
}));

import { openLocalRecording, openRemoteRecording } from "./playback";
import type { NativeRemoteConnection } from "./remote";

beforeEach(() => {
  vi.resetAllMocks();
  device.audioExist.mockResolvedValue({ status: "ok", data: true });
  device.audioPath.mockResolvedValue({
    status: "ok",
    data: "/vault/sessions/id/audio.wav",
  });
  device.audioPeaks.mockResolvedValue({
    status: "ok",
    data: { durationSec: 1, channels: [[0, 1]] },
  });
  device.convert.mockImplementation(
    (path, scheme = "asset") => `${scheme}://${path}`,
  );
  device.metadata.mockResolvedValue({
    status: "ok",
    data: { sha256: "current-local-sha" },
  });
});

it("keeps login-free local playback paths inside the device adapter", async () => {
  const recording = await openLocalRecording("session");
  expect(recording!.url).toBe("asset:///vault/sessions/id/audio.wav");
  expect(await recording!.peaks()).toEqual({
    durationSec: 1,
    channels: [[0, 1]],
  });
  expect(await recording!.deletionRevision()).toBe("current-local-sha");
  expect(device.metadata).toHaveBeenCalledExactlyOnceWith("session");
  await recording!.close();
  await expect(recording!.peaks()).rejects.toThrow();
  expect(device.audioPath).toHaveBeenCalledExactlyOnceWith("session");
});

it("distinguishes missing local recordings from IO failures and cancelled work", async () => {
  device.audioExist.mockResolvedValueOnce({ status: "ok", data: false });
  expect(await openLocalRecording("missing")).toBeNull();
  expect(device.audioPath).not.toHaveBeenCalled();
  device.audioExist.mockResolvedValueOnce({
    status: "error",
    error: "read failed",
  });
  await expect(openLocalRecording("broken")).rejects.toThrow("read failed");
  const abort = new AbortController();
  device.audioPath.mockImplementationOnce(async () => {
    abort.abort();
    return { status: "ok", data: "private" };
  });
  await expect(
    openLocalRecording("cancelled", { signal: abort.signal }),
  ).rejects.toThrow();
  expect(device.convert).not.toHaveBeenCalled();
});

it("uses an opaque native URI for remote playback without exposing credentials or server paths", async () => {
  const close = vi.fn().mockResolvedValue(undefined);
  const peaks = { durationSec: 1, channels: [[1]] };
  const connection = {
    openRecording: vi.fn().mockResolvedValue({
      id: "opaque-id",
      metadata: { filename: "audio.wav", sha256: "sha" },
    }),
    recordingWaveform: vi
      .fn()
      .mockResolvedValue({ recording_sha256: "sha", peaks }),
    closeRecording: close,
  } as unknown as NativeRemoteConnection;
  const signal = new AbortController().signal;
  const recording = await openRemoteRecording(connection, "vault", "session", {
    signal,
  });
  expect(recording!.url).toBe("loofah-media://opaque-id");
  expect(connection.openRecording).toHaveBeenCalledWith("vault", "session", {
    signal,
  });
  expect(await recording!.peaks({ signal })).toEqual(peaks);
  expect(await recording!.deletionRevision()).toBe("sha");
  expect(device.metadata).not.toHaveBeenCalled();
  expect(connection.recordingWaveform).toHaveBeenCalledWith("opaque-id", {
    signal,
  });
  await recording!.close();
  expect(close).toHaveBeenCalledWith("opaque-id");
  expect(device.audioPath).not.toHaveBeenCalled();
});

it("releases a remote lease if device URI conversion fails", async () => {
  const close = vi.fn().mockResolvedValue(undefined);
  const connection = {
    openRecording: vi
      .fn()
      .mockResolvedValue({ id: "id", metadata: { sha256: "sha" } }),
    closeRecording: close,
  } as unknown as NativeRemoteConnection;
  device.convert.mockImplementationOnce(() => {
    throw new Error("URI unavailable");
  });
  await expect(
    openRemoteRecording(connection, "vault", "session"),
  ).rejects.toThrow("URI unavailable");
  expect(close).toHaveBeenCalledWith("id");
});
