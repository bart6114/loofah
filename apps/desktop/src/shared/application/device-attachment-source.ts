import type { FileUploadCandidate } from "@hypr/editor/note";

import { applicationResult, ApplicationRequestError } from "./errors";
import type { UploadAttachmentRequest } from "./protocol.gen";
import type { RequestOptions } from "./vault-client";

import { commands, type DeviceAttachmentSource } from "~/types/tauri.gen";

export type PreparedAttachmentUpload = {
  input: UploadAttachmentRequest;
  source: DeviceAttachmentSource;
  close: () => Promise<void>;
};

const CHUNK_BYTES = 64 * 1024;

export function deviceFileName(path: string) {
  return path.split(/[\\/]/).pop() || path;
}

export async function prepareDeviceAttachmentSource(
  candidate: FileUploadCandidate,
  options?: RequestOptions,
): Promise<PreparedAttachmentUpload> {
  const signal = options?.signal;
  signal?.throwIfAborted();
  if (candidate.kind === "path") {
    const input = applicationResult(
      await commands.devicePrepareAttachmentUpload(candidate.path),
    );
    signal?.throwIfAborted();
    return {
      input,
      source: { kind: "path", path: candidate.path },
      close: async () => {},
    };
  }
  if (!Number.isSafeInteger(candidate.file.size) || candidate.file.size < 0)
    throw invalidSource();
  const id = crypto.randomUUID();
  let closing: Promise<void> | undefined;
  const close = () => (closing ??= commands.deviceAttachmentSourceClose(id));
  try {
    applicationResult(
      await commands.deviceAttachmentSourceBegin(
        id,
        candidate.name,
        candidate.file.size,
      ),
    );
    signal?.throwIfAborted();
    for (let offset = 0; offset < candidate.file.size; offset += CHUNK_BYTES) {
      signal?.throwIfAborted();
      const bytes = new Uint8Array(
        await candidate.file
          .slice(offset, Math.min(candidate.file.size, offset + CHUNK_BYTES))
          .arrayBuffer(),
      );
      signal?.throwIfAborted();
      if (bytes.length !== Math.min(CHUNK_BYTES, candidate.file.size - offset))
        throw invalidSource();
      const received = applicationResult(
        await commands.deviceAttachmentSourceAppend(
          id,
          offset,
          Array.from(bytes),
        ),
      );
      if (received !== offset + bytes.length) throw invalidSource();
    }
    const input = applicationResult(
      await commands.deviceAttachmentSourceFinish(id),
    );
    signal?.throwIfAborted();
    if (
      input.filename !== candidate.name ||
      input.size_bytes !== candidate.file.size ||
      !/^[0-9a-f]{64}$/.test(input.sha256)
    )
      throw invalidSource();
    return { input, source: { kind: "staged", id }, close };
  } catch (cause) {
    await close().catch(() => {});
    throw cause;
  }
}

function invalidSource() {
  return new ApplicationRequestError({
    code: "validation",
    message: "The selected attachment source could not be verified",
    request_id: null,
    retryable: false,
    current_revision: null,
    mutation_outcome: "not_committed",
  });
}
