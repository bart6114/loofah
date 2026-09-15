import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  audioDelete: vi.fn(),
  audioMetadata: vi.fn(),
  sessionStoreAudio: vi.fn(),
  sessionListAudio: vi.fn(),
  sessionDeleteAudio: vi.fn(),
  enqueueDatabaseWrite: vi.fn(
    async (_key: string, write: () => Promise<number[]>) => write(),
  ),
}));

vi.mock("@hypr/plugin-fs-sync", () => ({
  commands: {
    audioDelete: mocks.audioDelete,
    audioMetadata: mocks.audioMetadata,
  },
}));

vi.mock("~/types/tauri.gen", () => ({
  commands: {
    sessionStoreAudio: mocks.sessionStoreAudio,
    sessionListAudio: mocks.sessionListAudio,
    sessionDeleteAudio: mocks.sessionDeleteAudio,
  },
}));

vi.mock("~/shared/write-queue", () => ({
  enqueueDatabaseWrite: mocks.enqueueDatabaseWrite,
}));

import { catalogLocalSessionAudio, deleteSessionAudio } from "./attachments";

describe("attachment catalog", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.audioDelete.mockResolvedValue({ status: "ok", data: true });
    mocks.audioMetadata.mockResolvedValue({
      status: "ok",
      data: {
        filename: "audio.mp3",
        contentType: "audio/mpeg",
        sizeBytes: 84,
        sha256: "d".repeat(64),
      },
    });
    mocks.sessionStoreAudio.mockResolvedValue({
      status: "ok",
      data: "/vault/sessions/session-1/audio.wav",
    });
    mocks.sessionListAudio.mockResolvedValue({ status: "ok", data: [] });
    mocks.sessionDeleteAudio.mockResolvedValue({ status: "ok", data: null });
  });

  // REGRESSION: this used to discard the store's return value, so callers kept using the
  // path they passed in — which the store had just moved out from under them.
  it("returns the path the recording actually settled at", async () => {
    await expect(
      catalogLocalSessionAudio("session-1", "/tmp/recording.wav"),
    ).resolves.toBe("/vault/sessions/session-1/audio.wav");

    expect(mocks.sessionStoreAudio).toHaveBeenCalledWith(
      "session-1",
      "/tmp/recording.wav",
    );
  });

  it("surfaces a store failure instead of swallowing it", async () => {
    mocks.sessionStoreAudio.mockResolvedValue({
      status: "error",
      error: "disk full",
    });

    await expect(
      catalogLocalSessionAudio("session-1", "/tmp/recording.wav"),
    ).rejects.toThrow("disk full");
  });

  it("deletes local audio bytes from both the flat and store-owned locations", async () => {
    mocks.sessionListAudio.mockResolvedValue({
      status: "ok",
      data: ["recording.wav"],
    });

    await expect(deleteSessionAudio("session-1", () => true)).resolves.toBe(
      true,
    );
    expect(mocks.audioDelete).toHaveBeenCalledWith("session-1");
    expect(mocks.sessionListAudio).toHaveBeenCalledWith("session-1");
    expect(mocks.sessionDeleteAudio).toHaveBeenCalledWith(
      "session-1",
      "recording.wav",
    );
  });

  it("deletes the recording without touching the database", async () => {
    await expect(deleteSessionAudio("session-1", () => true)).resolves.toBe(
      true,
    );
    expect(mocks.audioDelete).toHaveBeenCalledWith("session-1");
  });

  it("completes logical deletion when local audio is already absent", async () => {
    mocks.audioDelete.mockResolvedValue({ status: "ok", data: false });

    await expect(deleteSessionAudio("session-1", () => true)).resolves.toBe(
      true,
    );
  });

  it("rechecks capture safety inside the serialized delete operation", async () => {
    await expect(deleteSessionAudio("session-1", () => false)).resolves.toBe(
      false,
    );
    expect(mocks.audioDelete).not.toHaveBeenCalled();
  });
});
