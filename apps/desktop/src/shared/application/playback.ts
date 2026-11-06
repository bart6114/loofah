import { convertFileSrc } from "@tauri-apps/api/core";

import { commands as fsSyncCommands } from "@hypr/plugin-fs-sync";

import { applicationResult, connectionClosed } from "./errors";
import type { AudioPeaks } from "./protocol.gen";
import type { NativeRemoteConnection } from "./remote";
import type { RequestOptions } from "./vault-client";

import { commands } from "~/types/tauri.gen";

export type PlaybackRecording = {
  readonly id: string;
  readonly url: string;
  readonly resumeKey: string;
  deletionRevision: (options?: RequestOptions) => Promise<string | null>;
  peaks: (options?: RequestOptions) => Promise<AudioPeaks | null>;
  close: () => Promise<void>;
};

export async function openLocalRecording(
  sessionId: string,
  options?: RequestOptions,
): Promise<PlaybackRecording | null> {
  options?.signal?.throwIfAborted();
  const exists = await fsSyncCommands.audioExist(sessionId);
  if (exists.status === "error") throw new Error(exists.error);
  if (!exists.data) return null;
  const path = await fsSyncCommands.audioPath(sessionId);
  if (path.status === "error") throw new Error(path.error);
  options?.signal?.throwIfAborted();
  let closed = false;
  return {
    id: crypto.randomUUID(),
    url: convertFileSrc(path.data),
    resumeKey: path.data,
    deletionRevision: async (request) => {
      if (closed) throw connectionClosed();
      request?.signal?.throwIfAborted();
      const metadata = applicationResult(
        await commands.applicationAudioMetadata(sessionId),
      );
      if (closed) throw connectionClosed();
      request?.signal?.throwIfAborted();
      return metadata?.sha256 ?? null;
    },
    peaks: async (request) => {
      if (closed) throw connectionClosed();
      request?.signal?.throwIfAborted();
      const peaks = await fsSyncCommands.audioPeaks(sessionId);
      if (closed) throw connectionClosed();
      request?.signal?.throwIfAborted();
      if (peaks.status === "error") throw new Error(peaks.error);
      return peaks.data;
    },
    close: async () => {
      closed = true;
    },
  };
}

export async function openRemoteRecording(
  connection: NativeRemoteConnection,
  vaultId: string,
  sessionId: string,
  options?: RequestOptions,
): Promise<PlaybackRecording | null> {
  const recording = await connection.openRecording(vaultId, sessionId, options);
  if (!recording) return null;
  try {
    return {
      id: recording.id,
      url: convertFileSrc(recording.id, "loofah-media"),
      resumeKey: recording.metadata.sha256,
      deletionRevision: async () => recording.metadata.sha256,
      peaks: async (request) =>
        (await connection.recordingWaveform(recording.id, request)).peaks,
      close: () => connection.closeRecording(recording.id),
    };
  } catch (error) {
    await connection.closeRecording(recording.id).catch(() => {});
    throw error;
  }
}
